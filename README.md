# dns-seed-monitor

Monitoring for Bitcoin DNS seeds, modeled on
[21.ninja/dns-seeds](https://21.ninja/dns-seeds/). One Rust binary that
resolves each mainnet seed, attempts a Bitcoin P2P handshake against every
returned address, classifies the result, stores it in SQLite, and serves a
small dashboard.

See [`DESIGN.md`](./DESIGN.md) for the full spec.

## Run

```sh
cargo run --release
# open http://localhost:8080
```

Config lives in `config.toml`. Override with environment variables prefixed
`MONITOR_`, e.g. `MONITOR_COLLECTOR__ROUND_INTERVAL_SECS=60`.

## Layout

```
crates/monitor   the binary (collector + axum API)
crates/seeds     static seed catalog
web/             static frontend (uPlot, no build step)
```

## Probing identity

Probes identify themselves as `/DCTRL-YVR-seed-monitor:0.1.0/` (BIP14).
Node operators can identify and contact us via that user agent.
