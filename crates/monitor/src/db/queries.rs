use anyhow::Result;
use rusqlite::{params, Connection};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct SeedRow {
    pub id: i64,
    pub hostname: String,
    pub network: String,
    pub operator: Option<String>,
}

pub fn list_seeds(conn: &Connection) -> Result<Vec<SeedRow>> {
    let mut stmt =
        conn.prepare("SELECT id, hostname, network, operator FROM seeds ORDER BY hostname")?;
    let rows = stmt
        .query_map([], |r| {
            Ok(SeedRow {
                id: r.get(0)?,
                hostname: r.get(1)?,
                network: r.get(2)?,
                operator: r.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[derive(Debug, Clone, Serialize)]
pub struct HourlyPoint {
    pub bucket: i64,
    pub seed_id: i64,
    pub reachable: i64,
    pub unreachable: i64,
}

pub fn hourly_for_seed(
    conn: &Connection,
    seed_id: i64,
    from: i64,
    to: i64,
) -> Result<Vec<HourlyPoint>> {
    let mut stmt = conn.prepare(
        "SELECT bucket, seed_id, reachable, unreachable
         FROM probes_hourly
         WHERE seed_id = ?1 AND bucket BETWEEN ?2 AND ?3
         ORDER BY bucket",
    )?;
    let rows = stmt
        .query_map(params![seed_id, from, to], |r| {
            Ok(HourlyPoint {
                bucket: r.get(0)?,
                seed_id: r.get(1)?,
                reachable: r.get(2)?,
                unreachable: r.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[derive(Debug, Clone, Serialize)]
pub struct ReachableByNetworkRow {
    pub bucket: i64,
    pub network_type: String, // ipv4|ipv6
    pub count: i64,
}

pub fn reachable_by_network(
    conn: &Connection,
    from: i64,
    to: i64,
    network: &str,
) -> Result<Vec<ReachableByNetworkRow>> {
    let mut stmt = conn.prepare(
        "SELECT
           (p.ts / 3600) * 3600 AS bucket,
           CASE WHEN instr(p.address, ':') > 0 THEN 'ipv6' ELSE 'ipv4' END AS net_type,
           count(DISTINCT p.address)
         FROM probes p JOIN seeds s ON s.id = p.seed_id
         WHERE p.success = 1 AND p.ts BETWEEN ?1 AND ?2 AND (?3 = 'all' OR s.network = ?3)
         GROUP BY bucket, net_type
         ORDER BY bucket",
    )?;
    let rows = stmt
        .query_map(params![from, to, network], |r| {
            Ok(ReachableByNetworkRow {
                bucket: r.get(0)?,
                network_type: r.get(1)?,
                count: r.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[derive(Debug, Clone, Serialize)]
pub struct UniqueReachablePoint {
    pub bucket: i64,
    pub count: i64,
}

pub fn unique_reachable(
    conn: &Connection,
    from: i64,
    to: i64,
    network: &str,
) -> Result<Vec<UniqueReachablePoint>> {
    let mut stmt = conn.prepare(
        "SELECT (p.ts / 86400) * 86400 AS bucket, count(DISTINCT p.address)
         FROM probes p JOIN seeds s ON s.id = p.seed_id
         WHERE p.success = 1 AND p.ts BETWEEN ?1 AND ?2 AND (?3 = 'all' OR s.network = ?3)
         GROUP BY bucket
         ORDER BY bucket",
    )?;
    let rows = stmt
        .query_map(params![from, to, network], |r| {
            Ok(UniqueReachablePoint {
                bucket: r.get(0)?,
                count: r.get(1)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[derive(Debug, Clone, Serialize)]
pub struct UserAgentRow {
    pub user_agent: String,
    pub count: i64,
}

pub fn top_user_agents(
    conn: &Connection,
    from: i64,
    to: i64,
    limit: i64,
    network: &str,
) -> Result<Vec<UserAgentRow>> {
    let mut stmt = conn.prepare(
        "SELECT p.user_agent, count(*) AS n
         FROM probes p JOIN seeds s ON s.id = p.seed_id
         WHERE p.success = 1 AND p.user_agent IS NOT NULL
           AND p.ts BETWEEN ?1 AND ?2 AND (?3 = 'all' OR s.network = ?3)
         GROUP BY p.user_agent
         ORDER BY n DESC
         LIMIT ?4",
    )?;
    let rows = stmt
        .query_map(params![from, to, network, limit], |r| {
            Ok(UserAgentRow {
                user_agent: r.get(0)?,
                count: r.get(1)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[derive(Debug, Clone, Serialize)]
pub struct AlertRow {
    pub seed_id: i64,
    pub hostname: String,
    pub network: String,
    pub operator: Option<String>,
    pub level: String,
    pub reason: String,
    pub reachable: i64,
    pub unreachable: i64,
    pub last_round: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct FixedSeedDecayRow {
    pub tag: String,
    pub network: String,
    pub released_at: i64,
    pub total: i64,
    pub reachable_recent: i64,
    pub probed_recent: i64,
}

/// Per-release snapshot of fixed-seed reachability, using the most recent
/// probe per (network, address) within `lookback_secs`.
pub fn fixed_seed_decay(
    conn: &Connection,
    network: &str,
    lookback_secs: i64,
) -> Result<Vec<FixedSeedDecayRow>> {
    // For each (network, address), grab the latest probe in the lookback window.
    // Then for each (tag, network) count totals and how many of *its* addresses
    // had a successful latest probe.
    let mut stmt = conn.prepare(
        "WITH latest AS (
           SELECT network, address, success
           FROM (
             SELECT network, address, success,
                    ROW_NUMBER() OVER (PARTITION BY network, address ORDER BY ts DESC) AS rn
             FROM fixed_seed_probes
             WHERE ts > strftime('%s','now') - ?2
           )
           WHERE rn = 1
         )
         SELECT r.tag, r.network, r.released_at,
                COUNT(*) AS total,
                COALESCE(SUM(CASE WHEN l.success = 1 THEN 1 ELSE 0 END), 0) AS reachable,
                COALESCE(SUM(CASE WHEN l.address IS NOT NULL THEN 1 ELSE 0 END), 0) AS probed
         FROM fixed_seed_releases r
         JOIN fixed_seeds f ON f.tag = r.tag AND f.network = r.network
         LEFT JOIN latest l ON l.network = f.network AND l.address = f.address
         WHERE (?1 = 'all' OR r.network = ?1)
         GROUP BY r.tag, r.network
         ORDER BY r.released_at",
    )?;
    let rows = stmt
        .query_map(params![network, lookback_secs], |r| {
            Ok(FixedSeedDecayRow {
                tag: r.get(0)?,
                network: r.get(1)?,
                released_at: r.get(2)?,
                total: r.get(3)?,
                reachable_recent: r.get(4)?,
                probed_recent: r.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn alerts(conn: &Connection) -> Result<Vec<AlertRow>> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.hostname, s.network, s.operator,
                COALESCE(SUM(ph.reachable), 0),
                COALESCE(SUM(ph.unreachable), 0),
                COALESCE(MAX(ph.bucket), 0)
         FROM seeds s
         LEFT JOIN probes_hourly ph
           ON ph.seed_id = s.id AND ph.bucket > strftime('%s','now') - 86400
         GROUP BY s.id
         ORDER BY s.hostname",
    )?;
    let rows: Vec<(i64, String, String, Option<String>, i64, i64, i64)> = stmt
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let now = chrono::Utc::now().timestamp();
    let mut out = Vec::new();
    for (seed_id, hostname, network, operator, reachable, unreachable, last_round) in rows {
        let total = reachable + unreachable;
        let rate = if total > 0 {
            reachable as f64 / total as f64
        } else {
            0.0
        };
        let stale_secs = if last_round > 0 {
            now - last_round
        } else {
            i64::MAX
        };

        let alert = if total >= 5 && rate < 0.10 {
            Some((
                "critical",
                format!("dead — {}/{} reachable in 24h", reachable, total),
            ))
        } else if last_round == 0 {
            Some(("warning", "no data in last 24h".to_string()))
        } else if stale_secs > 7200 {
            Some((
                "warning",
                format!("stale — last round {}h ago", stale_secs / 3600),
            ))
        } else if total >= 30 && rate < 0.50 {
            Some((
                "warning",
                format!("degraded — {:.0}% success over 24h", rate * 100.0),
            ))
        } else {
            None
        };

        if let Some((level, reason)) = alert {
            out.push(AlertRow {
                seed_id,
                hostname,
                network,
                operator,
                level: level.to_string(),
                reason,
                reachable,
                unreachable,
                last_round,
            });
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, Serialize)]
pub struct ClassShareRow {
    pub seed_id: i64,
    pub class: String,
    pub count: i64,
}

pub fn class_shares_range(
    conn: &Connection,
    from_day: i64,
    to_day: i64,
    network: &str,
) -> Result<Vec<ClassShareRow>> {
    let mut stmt = conn.prepare(
        "SELECT dc.seed_id, dc.class, count(*)
         FROM daily_classifications dc JOIN seeds s ON s.id = dc.seed_id
         WHERE dc.day BETWEEN ?1 AND ?2 AND (?3 = 'all' OR s.network = ?3)
         GROUP BY dc.seed_id, dc.class
         ORDER BY dc.seed_id, dc.class",
    )?;
    let rows = stmt
        .query_map(params![from_day, to_day, network], |r| {
            Ok(ClassShareRow {
                seed_id: r.get(0)?,
                class: r.get(1)?,
                count: r.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[derive(Debug, Clone, Serialize)]
pub struct SeedTotalRow {
    pub seed_id: i64,
    pub addresses: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeedOverlapPair {
    pub a: i64,
    pub b: i64,
    pub shared: i64,
}

pub fn seed_totals(
    conn: &Connection,
    from: i64,
    to: i64,
    network: &str,
) -> Result<Vec<SeedTotalRow>> {
    let mut stmt = conn.prepare(
        "SELECT p.seed_id, COUNT(DISTINCT p.address)
         FROM probes p JOIN seeds s ON s.id = p.seed_id
         WHERE p.ts BETWEEN ?1 AND ?2 AND (?3 = 'all' OR s.network = ?3)
         GROUP BY p.seed_id",
    )?;
    let rows = stmt
        .query_map(params![from, to, network], |r| {
            Ok(SeedTotalRow {
                seed_id: r.get(0)?,
                addresses: r.get(1)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn seed_overlap(
    conn: &Connection,
    from: i64,
    to: i64,
    network: &str,
) -> Result<Vec<SeedOverlapPair>> {
    let mut stmt = conn.prepare(
        "WITH per AS (
           SELECT DISTINCT p.seed_id, p.address
           FROM probes p JOIN seeds s ON s.id = p.seed_id
           WHERE p.ts BETWEEN ?1 AND ?2 AND (?3 = 'all' OR s.network = ?3)
         )
         SELECT s1.seed_id AS a, s2.seed_id AS b, COUNT(*) AS shared
         FROM per s1
         JOIN per s2 ON s1.address = s2.address AND s1.seed_id < s2.seed_id
         GROUP BY s1.seed_id, s2.seed_id",
    )?;
    let rows = stmt
        .query_map(params![from, to, network], |r| {
            Ok(SeedOverlapPair {
                a: r.get(0)?,
                b: r.get(1)?,
                shared: r.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[derive(Debug, Clone, Serialize)]
pub struct VersionShareRow {
    pub seed_id: i64,
    pub major: Option<i64>,
    pub count: i64,
}

pub fn version_share_by_seed(
    conn: &Connection,
    from: i64,
    to: i64,
    network: &str,
) -> Result<Vec<VersionShareRow>> {
    let mut stmt = conn.prepare(
        "SELECT p.seed_id, p.user_agent, count(*) AS n
         FROM probes p JOIN seeds s ON s.id = p.seed_id
         WHERE p.success = 1 AND p.user_agent IS NOT NULL
           AND p.ts BETWEEN ?1 AND ?2 AND (?3 = 'all' OR s.network = ?3)
         GROUP BY p.seed_id, p.user_agent",
    )?;
    let raw: Vec<(i64, String, i64)> = stmt
        .query_map(params![from, to, network], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut agg: std::collections::HashMap<(i64, Option<i64>), i64> =
        std::collections::HashMap::new();
    for (seed_id, ua, n) in raw {
        *agg.entry((seed_id, parse_satoshi_major(&ua))).or_insert(0) += n;
    }
    let mut rows: Vec<VersionShareRow> = agg
        .into_iter()
        .map(|((seed_id, major), count)| VersionShareRow {
            seed_id,
            major,
            count,
        })
        .collect();
    rows.sort_by_key(|r| (r.seed_id, r.major.unwrap_or(i64::MAX)));
    Ok(rows)
}

fn parse_satoshi_major(ua: &str) -> Option<i64> {
    let rest = ua.strip_prefix("/Satoshi:")?;
    let dot = rest.find('.')?;
    rest[..dot].parse().ok()
}

#[derive(Debug, Clone, Serialize)]
pub struct LatestSummaryRow {
    pub seed_id: i64,
    pub last_round: i64,
    pub reachable: i64,
    pub unreachable: i64,
}

pub fn latest_summary(conn: &Connection) -> Result<Vec<LatestSummaryRow>> {
    let mut stmt = conn.prepare(
        "SELECT seed_id, MAX(bucket) AS last_round,
                SUM(reachable), SUM(unreachable)
         FROM probes_hourly
         WHERE bucket > (strftime('%s','now') - 86400)
         GROUP BY seed_id",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(LatestSummaryRow {
                seed_id: r.get(0)?,
                last_round: r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                reachable: r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                unreachable: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}
