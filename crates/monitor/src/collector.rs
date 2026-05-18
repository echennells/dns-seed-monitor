use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use anyhow::Result;
use bitcoin::p2p::Magic;
use chrono::Utc;
use futures::stream::{FuturesUnordered, StreamExt};
use rusqlite::params;
use tokio::sync::Semaphore;

use crate::alerting;
use crate::config::{AlertingConfig, CollectorConfig};
use crate::db::Db;
use crate::dns::DnsClient;
use crate::p2p::handshake::{handshake, PeerInfo, ProbeError};
use seeds::{Network, Seed};

pub async fn run(db: Arc<Db>, cfg: CollectorConfig, alert_cfg: AlertingConfig) -> Result<()> {
    let dns = DnsClient::system(cfg.enable_ipv6)?;
    let mut tick = tokio::time::interval(cfg.round_interval());
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        match run_round(&db, &dns, &cfg).await {
            Ok(summary) => tracing::info!(?summary, "round complete"),
            Err(e) => tracing::error!(error = ?e, "round failed"),
        }
        if let Err(e) = alerting::evaluate_and_dispatch(db.clone(), alert_cfg.clone()).await {
            tracing::error!(error = ?e, "alerting evaluation failed");
        }
    }
}

#[derive(Debug, Default)]
pub struct RoundSummary {
    pub seeds_queried: usize,
    pub unique_addresses: usize,
    pub reachable: usize,
    pub unreachable: usize,
}

