CREATE TABLE alert_state (
  seed_id     INTEGER NOT NULL PRIMARY KEY REFERENCES seeds(id),
  level       TEXT    NOT NULL CHECK (level IN ('warning','critical')),
  reason      TEXT    NOT NULL,
  opened_at   INTEGER NOT NULL,
  notified_at INTEGER
);

CREATE TABLE alert_history (
  ts       INTEGER NOT NULL,
  seed_id  INTEGER NOT NULL,
  event    TEXT    NOT NULL CHECK (event IN ('opened','escalated','deescalated','resolved')),
  level    TEXT,
  reason   TEXT
);
CREATE INDEX alert_history_ts      ON alert_history(ts);
CREATE INDEX alert_history_seed_ts ON alert_history(seed_id, ts);
