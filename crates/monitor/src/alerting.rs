use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use rusqlite::params;
use serde_json::json;

use crate::config::AlertingConfig;
use crate::db::queries::AlertRow;
use crate::db::Db;

#[derive(Debug)]
enum Event<'a> {
    Opened(&'a AlertRow),
    Escalated(&'a AlertRow, &'a str), // from level
    Deescalated(&'a AlertRow, &'a str),
    Resolved(i64, String, String, String, String, i64), // seed_id, host, net, op, level, opened_at
}

pub async fn evaluate_and_dispatch(db: Arc<Db>, cfg: AlertingConfig) -> Result<()> {
    // 1. Compute current alerts (same logic as the dashboard).
    let current: Vec<AlertRow> = {
        let conn = db.conn.lock().unwrap();
        crate::db::queries::alerts(&conn)?
    };

    // 2. Load prior state from alert_state.
    let prior: HashMap<i64, (String, String, i64)> = {
        let conn = db.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT seed_id, level, reason, opened_at FROM alert_state")?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    (
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                    ),
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter().collect()
    };

    let now = chrono::Utc::now().timestamp();

    // 3. Diff current vs prior; collect events and update state.
    let mut events: Vec<Event> = Vec::new();
    let current_ids: std::collections::HashSet<i64> = current.iter().map(|a| a.seed_id).collect();

    for a in &current {
        match prior.get(&a.seed_id) {
            None => events.push(Event::Opened(a)),
            Some((prev_level, prev_reason, _)) => {
                if prev_level != &a.level {
                    if a.level == "critical" {
                        events.push(Event::Escalated(a, prev_level));
                    } else {
                        events.push(Event::Deescalated(a, prev_level));
                    }
                } else if prev_reason != &a.reason {
                    // Same level, different reason — treat as a quiet update (no event,
                    // but we'll update the row below).
                }
            }
        }
    }
    // Resolved = was in prior, not in current.
    let mut resolved: Vec<(i64, String, i64)> = Vec::new();
    for (seed_id, (level, _reason, opened_at)) in &prior {
        if !current_ids.contains(seed_id) {
            resolved.push((*seed_id, level.clone(), *opened_at));
        }
    }

    // Fetch hostnames for resolved alerts (since they're no longer in `current`).
    let resolved_meta: Vec<(i64, String, String, String, String, i64)> = if !resolved.is_empty() {
        let conn = db.conn.lock().unwrap();
        let placeholders = vec!["?"; resolved.len()].join(",");
        let sql = format!(
            "SELECT id, hostname, network, COALESCE(operator,'') FROM seeds WHERE id IN ({})",
            placeholders
        );
        let mut stmt = conn.prepare(&sql)?;
        let ids: Vec<i64> = resolved.iter().map(|(id, _, _)| *id).collect();
        let id_params: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|i| i as &dyn rusqlite::ToSql).collect();
        let mut hosts: HashMap<i64, (String, String, String)> = HashMap::new();
        let mut rows = stmt.query(id_params.as_slice())?;
        while let Some(r) = rows.next()? {
            let id: i64 = r.get(0)?;
            hosts.insert(id, (r.get(1)?, r.get(2)?, r.get(3)?));
        }
        resolved
            .iter()
            .filter_map(|(sid, level, opened_at)| {
                hosts.get(sid).map(|(h, n, o)| {
                    (
                        *sid,
                        h.clone(),
                        n.clone(),
                        o.clone(),
                        level.clone(),
                        *opened_at,
                    )
                })
            })
            .collect()
    } else {
        Vec::new()
    };

    for r in &resolved_meta {
        events.push(Event::Resolved(
            r.0,
            r.1.clone(),
            r.2.clone(),
            r.3.clone(),
            r.4.clone(),
            r.5,
        ));
    }

    // 4. Persist new state (upserts + delete-resolved + history) atomically.
    {
        let mut conn = db.conn.lock().unwrap();
        let tx = conn.transaction()?;
        for a in &current {
            let opened_at = prior.get(&a.seed_id).map(|(_, _, t)| *t).unwrap_or(now);
            tx.execute(
                "INSERT INTO alert_state (seed_id, level, reason, opened_at, notified_at)
                 VALUES (?1, ?2, ?3, ?4, NULL)
                 ON CONFLICT(seed_id) DO UPDATE SET
                   level = excluded.level,
                   reason = excluded.reason",
                params![a.seed_id, a.level, a.reason, opened_at],
            )?;
        }
        for (sid, _level, _opened) in &resolved {
            tx.execute("DELETE FROM alert_state WHERE seed_id = ?1", params![sid])?;
        }
        for e in &events {
            let (sid, ev, level, reason): (i64, &str, Option<&str>, Option<&str>) = match e {
                Event::Opened(a) => (a.seed_id, "opened", Some(&a.level), Some(&a.reason)),
                Event::Escalated(a, _) => (a.seed_id, "escalated", Some(&a.level), Some(&a.reason)),
                Event::Deescalated(a, _) => {
                    (a.seed_id, "deescalated", Some(&a.level), Some(&a.reason))
                }
                Event::Resolved(sid, _, _, _, level, _) => {
                    (*sid, "resolved", Some(level.as_str()), None)
                }
            };
            tx.execute(
                "INSERT INTO alert_history (ts, seed_id, event, level, reason)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![now, sid, ev, level, reason],
            )?;
        }
        tx.commit()?;
    }

    // 5. Dispatch webhook for each event, if enabled.
    if !cfg.enabled {
        return Ok(());
    }
    let Some(url) = cfg.webhook_url.as_deref() else {
        return Ok(());
    };
    if url.is_empty() {
        return Ok(());
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    for e in &events {
        let payload = match e {
            Event::Opened(a) => json!({
                "event": "opened", "ts": now,
                "seed": { "id": a.seed_id, "hostname": a.hostname, "network": a.network, "operator": a.operator },
                "level": a.level, "reason": a.reason,
                "reachable_24h": a.reachable, "unreachable_24h": a.unreachable,
                "last_round": a.last_round,
                "summary": format!("[{} {}] {} — {}", a.level.to_uppercase(), a.network, a.hostname, a.reason),
            }),
            Event::Escalated(a, prev) => json!({
                "event": "escalated", "ts": now,
                "seed": { "id": a.seed_id, "hostname": a.hostname, "network": a.network, "operator": a.operator },
                "level": a.level, "previous_level": prev, "reason": a.reason,
                "summary": format!("[{}→{} {}] {} — {}", prev.to_uppercase(), a.level.to_uppercase(), a.network, a.hostname, a.reason),
            }),
            Event::Deescalated(a, prev) => json!({
                "event": "deescalated", "ts": now,
                "seed": { "id": a.seed_id, "hostname": a.hostname, "network": a.network, "operator": a.operator },
                "level": a.level, "previous_level": prev, "reason": a.reason,
                "summary": format!("[{}→{} {}] {} — {}", prev.to_uppercase(), a.level.to_uppercase(), a.network, a.hostname, a.reason),
            }),
            Event::Resolved(sid, host, net, _op, level, opened_at) => json!({
                "event": "resolved", "ts": now,
                "seed": { "id": sid, "hostname": host, "network": net },
                "previous_level": level, "duration_secs": now - opened_at,
                "summary": format!("[RESOLVED {}] {} — recovered after {}m", net, host, (now - opened_at) / 60),
            }),
        };

        let mut req = client.post(url).json(&payload);
        if let Some(auth) = cfg.auth_header.as_deref() {
            if let Some((k, v)) = auth.split_once(':') {
                req = req.header(k.trim(), v.trim());
            }
        }
        match req.send().await {
            Ok(resp) => {
                if !resp.status().is_success() {
                    tracing::warn!(status = ?resp.status(), "alerting webhook returned non-success");
                }
            }
            Err(e) => tracing::warn!(error = ?e, "alerting webhook post failed"),
        }
    }

    if !events.is_empty() {
        tracing::info!(
            opened = events
                .iter()
                .filter(|e| matches!(e, Event::Opened(_)))
                .count(),
            resolved = events
                .iter()
                .filter(|e| matches!(e, Event::Resolved(..)))
                .count(),
            escalated = events
                .iter()
                .filter(|e| matches!(e, Event::Escalated(..)))
                .count(),
            "alert events dispatched"
        );
    }

    Ok(())
}
