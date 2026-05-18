# dns-seed-monitor

Monitoring for Bitcoin DNS seeds, modeled on
[21.ninja/dns-seeds](https://21.ninja/dns-seeds/). One Rust binary that
resolves each seed (mainnet, testnet, signet), attempts a Bitcoin P2P
handshake against every returned address, classifies the result, stores
it in SQLite, and serves a small dashboard.

Built in response to
[0xB10C/project-ideas#14](https://github.com/0xB10C/project-ideas/issues/14).

Live: <https://monitor.yvrbtclabs.dev>

See [`DESIGN.md`](./DESIGN.md) for the full spec.

## What it tracks

- **Per-seed reachability** — success / unreachable counts per round,
  rolled up hourly and daily.
- **Classification** — pristine / stale / duplicate / reachable /
  unreachable, matching 21.ninja's vocabulary.
- **Version share** — heatmap of Core major versions per seed (this is
  what makes user-agent filtering, like the
  [dashjr seed removal](https://github.com/bitcoin/bitcoin/pull/33723),
  visually obvious).
- **Seed overlap** — pairwise Jaccard similarity of address sets.
- **Multi-network** — mainnet (10 seeds), testnet3 (5), signet (2),
  each filtered independently via a sidebar selector.
- **Alerts** — critical / warning rules computed each round, optional
  webhook dispatch (`opened` / `escalated` / `deescalated` / `resolved`
  events as JSON POSTs).
- **Fixed-seed decay** — per Core release tag, what fraction of the
  compiled-in `chainparams_seeds.h` entries are still reachable.
  Probed every 6 hours. Clearnet (IPv4/IPv6) only; onion/i2p/cjdns
  excluded at ingest.

## Run

```sh
cargo run --release
# open http://localhost:8080
```

Config lives in `config.toml`. Override with environment variables prefixed
`MONITOR_`, e.g. `MONITOR_COLLECTOR__ROUND_INTERVAL_SECS=60` or
`MONITOR_ALERTING__WEBHOOK_URL=https://ntfy.sh/your-topic`.

## Layout

```
crates/monitor   the binary (collector + alerting + fixed-seed monitor + axum API)
crates/seeds     static seed catalog (mirrored from chainparams.cpp)
web/             static frontend (vanilla JS modules + uPlot, no build step)
```

## Probing identity

Probes identify themselves as `/DCTRL-YVR-seed-monitor:0.1.0/` (BIP14).
Node operators can identify and contact us via that user agent.
