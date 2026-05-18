CREATE TABLE fixed_seed_releases (
  tag         TEXT    NOT NULL,
  network     TEXT    NOT NULL,
  released_at INTEGER NOT NULL,
  ingested_at INTEGER NOT NULL,
  PRIMARY KEY (tag, network)
);

CREATE TABLE fixed_seeds (
  tag      TEXT    NOT NULL,
  network  TEXT    NOT NULL,
  address  TEXT    NOT NULL,
  port     INTEGER NOT NULL,
  PRIMARY KEY (tag, network, address),
  FOREIGN KEY (tag, network) REFERENCES fixed_seed_releases(tag, network)
);
CREATE INDEX fixed_seeds_net_addr ON fixed_seeds(network, address);

CREATE TABLE fixed_seed_probes (
  ts          INTEGER NOT NULL,
  network     TEXT    NOT NULL,
  address     TEXT    NOT NULL,
  port        INTEGER NOT NULL,
  success     INTEGER NOT NULL,
  latency_ms  INTEGER,
  user_agent  TEXT,
  error_kind  TEXT
);
CREATE INDEX fixed_seed_probes_ts        ON fixed_seed_probes(ts);
CREATE INDEX fixed_seed_probes_net_addr  ON fixed_seed_probes(network, address);
