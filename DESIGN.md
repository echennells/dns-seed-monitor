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

## Scope

**In scope:**

- DNS seeds for **mainnet, testnet (testnet3), and signet**, mirrored from
  Bitcoin Core's `chainparams.cpp`.
- Per-seed reachability tracking, classification (pristine / stale /
  duplicate / reachable / unreachable), version-share, class-mix, overlap.
- **Alerting** — rule-based, computed each collector round, dispatched as
  JSON POSTs to a configurable webhook on state transitions.
- **Fixed-seed decay** — parse `contrib/seeds/nodes_*.txt` from each Core
  release tag, probe the clearnet entries every 6h, plot decay since
  release date. Addresses @0xB10C's follow-up comment on the issue.

**Out of scope (v1):**

- Tor `.onion` / I2P `.b32.i2p` / cjdns probing. Needs Tor SOCKS5 / I2P
  SAM bridge / cjdns mesh peer. DNS seeds can't carry these address types
  anyway (record format limit); they only matter for fixed-seed decay,
  where they're filtered out at ingest. v2.
- Crawling beyond what seeds advertise. We are not a node crawler.
- Service-bit-filtered seed variants (`x1.`, `x5.`, `x9.` subdomains). v2.

## Architecture

```
                  ┌──────────────────────────────────────────┐
                  │  dns-seed-monitor (single Rust binary)   │
                  │                                          │
   round tick ──► │  collector ──► sqlite (WAL)  ──┐         │
                  │   │                            │         │
                  │   ├─► hickory-resolver         │         │
                  │   ├─► p2p handshake (tcp)      │         │
                  │   └─► alerting ──► webhook ────┼─► HTTPS │
                  │                                │         │
   6h tick   ──►  │  fixed_seeds ──► sqlite ───────┤         │
                  │   ├─► fetch chainparams_seeds  │         │
                  │   └─► probe (re-uses handshake)│         │
                  │                                │         │
                  │                                ▼         │
                  │                       axum HTTP API ────►│ JSON
                  │                                          │
                  └──────────────────────────────────────────┘
                                                          ▲
                                                          │
                                          ┌───────────────┴──────────────┐
                                          │  static frontend             │
                                          │  (vanilla ES modules, uPlot, │
                                          │   served by axum ServeDir)   │
                                          └──────────────────────────────┘
```

One process. The collector, the fixed-seed runner, the alerting evaluator,
and the HTTP API share a single `Mutex<rusqlite::Connection>`. Writers are
the collector and fixed-seed runner; the API is read-only.

## Collection methodology

A **probing round** runs every hour, across every seed in the catalog:

1. **Resolve.** Query A and AAAA records via `hickory-resolver` for each
   seed, alternating between the two record types over `queries_per_seed`
   queries spread out by `query_spread_secs` for politeness. AAAA queries
   are skipped entirely when `enable_ipv6 = false` (default in repo
   config; flip to `true` on hosts that have v6 routing). Returned IPs
   are deduplicated within the round per `(network, ip)` pair.
2. **Dedupe within the round.** If the same `(network, ip)` came back
   from multiple seeds *on the same network* in this round, mark them
   `duplicate` for the day's classification.
3. **Probe.** For each unique `(network, ip)` target, open a TCP
   connection on the network's P2P port (mainnet 8333, testnet 18333,
   signet 38333) with a 10 s timeout, send a `version` message with the
   network's magic, await `version` + `verack`, record:
   - success / failure + `error_kind` (`connect_timeout`,
     `connect_refused`, `read_timeout`, `eof`, `bad_magic`, `decode`,
     `no_version`, `io`).
   - latency in ms.
   - peer's user agent, services bitfield, protocol version, start
     height (on success).

   Concurrency cap: `probe_concurrency` in-flight connections
   (`tokio::sync::Semaphore`) so we don't exhaust ephemeral ports.
4. **Classify** each `(seed, address)` for the current day into one of:
   - **pristine** — handshake succeeded; address was not in yesterday's
     classifications for this seed.
   - **stale** — handshake succeeded; address was already in
     yesterday's classifications for this seed.
   - **duplicate** — address advertised by another seed in the same
     network this round.
   - **reachable** — succeeded but not pristine/stale/duplicate (e.g.
     first day of data collection).
   - **unreachable** — handshake failed.

   Definitions intentionally match 21.ninja so historical comparison is
   possible.