pub async fn run_round(db: &Db, dns: &DnsClient, cfg: &CollectorConfig) -> Result<RoundSummary> {
    let now = Utc::now().timestamp();
    let today = now / 86400;
    let yesterday = today - 1;

    // 1. Resolve every mainnet seed in parallel. Each seed's queries are
    //    still spread out internally for politeness; the parallelism here is
    //    across seeds, not within a single seed.
    let all_seeds = seeds::all();
    let mut tasks = FuturesUnordered::new();
    for s in all_seeds {
        let host = s.hostname;
        let q = cfg.queries_per_seed;
        let spread = cfg.query_spread();
        tasks.push(async move {
            let addrs = dns.sample_seed(host, q, spread).await;
            tracing::debug!(seed = host, n = addrs.len(), "resolved");
            (s, addrs)
        });
    }
    let mut seed_to_addrs: HashMap<&Seed, HashSet<IpAddr>> = HashMap::new();
    while let Some((s, addrs)) = tasks.next().await {
        seed_to_addrs.insert(s, addrs);
    }

    // 2. Compute classification context.
    //    duplicates: address advertised by more than one seed in this round, on the same network.
    //    yesterday's set per seed: from daily_classifications.
    let mut net_addr_seed_count: HashMap<(Network, IpAddr), usize> = HashMap::new();
    for (seed, addrs) in &seed_to_addrs {
        for a in addrs {
            *net_addr_seed_count.entry((seed.network, *a)).or_insert(0) += 1;
        }
    }
    let yesterdays_addrs: HashMap<String, HashSet<String>> = {
        let conn = db.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT s.hostname, dc.address
             FROM daily_classifications dc
             JOIN seeds s ON s.id = dc.seed_id
             WHERE dc.day = ?1",
        )?;
        let mut map: HashMap<String, HashSet<String>> = HashMap::new();
        let mut rows = stmt.query(params![yesterday])?;
        while let Some(r) = rows.next()? {
            let host: String = r.get(0)?;
            let addr: String = r.get(1)?;
            map.entry(host).or_default().insert(addr);
        }
        map
    };

    // 3. Insert dns_responses + probe each (network, address) target.
    let unique_targets: Vec<(Network, IpAddr)> = net_addr_seed_count.keys().copied().collect();
    let sem = Arc::new(Semaphore::new(cfg.probe_concurrency));
    let ua = cfg.user_agent.clone();
    let to = cfg.probe_timeout();

    // Persist DNS responses.
    {
        let conn = db.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "INSERT INTO dns_responses (ts, seed_id, address, port)
             SELECT ?1, id, ?2, ?3 FROM seeds WHERE hostname = ?4",
        )?;
        for (seed, addrs) in &seed_to_addrs {
            let port = seed.network.p2p_port();
            for a in addrs {
                stmt.execute(params![now, a.to_string(), port, seed.hostname])?;
            }
        }
    }

    // Probe in parallel. Each unique (network, address) target is probed once on the
    // appropriate port/magic, even if multiple seeds on the same network advertised it.
    let mut futures = FuturesUnordered::new();
    for (net, ip) in &unique_targets {
        let net = *net;
        let ip = *ip;
        let sock = SocketAddr::new(ip, net.p2p_port());
        let magic = Magic::from_bytes(net.magic());
        let sem = sem.clone();
        let ua = ua.clone();
        futures.push(async move {
            let _permit = sem.acquire_owned().await.unwrap();
            let res = handshake(sock, magic, &ua, to).await;
            ((net, ip), res)
        });
    }

    let mut probe_results: HashMap<(Network, IpAddr), Result<PeerInfo, ProbeError>> =
        HashMap::new();
    while let Some((key, res)) = futures.next().await {
        probe_results.insert(key, res);
    }

    // 4. Persist probes + daily_classifications + probes_hourly.
    let bucket = (now / 3600) * 3600;
    let mut reach_count = 0usize;
    let mut unreach_count = 0usize;
    {
        let mut conn = db.conn.lock().unwrap();
        let tx = conn.transaction()?;

        // Per-seed hourly counters
        let mut hourly: HashMap<i64, (i64, i64)> = HashMap::new();

        for (seed, addrs) in &seed_to_addrs {
            let seed_id: i64 = tx.query_row(
                "SELECT id FROM seeds WHERE hostname = ?1",
                params![seed.hostname],
                |r| r.get(0),
            )?;
            let yesterday_set = yesterdays_addrs.get(seed.hostname);

            for ip in addrs {
                let addr_s = ip.to_string();
                let key = (seed.network, *ip);
                let result = probe_results.get(&key);
                let success = matches!(result, Some(Ok(_)));

                // Insert probe row (one per (seed, address) — same probe result
                // attributed to each seed that advertised the address).
                match result {
                    Some(Ok(info)) => {
                        tx.execute(
                            "INSERT INTO probes (ts, seed_id, address, port, success, latency_ms,
                                                 user_agent, services, version, height, error_kind)
                             VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, ?7, ?8, ?9, NULL)",
                            params![
                                now,
                                seed_id,
                                addr_s,
                                seed.network.p2p_port(),
                                info.latency_ms as i64,
                                info.user_agent,
                                info.services as i64,
                                info.version,
                                info.height,
                            ],
                        )?;
                    }
                    Some(Err(e)) => {
                        tx.execute(
                            "INSERT INTO probes (ts, seed_id, address, port, success, error_kind)
                             VALUES (?1, ?2, ?3, ?4, 0, ?5)",
                            params![now, seed_id, addr_s, seed.network.p2p_port(), e.kind()],
                        )?;
                    }
                    None => continue,
                }

                // Classify.
                let class = if !success {
                    "unreachable"
                } else if net_addr_seed_count.get(&key).copied().unwrap_or(0) > 1 {
                    "duplicate"
                } else if yesterday_set.map(|s| s.contains(&addr_s)).unwrap_or(false) {
                    "stale"
                } else if yesterday_set.is_some() {
                    "pristine"
                } else {
                    "reachable"
                };

                tx.execute(
                    "INSERT INTO daily_classifications (day, seed_id, address, class)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(day, seed_id, address) DO UPDATE SET class = excluded.class
                     WHERE excluded.class != 'unreachable' OR daily_classifications.class = 'unreachable'",
                    params![today, seed_id, addr_s, class],
                )?;

                let entry = hourly.entry(seed_id).or_insert((0, 0));
                if success {
                    entry.0 += 1;
                    reach_count += 1;
                } else {
                    entry.1 += 1;
                    unreach_count += 1;
                }
            }
        }

        for (seed_id, (r, u)) in hourly {
            tx.execute(
                "INSERT INTO probes_hourly (bucket, seed_id, reachable, unreachable)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(bucket, seed_id) DO UPDATE SET
                   reachable   = reachable   + excluded.reachable,
                   unreachable = unreachable + excluded.unreachable",
                params![bucket, seed_id, r, u],
            )?;
        }

        // (user_agents_daily is intentionally not written here — it has no network
        //  column, would mix UAs across networks, and the API reads UAs from `probes`.)

        tx.commit()?;
    }

    Ok(RoundSummary {
        seeds_queried: seed_to_addrs.len(),
        unique_addresses: unique_targets.len(),
        reachable: reach_count,
        unreachable: unreach_count,
    })
}
