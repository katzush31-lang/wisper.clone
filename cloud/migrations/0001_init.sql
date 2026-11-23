CREATE TABLE users (
  id TEXT PRIMARY KEY,
  google_sub TEXT UNIQUE NOT NULL,
  email TEXT NOT NULL,
  name TEXT,
  created_at INTEGER NOT NULL
);

CREATE TABLE devices (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL,
  name TEXT NOT NULL,
  refresh_hash TEXT NOT NULL,
  prev_refresh_hash TEXT,
  created_at INTEGER,
  last_seen INTEGER,
  revoked INTEGER DEFAULT 0
);
CREATE INDEX idx_devices_user ON devices(user_id);
CREATE INDEX idx_devices_refresh ON devices(refresh_hash);
CREATE INDEX idx_devices_prev_refresh ON devices(prev_refresh_hash);

CREATE TABLE login_requests (
  id TEXT PRIMARY KEY,
  challenge TEXT NOT NULL,
  port INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  code_hash TEXT,
  code_expires INTEGER,
  user_id TEXT,
  used INTEGER DEFAULT 0
);
CREATE INDEX idx_login_requests_code ON login_requests(code_hash);

CREATE TABLE api_keys (
  user_id TEXT,
  provider TEXT,
  ciphertext TEXT,
  iv TEXT,
  key_version INTEGER,
  updated_at INTEGER,
  PRIMARY KEY (user_id, provider)
);

CREATE TABLE settings (
  user_id TEXT,
  section TEXT,
  value TEXT NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (user_id, section)
);

CREATE TABLE items (
  user_id TEXT,
  kind TEXT,
  uid TEXT,
  data TEXT,
  updated_at INTEGER NOT NULL,
  deleted INTEGER DEFAULT 0,
  PRIMARY KEY (user_id, kind, uid)
);
CREATE INDEX idx_items_user_updated ON items(user_id, updated_at);

CREATE TABLE stats (
  user_id TEXT,
  uid TEXT,
  device_id TEXT,
  created_at INTEGER,
  words INTEGER,
  duration_ms INTEGER,
  app TEXT,
  fixes INTEGER,
  PRIMARY KEY (user_id, uid)
);
CREATE INDEX idx_stats_user_created ON stats(user_id, created_at);
