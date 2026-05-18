use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use figment::providers::{Env, Format, Toml};
use figment::Figment;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub db: DbConfig,
    pub collector: CollectorConfig,
    #[serde(default)]
    pub alerting: AlertingConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub bind: SocketAddr,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DbConfig {
    pub path: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CollectorConfig {
    pub enabled: bool,
    pub round_interval_secs: u64,
    pub queries_per_seed: u32,
    pub query_spread_secs: u64,
    pub probe_concurrency: usize,
    pub probe_timeout_secs: u64,
    pub user_agent: String,
    #[serde(default = "default_true")]
    pub enable_ipv6: bool,
}

fn default_true() -> bool {
    true
}

impl CollectorConfig {
    pub fn round_interval(&self) -> Duration {
        Duration::from_secs(self.round_interval_secs)
    }
    pub fn probe_timeout(&self) -> Duration {
        Duration::from_secs(self.probe_timeout_secs)
    }
    pub fn query_spread(&self) -> Duration {
        Duration::from_secs(self.query_spread_secs)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AlertingConfig {
    #[serde(default)]
    pub enabled: bool,
    pub webhook_url: Option<String>,
    pub auth_header: Option<String>,
}

impl Config {
    #[allow(clippy::result_large_err)] // figment::Error is large; boxing this one call site isn't worth the churn
    pub fn load() -> Result<Self, figment::Error> {
        let path = std::env::var("MONITOR_CONFIG").unwrap_or_else(|_| "config.toml".into());
        Figment::new()
            .merge(Toml::file(path))
            .merge(Env::prefixed("MONITOR_").split("__"))
            .extract()
    }
}