5. **Roll up.** Same transaction: upsert `probes_hourly(bucket, seed_id)`
   counters.
6. **Evaluate alerts.** After the round commits, compute current alerts
   from `probes_hourly` (see Alerting below), diff against `alert_state`,
   write any state transitions to `alert_history`, and dispatch webhook
   events for each transition if `[alerting].enabled = true`.

## Bitcoin P2P handshake

Hand-rolled. We only need version exchange, not addr/inv/etc. Reuse the
`bitcoin` crate for message encoding/decoding. ~170 LOC.

Outline:

```rust
async fn handshake(addr: SocketAddr, magic: Magic, user_agent: &str, timeout: Duration)
    -> Result<PeerInfo, ProbeError>
{
    let stream = tokio::time::timeout(timeout, TcpStream::connect(addr)).await??;
    // Send our VERSION, then drain incoming messages until we see VERSION
    // (reply with VERACK) and then VERACK from them. Ignore anything else
    // (wtxidrelay / sendaddrv2 / etc. arrive before verack on modern Core).
    ...
}
```

Magic and port are determined by the seed's network. Both are encoded on
the `seeds::Network` enum.

## Fixed-seed decay

A second collection pipeline, separate from the DNS collector:

1. **Ingest.** At startup (and on demand later), for each Core release
   tag in the hardcoded `RELEASES` list, fetch
   `contrib/seeds/nodes_{main,test,signet}.txt` from
   `raw.githubusercontent.com/bitcoin/bitcoin/<tag>/...`. Parse
   `addr:port` per line; skip comments / blank lines; **filter out
   `.onion`, `.b32.i2p`, and `fc00::/8` cjdns entries** at this stage.
   Persist into `fixed_seed_releases` and `fixed_seeds` with `INSERT OR
   IGNORE` so re-runs are idempotent.
2. **Probe loop.** Every `probe_interval` (default 6h), take the
   distinct `(network, address, port)` tuples across all releases and
   probe each one, reusing the same handshake code as the DNS pipeline.
   Results land in `fixed_seed_probes`.
3. **Snapshot query.** `/api/fixed-seeds/decay?network=…` joins the
   tables and computes per-release `{total, probed_recent,
   reachable_recent}` using the most recent probe per address within a
   24h lookback.

## Alerting

Rule-based, evaluated each collector round. Rules currently:

- **critical** — `<10%` success rate over the last 24h with at least 5
  probes attempted. Effectively "this seed is dead."
- **warning — degraded** — `<50%` success rate over 24h with at least
  30 probes.
- **warning — stale** — last round older than 2 hours.
- **warning — no data** — no probes for this seed in the last 24h
  (typically newly-added seeds; will clear once probed).

State transitions are tracked in `alert_state`. Webhook events fire on
`opened`, `escalated`, `deescalated`, `resolved`. Disabled by default;
turn on by setting `webhook_url` and `enabled = true` in the
`[alerting]` section. Optional `auth_header` (`"Header-Name: value"`) is
forwarded.

The dashboard `#alerts` page reads the same rule output regardless of
the webhook setting.

## Configuration

```toml
[server]
bind = "0.0.0.0:8080"           # tighten to 127.0.0.1 behind a reverse proxy

[db]
path = "data/monitor.db"

[collector]
enabled             = true
round_interval_secs = 3600
queries_per_seed    = 20
query_spread_secs   = 300
probe_concurrency   = 256
probe_timeout_secs  = 10
user_agent          = "/DCTRL-YVR-seed-monitor:0.1.0/"
enable_ipv6         = false      # set true on hosts with v6 routing

[alerting]
enabled     = false
# webhook_url = "https://ntfy.sh/your-topic"
# auth_header = "Authorization: Bearer your-token"
```

The user agent follows BIP14 (`/Name:Version/`). Hyphens because spaces
in the name field break some peer parsers. Bump the version on releases
so node operators inspecting their inbound peers can identify which
build is probing them.

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

