//! Fixed-seed decay monitoring.
//!
//! Bitcoin Core ships a compiled-in fallback list of peer addresses (the "fixed
//! seeds", from `contrib/seeds/nodes_{main,test,signet}.txt` -> `chainparams_seeds.h`)
//! updated each release. This module parses those lists per release tag, probes
//! the clearnet entries, and tracks how reachability decays over time after a
//! release ships.
//!
//! v1: clearnet only (IPv4/IPv6). Onion / i2p / cjdns are filtered out at
//! ingest because probing them requires Tor/I2P infrastructure not present here.

use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use bitcoin::p2p::Magic;
use chrono::Utc;
use futures::stream::{FuturesUnordered, StreamExt};
use rusqlite::params;
use tokio::sync::Semaphore;

use crate::db::Db;
use crate::p2p::handshake::handshake;
use seeds::Network;

/// Core releases we track. (tag, mainnet released_at unix). Each tag's
/// `nodes_*.txt` files will be fetched relative to this tag.
///
/// Dates are approximate (release-day UTC midnight) and only used to compute
/// "age since release" on the dashboard.
const RELEASES: &[(&str, i64)] = &[
    ("v26.0",  1701907200),  // 2023-12-07
    ("v26.1",  1712016000),  // 2024-04-02
    ("v27.0",  1713312000),  // 2024-04-17
    ("v27.1",  1721174400),  // 2024-07-17
    ("v28.0",  1727913600),  // 2024-10-03
    ("v28.1",  1733184000),  // 2024-12-03
    ("v29.0",  1744156800),  // 2025-04-09 (approx)
    ("v30.0",  1759708800),  // 2025-10-06 (approx)
    ("v31.0",  1775606400),  // 2026-04-09 (approx)
];

const NETWORKS: &[(&str, &str)] = &[
    ("mainnet", "nodes_main.txt"),
    ("testnet", "nodes_test.txt"),
    ("signet",  "nodes_signet.txt"),
];

pub async fn run(db: Arc<Db>, probe_interval: Duration, ua: String, probe_concurrency: usize, probe_timeout: Duration) -> Result<()> {
    // Ingest once at startup (idempotent), then loop probing.
    if let Err(e) = ingest_all(&db).await {
        tracing::error!(error = ?e, "fixed-seed ingestion failed at startup");
    }

    let mut tick = tokio::time::interval(probe_interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        if let Err(e) = probe_round(&db, &ua, probe_concurrency, probe_timeout).await {
            tracing::error!(error = ?e, "fixed-seed probe round failed");
        }
    }
}

/// Fetch nodes_*.txt for every (tag, network) we don't already have, parse,
/// filter to clearnet, persist.
async fn ingest_all(db: &Db) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent("dctrl-yvr-seed-monitor")
        .build()?;

    // Figure out which (tag, network) we still need.
    let already: HashSet<(String, String)> = {
        let conn = db.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT tag, network FROM fixed_seed_releases")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter().collect()
    };

    let now = Utc::now().timestamp();
    let mut total_ingested = 0usize;

    for (tag, released_at) in RELEASES {
        for (network, file) in NETWORKS {
            if already.contains(&(tag.to_string(), network.to_string())) {
                continue;
            }
            let url = format!("https://raw.githubusercontent.com/bitcoin/bitcoin/{}/contrib/seeds/{}", tag, file);
            tracing::info!(tag, network, "ingesting fixed seeds");
            let resp = match client.get(&url).send().await {
                Ok(r) => r,
                Err(e) => { tracing::warn!(tag, network, error = ?e, "fetch failed"); continue; }
            };
            if !resp.status().is_success() {
                tracing::warn!(tag, network, status = ?resp.status(), "non-2xx response");
                continue;
            }
            let body = resp.text().await.context("read body")?;
            let entries = parse_nodes_file(&body, network);

            let mut conn = db.conn.lock().unwrap();
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT OR IGNORE INTO fixed_seed_releases (tag, network, released_at, ingested_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![tag, network, released_at, now],
            )?;
            for (addr, port) in &entries {
                tx.execute(
                    "INSERT OR IGNORE INTO fixed_seeds (tag, network, address, port)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![tag, network, addr, port],
                )?;
            }
            tx.commit()?;
            total_ingested += entries.len();
            tracing::info!(tag, network, n = entries.len(), "ingested fixed seeds");
        }
    }

    if total_ingested > 0 {
        tracing::info!(total_ingested, "fixed-seed ingestion complete");
    }
    Ok(())
}

