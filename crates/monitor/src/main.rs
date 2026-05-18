use std::sync::Arc;

use anyhow::Context;
use tracing_subscriber::EnvFilter;

mod alerting;
mod api;
mod collector;
mod config;
mod db;
mod dns;
mod fixed_seeds;
mod p2p;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "monitor=info,tower_http=info".into()))
        .init();

    let cfg = config::Config::load().context("loading config")?;
    tracing::info!(?cfg.server.bind, db = %cfg.db.path.display(), "starting dns-seed-monitor");

    let db = db::Db::open(&cfg.db.path).context("opening db")?;
    db.seed_catalog(seeds::all())?;

    let db = Arc::new(db);

    if cfg.collector.enabled {
        let cdb = db.clone();
        let ccfg = cfg.collector.clone();
        let acfg = cfg.alerting.clone();
        tokio::spawn(async move {
            if let Err(e) = collector::run(cdb, ccfg, acfg).await {
                tracing::error!(error = ?e, "collector exited");
            }
        });

        // Fixed-seed decay monitor — every 6h.
        let fdb = db.clone();
        let fcfg = cfg.collector.clone();
        tokio::spawn(async move {
            let interval = std::time::Duration::from_secs(6 * 3600);
            if let Err(e) = fixed_seeds::run(fdb, interval, fcfg.user_agent.clone(), fcfg.probe_concurrency, fcfg.probe_timeout()).await {
                tracing::error!(error = ?e, "fixed-seed runner exited");
            }
        });
    }

    let app = api::router(db.clone());
    let listener = tokio::net::TcpListener::bind(cfg.server.bind).await?;
    tracing::info!(addr = %listener.local_addr()?, "http listening");
    axum::serve(listener, app).await?;
    Ok(())
}