Migrations live in `crates/monitor/migrations/` and run via
`rusqlite_migration` at startup.

### Schema

**`001_initial.sql`** — DNS-seed collection tables.

```sql
CREATE TABLE seeds (
  id, hostname (UNIQUE), network, filter_flags, operator, added_at
);
CREATE TABLE dns_responses (ts, seed_id, address, port);
CREATE TABLE probes (
  ts, seed_id, address, port, success, latency_ms,
  user_agent, services, version, height, error_kind
);
CREATE TABLE daily_classifications (
  day, seed_id, address,
  class CHECK (class IN ('pristine','stale','duplicate','reachable','unreachable')),
  PRIMARY KEY (day, seed_id, address)
);
CREATE TABLE probes_hourly (bucket, seed_id, reachable, unreachable);
CREATE TABLE user_agents_daily (day, user_agent, count);
```

Note: `user_agents_daily` is no longer written to. The collector used to
populate it, but the API reads UAs directly from `probes` (per-network)
and the legacy table had no network column. Left in place for archeology;
the dead-write was removed when multi-network shipped.

**`002_alerting.sql`** — alerting state.

```sql
CREATE TABLE alert_state (
  seed_id PK, level CHECK ('warning','critical'), reason, opened_at, notified_at
);
CREATE TABLE alert_history (
  ts, seed_id, event CHECK ('opened','escalated','deescalated','resolved'),
  level, reason
);
```

**`003_fixed_seeds.sql`** — fixed-seed manifest + probes.

```sql
CREATE TABLE fixed_seed_releases (tag, network, released_at, ingested_at, PK (tag, network));
CREATE TABLE fixed_seeds (tag, network, address, port, PK (tag, network, address));
CREATE TABLE fixed_seed_probes (
  ts, network, address, port, success, latency_ms, user_agent, error_kind
);
```

### Sizing

- Hourly probing, ~17 seeds (10 + 5 + 2), ~2 k unique addresses/day →
  ~50 k `probes` rows/day ≈ ~5 MB/day raw. Well under 2 GB/year. No
  partitioning needed.
- Fixed-seed probes add ~4.5 k probes per 6h round → ~18 k/day raw.
- If we outgrow it, the schema ports cleanly to Postgres+TimescaleDB.

### Backups

- `VACUUM INTO data/backups/monitor-YYYYMMDD.db` once a day, retain 14.
- Optional `litestream` to S3/B2 for continuous replication if/when
  scale demands it.

## HTTP API

JSON, served by `axum` from the same binary. All read endpoints accept
`from` / `to` (unix seconds) and `network` (`mainnet` / `testnet` /
`signet` / `all`) query params. `network` defaults to `mainnet`.

| Method | Path                                | Returns                                                |
|--------|-------------------------------------|--------------------------------------------------------|
| GET    | `/api/seeds`                        | seed catalog + 24h latest summary                      |
| GET    | `/api/seeds/:id/timeseries`         | per-seed reachable/unreachable by hour                 |
| GET    | `/api/nodes/reachable`              | reachable counts split by ipv4/ipv6                    |
| GET    | `/api/nodes/unique`                 | deduped reachable address count per day                |
| GET    | `/api/user-agents`                  | top user agents over the range                         |
| GET    | `/api/version-share`                | per-(seed, Core major version) successful-handshake counts |
| GET    | `/api/class-shares`                 | per-(seed, classification) counts over the range       |
| GET    | `/api/seed-overlap`                 | pairwise shared-address counts + per-seed totals       |
| GET    | `/api/alerts`                       | current alert state across all networks                |
| GET    | `/api/fixed-seeds/decay`            | per-release reachability snapshot (24h lookback)       |
| GET    | `/healthz`                          | liveness                                               |

CORS is `CorsLayer::permissive()` — intentional, the API is public
read-only.

## Frontend

- **Vanilla ES modules**, no build step. `web/index.html` loads
  `app.js` and `style.css`; uPlot via CDN.
- **uPlot** for the few line charts we have. Heatmaps and class bars
  are plain HTML/CSS (no chart lib needed at the cell densities we
  show).