/// Parse a `nodes_*.txt` file: one `addr:port` per line, `#` comments, blank
/// lines ignored. Filters out non-clearnet entries (`.onion`, `.b32.i2p`,
/// cjdns `fc00::/8`).
fn parse_nodes_file(text: &str, _network: &str) -> Vec<(String, u16)> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() { continue; }
        if line.contains(".onion") || line.contains(".b32.i2p") { continue; }

        // Split host:port. IPv6 addresses come bracketed: [::1]:8333
        let (host, port) = if let Some(rest) = line.strip_prefix('[') {
            let Some((h, tail)) = rest.split_once(']') else { continue; };
            let Some(p) = tail.strip_prefix(':') else { continue; };
            (h.to_string(), p)
        } else {
            let Some((h, p)) = line.rsplit_once(':') else { continue; };
            (h.to_string(), p)
        };
        let Ok(port) = port.parse::<u16>() else { continue; };

        // cjdns range filter: anything in fc00::/8.
        if let Ok(ip) = host.parse::<IpAddr>() {
            if let IpAddr::V6(v6) = ip {
                if v6.octets()[0] == 0xfc { continue; }
            }
            out.push((ip.to_string(), port));
        }
    }
    out
}

async fn probe_round(db: &Db, ua: &str, concurrency: usize, timeout: Duration) -> Result<()> {
    // Distinct (network, address, port) across all releases.
    let targets: Vec<(String, String, u16)> = {
        let conn = db.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT network, address, port FROM fixed_seeds"
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)? as u16)))?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    if targets.is_empty() {
        tracing::debug!("no fixed seeds to probe yet");
        return Ok(());
    }
    tracing::info!(n = targets.len(), "probing fixed seeds");

    let sem = Arc::new(Semaphore::new(concurrency));
    let mut futures = FuturesUnordered::new();
    for (network_str, addr_s, port) in &targets {
        let network = match network_str.as_str() {
            "mainnet" => Network::Mainnet,
            "testnet" => Network::Testnet,
            "signet"  => Network::Signet,
            _ => continue,
        };
        let Ok(ip) = addr_s.parse::<IpAddr>() else { continue };
        let sock = SocketAddr::new(ip, *port);
        let magic = Magic::from_bytes(network.magic());
        let sem = sem.clone();
        let ua = ua.to_string();
        let addr_s = addr_s.clone();
        let network_str = network_str.clone();
        let port = *port;
        futures.push(async move {
            let _permit = sem.acquire_owned().await.unwrap();
            let res = handshake(sock, magic, &ua, timeout).await;
            (network_str, addr_s, port, res)
        });
    }

    // Drain all probes first — don't hold the DB lock across awaits.
    let mut results: Vec<(String, String, u16, std::result::Result<crate::p2p::handshake::PeerInfo, crate::p2p::handshake::ProbeError>)> =
        Vec::with_capacity(targets.len());
    while let Some(tuple) = futures.next().await {
        results.push(tuple);
    }

    // Single transaction for all inserts.
    let now = Utc::now().timestamp();
    let mut succ = 0usize;
    let mut fail = 0usize;
    {
        let mut conn_guard = db.conn.lock().unwrap();
        let tx = conn_guard.transaction()?;
        for (network, addr_s, port, res) in &results {
            match res {
                Ok(info) => {
                    tx.execute(
                        "INSERT INTO fixed_seed_probes (ts, network, address, port, success, latency_ms, user_agent)
                         VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6)",
                        params![now, network, addr_s, port, info.latency_ms as i64, info.user_agent],
                    )?;
                    succ += 1;
                }
                Err(e) => {
                    tx.execute(
                        "INSERT INTO fixed_seed_probes (ts, network, address, port, success, error_kind)
                         VALUES (?1, ?2, ?3, ?4, 0, ?5)",
                        params![now, network, addr_s, port, e.kind()],
                    )?;
                    fail += 1;
                }
            }
        }
        tx.commit()?;
    }
    tracing::info!(reachable = succ, unreachable = fail, "fixed-seed probe round complete");
    Ok(())
}
