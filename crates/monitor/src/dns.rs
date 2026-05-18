use std::collections::HashSet;
use std::net::IpAddr;
use std::time::Duration;

use anyhow::Result;
use hickory_resolver::config::{ResolverConfig, ResolverOpts};
use hickory_resolver::TokioAsyncResolver;
use rand::Rng;

pub struct DnsClient {
    resolver: TokioAsyncResolver,
    enable_ipv6: bool,
}

impl DnsClient {
    pub fn system(enable_ipv6: bool) -> Result<Self> {
        // Cloudflare + Google as defaults; many CI hosts have no /etc/resolv.conf
        // we can fully rely on, so go straight to public resolvers.
        let mut opts = ResolverOpts::default();
        opts.timeout = Duration::from_secs(5);
        opts.attempts = 2;
        let resolver = TokioAsyncResolver::tokio(ResolverConfig::cloudflare(), opts);
        Ok(Self { resolver, enable_ipv6 })
    }

    /// Run `n` queries against the seed (alternating A / AAAA when v6 is enabled,
    /// otherwise A only), spread out over `spread`, return the deduplicated set
    /// of addresses.
    pub async fn sample_seed(&self, hostname: &str, n: u32, spread: Duration) -> HashSet<IpAddr> {
        let mut out = HashSet::new();
        let per = if n == 0 { spread } else { spread / n };
        for i in 0..n {
            let query_v6 = self.enable_ipv6 && i % 2 == 1;
            let res = if query_v6 {
                self.resolver
                    .ipv6_lookup(hostname)
                    .await
                    .map(|r| r.into_iter().map(|a| IpAddr::V6(a.0)).collect::<Vec<_>>())
            } else {
                self.resolver
                    .ipv4_lookup(hostname)
                    .await
                    .map(|r| r.into_iter().map(|a| IpAddr::V4(a.0)).collect::<Vec<_>>())
            };
            match res {
                Ok(addrs) => {
                    for a in addrs {
                        out.insert(a);
                    }
                }
                Err(e) => {
                    tracing::warn!(seed = hostname, error = %e, "dns query failed");
                }
            }
            if i + 1 < n {
                let jitter_ms = rand::thread_rng().gen_range(0..250);
                tokio::time::sleep(per + Duration::from_millis(jitter_ms)).await;
            }
        }
        out
    }
}
