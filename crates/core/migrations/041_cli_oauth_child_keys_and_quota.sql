CREATE TABLE IF NOT EXISTS cli_child_keys (
  child_key_id TEXT PRIMARY KEY,
  owner_key_id TEXT NOT NULL,
  cli_instance_uuid TEXT NOT NULL UNIQUE,
  status TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  last_seen_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_cli_child_keys_owner_key_id
  ON cli_child_keys(owner_key_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_cli_child_keys_last_seen_at
  ON cli_child_keys(last_seen_at DESC);

CREATE TABLE IF NOT EXISTS cli_oauth_sessions (
  session_id TEXT PRIMARY KEY,
  child_key_id TEXT NOT NULL,
  owner_key_id TEXT NOT NULL,
  cli_instance_uuid TEXT NOT NULL,
  client_id TEXT NOT NULL,
  redirect_uri TEXT NOT NULL,
  pkce_challenge TEXT NOT NULL,
  pkce_method TEXT NOT NULL,
  state TEXT NOT NULL,
  authorization_code_hash TEXT,
  refresh_token_hash TEXT NOT NULL,
  status TEXT NOT NULL,
  id_token TEXT NOT NULL,
  expires_at INTEGER NOT NULL,
  refresh_expires_at INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  last_seen_at INTEGER NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_cli_oauth_sessions_authorization_code_hash
  ON cli_oauth_sessions(authorization_code_hash);

CREATE UNIQUE INDEX IF NOT EXISTS idx_cli_oauth_sessions_refresh_token_hash
  ON cli_oauth_sessions(refresh_token_hash);

CREATE INDEX IF NOT EXISTS idx_cli_oauth_sessions_child_key_id
  ON cli_oauth_sessions(child_key_id, updated_at DESC);

CREATE TABLE IF NOT EXISTS account_quota_exhaustion (
  account_id TEXT PRIMARY KEY,
  reason TEXT NOT NULL,
  exhausted_until INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_account_quota_exhaustion_until
  ON account_quota_exhaustion(exhausted_until DESC);

ALTER TABLE request_logs ADD COLUMN owner_key_id TEXT;

CREATE INDEX IF NOT EXISTS idx_request_logs_owner_key_id_created_at
  ON request_logs(owner_key_id, created_at DESC);

ALTER TABLE request_token_stats ADD COLUMN owner_key_id TEXT;

CREATE INDEX IF NOT EXISTS idx_request_token_stats_owner_key_id_created_at
  ON request_token_stats(owner_key_id, created_at DESC);
