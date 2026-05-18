CREATE TABLE seeds (
  id            INTEGER PRIMARY KEY,
  hostname      TEXT NOT NULL UNIQUE,
  network       TEXT NOT NULL,
  filter_flags  INTEGER,
  operator      TEXT,
  added_at      INTEGER NOT NULL
);

CREATE TABLE dns_responses (
  ts        INTEGER NOT NULL,
  seed_id   INTEGER NOT NULL REFERENCES seeds(id),
  address   TEXT    NOT NULL,
  port      INTEGER NOT NULL
);
CREATE INDEX dns_responses_seed_ts ON dns_responses(seed_id, ts);

CREATE TABLE probes (
  ts          INTEGER NOT NULL,
  seed_id     INTEGER NOT NULL REFERENCES seeds(id),
  address     TEXT    NOT NULL,
  port        INTEGER NOT NULL,
  success     INTEGER NOT NULL,
  latency_ms  INTEGER,
  user_agent  TEXT,
  services    INTEGER,
  version     INTEGER,
  height      INTEGER,
  error_kind  TEXT
);
CREATE INDEX probes_ts      ON probes(ts);
CREATE INDEX probes_seed_ts ON probes(seed_id, ts);

CREATE TABLE daily_classifications (
  day      INTEGER NOT NULL,
  seed_id  INTEGER NOT NULL REFERENCES seeds(id),
  address  TEXT    NOT NULL,
  class    TEXT    NOT NULL CHECK (class IN
            ('pristine','stale','duplicate','reachable','unreachable')),
  PRIMARY KEY (day, seed_id, address)
);

CREATE TABLE probes_hourly (
  bucket       INTEGER NOT NULL,
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
