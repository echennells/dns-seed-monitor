use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use axum::routing::get;
use axum::Router;
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::db::{queries, Db};

#[derive(Debug, Deserialize)]
pub struct Range {
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub network: Option<String>,
}

impl Range {
    fn resolve(&self, default_window_secs: i64) -> (i64, i64) {
        let now = Utc::now().timestamp();
        let to = self.to.unwrap_or(now);
        let from = self.from.unwrap_or(to - default_window_secs);
        (from, to)
    }
    fn net(&self) -> &str {
        let n = self.network.as_deref().unwrap_or("mainnet");
        match n {
            "mainnet" | "testnet" | "signet" | "all" => n,
            _ => "mainnet",
        }
    }
}

pub fn router(db: Arc<Db>) -> Router {
    let api = Router::new()
        .route("/healthz", get(healthz))
        .route("/api/seeds", get(list_seeds))
        .route("/api/seeds/:id/timeseries", get(seed_timeseries))
        .route("/api/nodes/reachable", get(reachable))
        .route("/api/nodes/unique", get(unique_reachable))
        .route("/api/user-agents", get(user_agents))
        .route("/api/version-share", get(version_share))
        .route("/api/class-shares", get(class_shares))
        .route("/api/seed-overlap", get(seed_overlap))
        .route("/api/alerts", get(alerts))
        .route("/api/fixed-seeds/decay", get(fixed_seed_decay))
        .with_state(db);

    Router::new()
        .merge(api)
        .nest_service("/", ServeDir::new("web"))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
}

async fn healthz() -> &'static str {
    "ok"
}

fn err(e: anyhow::Error) -> (StatusCode, String) {
    tracing::error!(error = ?e, "api error");
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

async fn list_seeds(State(db): State<Arc<Db>>) -> impl IntoResponse {
    let conn = db.conn.lock().unwrap();
    let seeds = queries::list_seeds(&conn).map_err(err)?;
    let summary = queries::latest_summary(&conn).map_err(err)?;
    Ok::<_, (StatusCode, String)>(Json(json!({
        "seeds": seeds,
        "latest": summary,
    })))
}

async fn seed_timeseries(
    State(db): State<Arc<Db>>,
    Path(id): Path<i64>,
    Query(range): Query<Range>,
) -> impl IntoResponse {
    let (from, to) = range.resolve(30 * 86400);
    let conn = db.conn.lock().unwrap();
    let points = queries::hourly_for_seed(&conn, id, from, to).map_err(err)?;
    Ok::<_, (StatusCode, String)>(Json(json!({ "seed_id": id, "points": points })))
}

async fn reachable(State(db): State<Arc<Db>>, Query(range): Query<Range>) -> impl IntoResponse {
    let (from, to) = range.resolve(30 * 86400);
    let net = range.net();
    let conn = db.conn.lock().unwrap();
    let rows = queries::reachable_by_network(&conn, from, to, net).map_err(err)?;
    Ok::<_, (StatusCode, String)>(Json(json!({ "points": rows })))
}

async fn unique_reachable(State(db): State<Arc<Db>>, Query(range): Query<Range>) -> impl IntoResponse {
    let (from, to) = range.resolve(30 * 86400);
    let net = range.net();
    let conn = db.conn.lock().unwrap();
    let rows = queries::unique_reachable(&conn, from, to, net).map_err(err)?;
    Ok::<_, (StatusCode, String)>(Json(json!({ "points": rows })))
}

async fn user_agents(State(db): State<Arc<Db>>, Query(range): Query<Range>) -> impl IntoResponse {
    let (from, to) = range.resolve(7 * 86400);
    let net = range.net();
    let conn = db.conn.lock().unwrap();
    let rows = queries::top_user_agents(&conn, from, to, 30, net).map_err(err)?;
    Ok::<_, (StatusCode, String)>(Json(json!({ "rows": rows })))
}

async fn version_share(State(db): State<Arc<Db>>, Query(range): Query<Range>) -> impl IntoResponse {
    let (from, to) = range.resolve(7 * 86400);
    let net = range.net();
    let conn = db.conn.lock().unwrap();
    let rows = queries::version_share_by_seed(&conn, from, to, net).map_err(err)?;
    Ok::<_, (StatusCode, String)>(Json(json!({ "rows": rows })))
}

async fn class_shares(State(db): State<Arc<Db>>, Query(range): Query<Range>) -> impl IntoResponse {
    let (from, to) = range.resolve(7 * 86400);
    let from_day = from / 86400;
    let to_day = to / 86400;
    let net = range.net();
    let conn = db.conn.lock().unwrap();
    let rows = queries::class_shares_range(&conn, from_day, to_day, net).map_err(err)?;
    Ok::<_, (StatusCode, String)>(Json(json!({ "rows": rows })))
}

async fn seed_overlap(State(db): State<Arc<Db>>, Query(range): Query<Range>) -> impl IntoResponse {
    let (from, to) = range.resolve(86400);
    let net = range.net();
    let conn = db.conn.lock().unwrap();
    let totals = queries::seed_totals(&conn, from, to, net).map_err(err)?;
    let pairs = queries::seed_overlap(&conn, from, to, net).map_err(err)?;
    Ok::<_, (StatusCode, String)>(Json(json!({ "totals": totals, "pairs": pairs })))
}

async fn alerts(State(db): State<Arc<Db>>) -> impl IntoResponse {
    let conn = db.conn.lock().unwrap();
    let rows = queries::alerts(&conn).map_err(err)?;
    Ok::<_, (StatusCode, String)>(Json(json!({ "alerts": rows })))
}

async fn fixed_seed_decay(State(db): State<Arc<Db>>, Query(range): Query<Range>) -> impl IntoResponse {
    let net = range.net();
    // 24h lookback for "currently reachable" (probes run every 6h, so 24h gives
    // each address ~4 chances to be probed).
    let conn = db.conn.lock().unwrap();
    let rows = queries::fixed_seed_decay(&conn, net, 86400).map_err(err)?;
    Ok::<_, (StatusCode, String)>(Json(json!({ "releases": rows })))
}
