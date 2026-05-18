# Bitcoin DNS Seed Monitor — Design Doc

Inspired by [21.ninja/dns-seeds/](https://21.ninja/dns-seeds/) (@virtu), which stopped
updating in late 2024 and has no public source. Tracks
[0xB10C/project-ideas#14](https://github.com/0xB10C/project-ideas/issues/14).

## Goals

1. Detect when a Bitcoin DNS seed goes down, returns stale data, or otherwise
   malfunctions.
2. Publish historical metrics so operators and the community can audit seed
   behavior over time.
3. Be cheap and boring to run: one binary, one SQLite file, one static
   frontend.

## Non-goals (v1)

- Tor / I2P probing. Deferred — needs an Arti or SOCKS proxy and adds
  significant probe latency. Add in v2.
- Testnet / signet coverage. Schema supports it via `seeds.network`, but the
  initial seed list and UI ship mainnet-only.
- Crawling beyond what seeds advertise. We are not a node crawler.
- Alerting / paging. Static charts only in v1.

## Architecture

```
                  ┌──────────────────────────────┐
                  │  dns-seed-monitor (Rust bin) │
                  │                              │
   cron tick ───► │  collector ──► sqlite (WAL)  │ ◄── /api/* (axum)
                  │     │                        │
                  │     ├─► hickory-resolver     │
                  │     └─► p2p handshake (tcp)  │
                  └──────────────────────────────┘
                                                       ▲
                                                       │ JSON
                                                       │
                                            ┌──────────┴───────────┐
                                            │  SvelteKit frontend  │
                                            │  (static, uPlot)     │
                                            └──────────────────────┘
```

One process. The collector and HTTP API share a `rusqlite::Connection` pool
(or `sqlx::SqlitePool`). The collector is the only writer; the API is
read-only. Frontend is a static build served either by the same Rust binary
under `/` or by any static host (Cloudflare Pages, S3+CF).

## Collection methodology

A **probing round** runs every hour, per seed:

1. **Resolve.** Query A and AAAA records via `hickory-resolver`. Some seeds
   support service-bit prefixes (`x1.`, `x5.`, `x9.dnsseed.*`) — resolve the
   base hostname for v1. Record every returned `(ip, port=8333)` pair into
   `dns_responses`.
2. **Dedupe within the round.** If the same `(ip, port)` came back from
   multiple seeds in this round, mark all but the first occurrence as
   `duplicate` for the round's daily classification.
3. **Probe.** For each unique address: open a TCP connection on port 8333
   with a 10s timeout, send a `version` message, await the peer's `version`
   and `verack`, record:
   - success / failure + `error_kind` (timeout, refused, reset, bad magic,
     bad version, etc.)
   - latency in ms
   - peer's user agent, services bitfield, protocol version, start height.

   Concurrency cap: 256 in-flight connections (`tokio::sync::Semaphore`) so
   we don't exhaust ephemeral ports.
4. **Classify** each `(seed, address)` for the current day into one of:
   - **pristine** — connection succeeded, and this address was not in
     yesterday's `dns_responses` for this seed.
   - **stale** — connection succeeded, but this address was already in
     yesterday's `dns_responses` for this seed.
   - **duplicate** — same address advertised by another seed earlier in
     today's rounds.
   - **reachable** — succeeded, but not pristine/stale/duplicate (e.g. first
     day of data collection).
   - **unreachable** — handshake failed.

   Definitions intentionally match 21.ninja so historical comparison is
   possible.
5. **Roll up.** Same transaction: upsert `probes_hourly(bucket, seed_id)`
   counters.

## Bitcoin P2P handshake

Hand-rolled. We only need version exchange, not addr/inv/etc. Reuse the
`bitcoin` crate for message encoding/decoding (`bitcoin::p2p::message`,
`bitcoin::p2p::message_network::VersionMessage`). About 150–250 LOC.

Outline:

```rust
async fn handshake(addr: SocketAddr, magic: Magic, timeout: Duration)
    -> Result<PeerInfo, ProbeError>
{
    let stream = tokio::time::timeout(timeout, TcpStream::connect(addr)).await??;
    let mut stream = BufStream::new(stream);

    write_msg(&mut stream, magic, our_version_message(addr)).await?;

    let their_version = read_until::<VersionMessage>(&mut stream, magic, timeout).await?;
    let _verack = read_until::<()>(&mut stream, magic, timeout).await?;

    write_msg(&mut stream, magic, VerAck).await?;

    Ok(PeerInfo {
        user_agent: their_version.user_agent,
        services:   their_version.services.to_u64(),
        version:    their_version.version,
        height:     their_version.start_height,
    })
}
```

Magic is mainnet only in v1. Ignore any non-version/verack messages received
before verack (some peers send `wtxidrelay` / `sendaddrv2` early — drain
them).

## Configuration

```toml
[collector]
round_interval_secs = 3600
queries_per_seed    = 20
query_spread_secs   = 300
probe_concurrency   = 256
probe_timeout_secs  = 10
user_agent          = "/DCTRL-YVR-seed-monitor:0.1.0/"
```

The user agent follows BIP14 (`/Name:Version/`). Hyphens because spaces in
the name field break some peer parsers. Bump the version on releases so node
operators inspecting their inbound peers can identify which build is probing
them.

## Database (SQLite)

Single file at `./data/monitor.db`. Open with:

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous  = NORMAL;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;
PRAGMA temp_store   = MEMORY;
PRAGMA mmap_size    = 268435456;
```

### Schema

```sql
CREATE TABLE seeds (
  id            INTEGER PRIMARY KEY,
  hostname      TEXT NOT NULL UNIQUE,
  network       TEXT NOT NULL,                -- mainnet|testnet|signet
  filter_flags  INTEGER,                      -- service-bit prefix, NULL = base
  operator      TEXT,
  added_at      INTEGER NOT NULL
);

CREATE TABLE dns_responses (
  ts        INTEGER NOT NULL,                 -- unix seconds
  seed_id   INTEGER NOT NULL REFERENCES seeds(id),
  address   TEXT    NOT NULL,                 -- canonical: lowercase, [::] for v6
  port      INTEGER NOT NULL
);
CREATE INDEX dns_responses_seed_ts ON dns_responses(seed_id, ts);
CREATE INDEX dns_responses_day_seed ON dns_responses(seed_id, ts);

CREATE TABLE probes (
  ts          INTEGER NOT NULL,
  seed_id     INTEGER NOT NULL REFERENCES seeds(id),
  address     TEXT    NOT NULL,
  port        INTEGER NOT NULL,
  success     INTEGER NOT NULL,               -- 0|1
  latency_ms  INTEGER,
  user_agent  TEXT,
  services    INTEGER,
  version     INTEGER,
  height      INTEGER,
  error_kind  TEXT
);
CREATE INDEX probes_ts        ON probes(ts);
CREATE INDEX probes_seed_ts   ON probes(seed_id, ts);

CREATE TABLE daily_classifications (
  day      INTEGER NOT NULL,                  -- unix day = ts / 86400
  seed_id  INTEGER NOT NULL REFERENCES seeds(id),
  address  TEXT    NOT NULL,
  class    TEXT    NOT NULL CHECK (class IN
            ('pristine','stale','duplicate','reachable','unreachable')),
  PRIMARY KEY (day, seed_id, address)
);

CREATE TABLE probes_hourly (
  bucket       INTEGER NOT NULL,              -- unix hour
  seed_id      INTEGER NOT NULL REFERENCES seeds(id),
  reachable    INTEGER NOT NULL DEFAULT 0,
  unreachable  INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (bucket, seed_id)
);

CREATE TABLE user_agents_daily (
  day        INTEGER NOT NULL,
  user_agent TEXT    NOT NULL,
  count      INTEGER NOT NULL,
  PRIMARY KEY (day, user_agent)
);
```

Migrations live in `migrations/` and run via `rusqlite_migration` at startup.

### Sizing

- Hourly probing, ~15 seeds, ~2k unique addresses/day → ~50k `probes` rows/day
  ≈ ~5 MB/day raw. Well under 2 GB/year. No partitioning needed.
- If we outgrow it, the schema ports cleanly to Postgres+TimescaleDB: `probes`
  becomes a hypertable on `ts`, `probes_hourly` becomes a continuous aggregate,
  and the inline rollup writes get deleted.

### Backups

- `VACUUM INTO data/backups/monitor-YYYYMMDD.db` once a day, retain 14.
- Optional `litestream` to S3/B2 for continuous replication if/when hosted.

## HTTP API

JSON, served by `axum`. All endpoints accept `from` and `to` query params as
unix seconds; defaults to last 30 days.

| Method | Path                                    | Returns                                  |
|--------|-----------------------------------------|------------------------------------------|
| GET    | `/api/seeds`                            | seed catalog + latest probe summary      |
| GET    | `/api/seeds/:id/timeseries?metric=`     | series for one seed (class shares, etc.) |
| GET    | `/api/nodes/reachable?network=`         | reachable counts split by ipv4/ipv6/…    |
| GET    | `/api/nodes/unique`                     | deduped reachable node trend             |
| GET    | `/api/user-agents`                      | top user agents over the range           |
| GET    | `/api/methodology`                      | static MDX/HTML for the methodology page |
| GET    | `/healthz`                              | liveness                                 |

Cache responses for 60s in-process (most queries are aggregates).

## Frontend

- **SvelteKit** in static-adapter mode. Tiny bundle, cheap hosting.
- **uPlot** for time-series charts (hundreds of thousands of points without
  jank). Tables are plain HTML.
- Pages mirror 21.ninja's nav:
  - **DNS seeds** — per-seed advertised counts; class share stacked area;
    version stack; sortable table of all seeds with last-seen + latest
    success rate.
  - **Reachable nodes** — network-type split (IPv4/IPv6/Tor*/I2P*),
    service-flag changes, long-term persistence.
  - **Unique reachable nodes** — dedup counts and trend.
  - **Methodology** — static MDX documenting the definitions above.
- Shared range selector (7d / 30d / 90d / 1y / all) lifted to a Svelte store.
- Tor/I2P chips render as "deferred" in v1 with a tooltip.

## Repo layout

```
dns-seed-monitor/
├── Cargo.toml                # workspace
├── crates/
│   ├── monitor/              # the binary
│   │   ├── src/
│   │   │   ├── main.rs
│   │   │   ├── collector/    # tick loop, classification
│   │   │   ├── p2p/          # handshake
│   │   │   ├── dns/          # resolver wrapper
│   │   │   ├── db/           # sqlite pool, queries
│   │   │   ├── api/          # axum routes
│   │   │   └── config.rs
│   │   └── migrations/
│   └── seeds/                # static seed list, parsed from chainparams
│       └── src/lib.rs
├── web/                      # sveltekit app
│   ├── src/routes/
│   ├── src/lib/charts/
│   └── package.json
├── docker/Dockerfile
├── docker-compose.yml
└── DESIGN.md                 # this file
```

## Initial seed list (mainnet)

Pulled verbatim from `bitcoin/src/kernel/chainparams.cpp` `vSeeds`. Stored as
a static array in the `seeds` crate, seeded into the DB on first boot:

- `seed.bitcoin.sipa.be` — Pieter Wuille
- `dnsseed.bluematt.me` — Matt Corallo
- `dnsseed.bitcoin.dashjr-list-of-p2p-nodes.us` — Luke Dashjr
- `seed.bitcoinstats.com` — Christian Decker
- `seed.bitcoin.jonasschnelli.ch` — Jonas Schnelli
- `seed.btc.petertodd.net` — Peter Todd
- `seed.bitcoin.sprovoost.nl` — Sjors Provoost
- `dnsseed.emzy.de` — Stephan Oeste
- `seed.bitcoin.wiz.biz` — Jason Maurice
- `seed.mainnet.achownodes.xyz` — Ava Chow

Add community seeds (`seed.21.ninja`, etc.) as discovered.

## Milestones / first PRs

1. **Bootstrap** (PR 1) — workspace, crate skeletons, CI (`cargo fmt`,
   `clippy`, `test`), Dockerfile, `seeds` crate populated.
2. **DB + migrations** (PR 2) — schema above, `rusqlite_migration` wiring,
   pool, integration tests against `:memory:`.
3. **DNS + handshake** (PR 3) — `dns::resolve_seed`, `p2p::handshake` with
   unit tests against a local mock peer (`tokio::net::TcpListener` that
   speaks just enough version/verack).
4. **Collector** (PR 4) — round runner, classification, rollups. Cron via
   `tokio::time::interval`. Integration test that drives one synthetic round
   end-to-end against `:memory:`.
5. **HTTP API** (PR 5) — axum routes, response caching, OpenAPI schema.
6. **Frontend skeleton** (PR 6) — SvelteKit app, range selector, one chart
   page wired to live API.
7. **Remaining pages + methodology** (PR 7).
8. **Deploy** (PR 8) — compose file, Litestream config, healthcheck.

## Open questions

- Do we host this ourselves, or hand it to a community operator after v1?
- Do we publish the raw SQLite snapshot (or daily parquet exports) for
  reproducibility, the way 21.ninja published CSVs?
- Do we want to add a JSON dump endpoint for `bitnodes.io`-style downstream
  consumers?

## v2 candidates

- Tor `.onion` and I2P `.b32.i2p` probing via Arti / SAM.
- Testnet + signet coverage.
- Service-bit-filtered seed variants (`x1.`, `x5.`, `x9.`) as separate
  logical seeds.
- Alerting (Prometheus exporter or webhook on N consecutive failures).
- Public, downloadable daily parquet exports.