- Pages:
  - **Alerts** — current rule-fired alerts across all networks.
  - **DNS seeds** — roster table + per-seed reachable/unreachable
    time series.
  - **Reachable nodes** — split by ipv4/ipv6.
  - **Unique reachable** — distinct addresses per day.
  - **User agents** — top 30 over the range.
  - **Version share** — heatmap of Core major versions per seed.
  - **Classification mix** — per-seed stacked bars (pristine / reachable
    / stale / duplicate / unreachable).
  - **Seed overlap** — pairwise Jaccard heatmap.
  - **Fixed-seed decay** — per Core release tag.
  - **Methodology** — definitions and probe identity.
- Global controls in the sidebar:
  - **Network selector** (mainnet / testnet / signet) — drives the
    `?network=` query parameter on every API call and the rendered
    seed list on each page. Defaults to mainnet, persists to
    `localStorage`.
  - **Range selector** (7 d / 30 d / 90 d / 1 y).
  - **Theme chips** (warm dark / cool dark / neutral dark), also
    persisted; pre-paint inline script avoids the flash-of-wrong-palette.

## Repo layout

```
dns-seed-monitor/
├── Cargo.toml                                # workspace
├── crates/
│   ├── monitor/                              # the binary
│   │   ├── Cargo.toml
│   │   ├── migrations/
│   │   │   ├── 001_initial.sql
│   │   │   ├── 002_alerting.sql
│   │   │   └── 003_fixed_seeds.sql
│   │   └── src/
│   │       ├── main.rs
│   │       ├── alerting.rs
│   │       ├── api.rs
│   │       ├── collector.rs
│   │       ├── config.rs
│   │       ├── dns.rs
│   │       ├── fixed_seeds.rs
│   │       ├── p2p/{handshake.rs, mod.rs}
│   │       └── db/{mod.rs, queries.rs}
│   └── seeds/                                # static seed catalog
│       └── src/lib.rs
├── web/
│   ├── index.html
│   ├── app.js
│   └── style.css
├── docker/Dockerfile
├── docker-compose.yml
├── README.md
└── DESIGN.md                                 # this file
```

## Initial seed list (mainnet)

Mirrored from `bitcoin/src/kernel/chainparams.cpp` `vSeeds`, stored as a
static array in the `seeds` crate, seeded into the DB on first boot:

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

The mirror is *manual* — Core's list can drift. `dnsseed.bitcoin.dashjr-...`
was removed from Core in Dec 2025
([PR #33723](https://github.com/bitcoin/bitcoin/pull/33723)) for serving a
user-agent-filtered sample; we keep probing it deliberately so the
violation remains visible in the historical record. `seed.bitcoinstats.com`
is also missing from current `chainparams.cpp` as of May 2026; we still
probe it.

A weekly CI guard that fails on drift between `chainparams.cpp` and the
`seeds` crate is a candidate follow-up.

## Testnet seed list (testnet3)

- `testnet-seed.bitcoin.jonasschnelli.ch` — Jonas Schnelli
- `seed.tbtc.petertodd.net` — Peter Todd
- `seed.testnet.bitcoin.sprovoost.nl` — Sjors Provoost
- `testnet-seed.bluematt.me` — Matt Corallo
- `seed.testnet.achownodes.xyz` — Ava Chow

## Signet seed list

- `seed.signet.bitcoin.sprovoost.nl` — Sjors Provoost
- `seed.signet.achownodes.xyz` — Ava Chow

## v2 candidates

- Tor `.onion` and I2P `.b32.i2p` probing via Arti / I2P SAM. Needed
  for the fixed-seed decay pipeline to give a complete picture, since
  Core ships onion/i2p entries in `chainparams_seeds.h`.
- cjdns probing — needs a peer into the mesh.
- Service-bit-filtered seed variants (`x1.`, `x5.`, `x9.` subdomains)
  as separate logical seeds.
- Weekly CI guard that fails when the in-tree seed catalog drifts from
  Core's `chainparams.cpp` (`vSeeds` arrays).
- Public daily parquet exports for downstream consumers.
- Testnet4 coverage (currently testnet3 only).
