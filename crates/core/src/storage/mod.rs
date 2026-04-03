use rusqlite::{Connection, Result};
use std::path::Path;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

mod account_metadata;
mod accounts;
mod aggregate_apis;
mod api_keys;
mod cli_oauth;
mod conversation_affinity;
mod conversation_bindings;
mod events;
mod model_options;
mod request_log_query;
mod request_logs;
mod request_token_stats;
mod settings;
mod tokens;
mod usage;

#[derive(Debug, Clone)]
pub struct Account {
    pub id: String,
    pub label: String,
    pub issuer: String,
    pub chatgpt_account_id: Option<String>,
    pub workspace_id: Option<String>,
    pub group_name: Option<String>,
    pub sort: i64,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct AccountMetadata {
    pub account_id: String,
    pub note: Option<String>,
    pub tags: Option<String>,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub account_id: String,
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
    pub api_key_access_token: Option<String>,
    pub last_refresh: i64,
}

#[derive(Debug, Clone)]
pub struct LoginSession {
    pub login_id: String,
    pub code_verifier: String,
    pub state: String,
    pub status: String,
    pub error: Option<String>,
    pub workspace_id: Option<String>,
    pub note: Option<String>,
    pub tags: Option<String>,
    pub group_name: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct UsageSnapshotRecord {
    pub account_id: String,
    pub used_percent: Option<f64>,
    pub window_minutes: Option<i64>,
    pub resets_at: Option<i64>,
    pub secondary_used_percent: Option<f64>,
    pub secondary_window_minutes: Option<i64>,
    pub secondary_resets_at: Option<i64>,
    pub credits_json: Option<String>,
    pub captured_at: i64,
}

#[derive(Debug, Clone)]
pub struct Event {
    pub account_id: Option<String>,
    pub event_type: String,
    pub message: String,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct ConversationBinding {
    pub platform_key_hash: String,
    pub conversation_id: String,
    pub account_id: String,
    pub thread_epoch: i64,
    pub thread_anchor: String,
    pub status: String,
    pub last_model: Option<String>,
    pub last_switch_reason: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_used_at: i64,
}

#[derive(Debug, Clone)]
pub struct ClientBinding {
    pub platform_key_hash: String,
    pub affinity_key: String,
    pub account_id: String,
    pub primary_scope_id: Option<String>,
    pub binding_version: i64,
    pub status: String,
    pub last_supply_score: Option<f64>,
    pub last_pressure_score: Option<f64>,
    pub last_final_score: Option<f64>,
    pub last_switch_reason: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_seen_at: i64,
}

#[derive(Debug, Clone)]
pub struct ConversationThread {
    pub platform_key_hash: String,
    pub affinity_key: String,
    pub conversation_scope_id: String,
    pub account_id: String,
    pub thread_epoch: i64,
    pub thread_anchor: String,
    pub thread_version: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_seen_at: i64,
}

#[derive(Debug, Clone)]
pub struct ConversationContextState {
    pub platform_key_hash: String,
    pub affinity_key: String,
    pub conversation_scope_id: String,
    pub model: Option<String>,
    pub instructions_text: Option<String>,
    pub tools_json: Option<String>,
    pub tool_choice_json: Option<String>,
    pub parallel_tool_calls: Option<bool>,
    pub reasoning_json: Option<String>,
    pub text_format_json: Option<String>,
    pub service_tier: Option<String>,
    pub metadata_json: Option<String>,
    pub encrypted_content: Option<String>,
    pub protocol_type: Option<String>,
    pub response_adapter: Option<String>,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct ConversationContextEvent {
    pub platform_key_hash: String,
    pub affinity_key: String,
    pub conversation_scope_id: String,
    pub turn_index: i64,
    pub item_seq: i64,
    pub role: Option<String>,
    pub pair_group_id: Option<String>,
    pub capture_complete: bool,
    pub item_json: String,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct ContextSnapshot {
    pub platform_key_hash: String,
    pub affinity_key: String,
    pub conversation_scope_id: String,
    pub upto_turn_index: i64,
    pub summary_text: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct AffinityScopePromotion {
    pub platform_key_hash: String,
    pub affinity_key: String,
    pub from_scope_id: String,
    pub to_scope_id: String,
}

#[derive(Debug, Clone)]
pub struct AffinityKeyMigration {
    pub platform_key_hash: String,
    pub from_affinity_key: String,
    pub to_affinity_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AffinityTurnCommitOutcome {
    Committed,
    Conflict,
    MigrationConflict,
}

#[derive(Debug, Clone, Default)]
pub struct RequestLog {
    pub trace_id: Option<String>,
    pub key_id: Option<String>,
    pub owner_key_id: Option<String>,
    pub account_id: Option<String>,
    pub initial_account_id: Option<String>,
    pub attempted_account_ids_json: Option<String>,
    pub initial_aggregate_api_id: Option<String>,
    pub attempted_aggregate_api_ids_json: Option<String>,
    pub request_path: String,
    pub original_path: Option<String>,
    pub adapted_path: Option<String>,
    pub method: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub response_adapter: Option<String>,
    pub upstream_url: Option<String>,
    pub aggregate_api_supplier_name: Option<String>,
    pub aggregate_api_url: Option<String>,
    pub status_code: Option<i64>,
    pub duration_ms: Option<i64>,
    pub input_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub reasoning_output_tokens: Option<i64>,
    pub estimated_cost_usd: Option<f64>,
    pub error: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Default)]
pub struct RequestTokenStat {
    pub request_log_id: i64,
    pub key_id: Option<String>,
    pub owner_key_id: Option<String>,
    pub account_id: Option<String>,
    pub model: Option<String>,
    pub input_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub reasoning_output_tokens: Option<i64>,
    pub estimated_cost_usd: Option<f64>,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct RequestLogTodaySummary {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub estimated_cost_usd: f64,
}

#[derive(Debug, Clone)]
pub struct RequestLogQuerySummary {
    pub count: i64,
    pub success_count: i64,
    pub error_count: i64,
    pub total_tokens: i64,
}

#[derive(Debug, Clone)]
pub struct ApiKeyTokenUsageSummary {
    pub key_id: String,
    pub total_tokens: i64,
}

#[derive(Debug, Clone)]
pub struct CliChildKey {
    pub child_key_id: String,
    pub owner_key_id: String,
    pub cli_instance_uuid: String,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_seen_at: i64,
}

#[derive(Debug, Clone)]
pub struct CliOAuthSession {
    pub session_id: String,
    pub child_key_id: String,
    pub owner_key_id: String,
    pub cli_instance_uuid: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub pkce_challenge: String,
    pub pkce_method: String,
    pub state: String,
    pub authorization_code_hash: Option<String>,
    pub refresh_token_hash: String,
    pub status: String,
    pub id_token: String,
    pub expires_at: i64,
    pub refresh_expires_at: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_seen_at: i64,
}

#[derive(Debug, Clone)]
pub struct ApiKeyOwnerContext {
    pub owner_key_id: String,
    pub cli_instance_uuid: String,
}

#[derive(Debug, Clone)]
pub struct AccountQuotaExhaustion {
    pub account_id: String,
    pub reason: String,
    pub exhausted_until: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct ApiKey {
    pub id: String,
    pub name: Option<String>,
    pub model_slug: Option<String>,
    pub reasoning_effort: Option<String>,
    pub service_tier: Option<String>,
    pub rotation_strategy: String,
    pub aggregate_api_id: Option<String>,
    pub aggregate_api_url: Option<String>,
    pub client_type: String,
    pub protocol_type: String,
    pub auth_scheme: String,
    pub upstream_base_url: Option<String>,
    pub static_headers_json: Option<String>,
    pub key_hash: String,
    pub status: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct AggregateApi {
    pub id: String,
    pub provider_type: String,
    pub supplier_name: Option<String>,
    pub sort: i64,
    pub url: String,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_test_at: Option<i64>,
    pub last_test_status: Option<String>,
    pub last_test_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ModelOptionsCacheRecord {
    pub scope: String,
    pub items_json: String,
    pub updated_at: i64,
}

#[derive(Debug)]
pub struct Storage {
    conn: Connection,
}

impl Storage {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        // 中文注释：并发写入时给 SQLite 一点等待时间，避免瞬时 lock 导致请求直接失败。
        conn.busy_timeout(Duration::from_millis(3000))?;
        // 中文注释：文件库启用 WAL + NORMAL，可明显降低并发读写互斥开销；
        // 仅在 open(path) 上设置，避免影响 open_in_memory 的行为预期。
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;",
        )?;
        Ok(Self { conn })
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.busy_timeout(Duration::from_millis(3000))?;
        Ok(Self { conn })
    }

    pub fn init(&self) -> Result<()> {
        self.ensure_migrations_table()?;

        self.apply_sql_migration("001_init", include_str!("../../migrations/001_init.sql"))?;
        self.apply_sql_migration(
            "002_login_sessions",
            include_str!("../../migrations/002_login_sessions.sql"),
        )?;
        self.apply_sql_migration(
            "003_api_keys",
            include_str!("../../migrations/003_api_keys.sql"),
        )?;
        self.apply_sql_or_compat_migration(
            "004_api_key_model",
            include_str!("../../migrations/004_api_key_model.sql"),
            |s| s.ensure_api_key_model_column(),
        )?;
        self.apply_sql_or_compat_migration(
            "005_request_logs",
            include_str!("../../migrations/005_request_logs.sql"),
            |s| s.ensure_request_logs_table(),
        )?;
        self.apply_sql_migration(
            "006_usage_snapshots_latest_index",
            include_str!("../../migrations/006_usage_snapshots_latest_index.sql"),
        )?;
        self.apply_sql_or_compat_migration(
            "007_usage_secondary_columns",
            include_str!("../../migrations/007_usage_secondary_columns.sql"),
            |s| s.ensure_usage_secondary_columns(),
        )?;
        self.apply_sql_or_compat_migration(
            "008_token_api_key_access_token",
            include_str!("../../migrations/008_token_api_key_access_token.sql"),
            |s| s.ensure_token_api_key_column(),
        )?;
        self.apply_sql_or_compat_migration(
            "009_api_key_reasoning_effort",
            include_str!("../../migrations/009_api_key_reasoning_effort.sql"),
            |s| s.ensure_api_key_reasoning_column(),
        )?;
        self.apply_sql_or_compat_migration(
            "010_request_log_reasoning_effort",
            include_str!("../../migrations/010_request_log_reasoning_effort.sql"),
            |s| s.ensure_request_log_reasoning_column(),
        )?;

        // 中文注释：先走 SQL 迁移，遇到历史库重复列冲突再回退 compat；不这样写会导致老库和新库长期两套机制并存。
        self.apply_sql_or_compat_migration(
            "011_account_meta_columns",
            include_str!("../../migrations/011_account_meta_columns.sql"),
            |s| s.ensure_account_meta_columns(),
        )?;
        self.apply_sql_migration(
            "012_request_logs_search_indexes",
            include_str!("../../migrations/012_request_logs_search_indexes.sql"),
        )?;
        self.apply_sql_migration(
            "013_drop_accounts_note_tags",
            include_str!("../../migrations/013_drop_accounts_note_tags.sql"),
        )?;
        self.apply_sql_migration(
            "014_drop_accounts_workspace_name",
            include_str!("../../migrations/014_drop_accounts_workspace_name.sql"),
        )?;
        self.apply_sql_or_compat_migration(
            "015_api_key_profiles",
            include_str!("../../migrations/015_api_key_profiles.sql"),
            |s| s.ensure_api_key_profiles_table(),
        )?;
        self.apply_sql_migration(
            "016_api_keys_key_hash_index",
            include_str!("../../migrations/016_api_keys_key_hash_index.sql"),
        )?;
        self.apply_sql_migration(
            "017_usage_snapshots_captured_id_index",
            include_str!("../../migrations/017_usage_snapshots_captured_id_index.sql"),
        )?;
        self.apply_sql_migration(
            "018_accounts_sort_updated_at_index",
            include_str!("../../migrations/018_accounts_sort_updated_at_index.sql"),
        )?;
        self.apply_sql_or_compat_migration(
            "019_api_key_secrets",
            include_str!("../../migrations/019_api_key_secrets.sql"),
            |s| s.ensure_api_key_secrets_table(),
        )?;
        self.apply_sql_or_compat_migration(
            "020_request_logs_account_tokens_cost",
            include_str!("../../migrations/020_request_logs_account_tokens_cost.sql"),
            |s| s.ensure_request_log_account_tokens_cost_columns(),
        )?;
        self.apply_sql_or_compat_migration(
            "021_request_logs_cached_reasoning_tokens",
            include_str!("../../migrations/021_request_logs_cached_reasoning_tokens.sql"),
            |s| s.ensure_request_log_cached_reasoning_columns(),
        )?;
        self.apply_sql_or_compat_migration(
            "022_request_token_stats",
            include_str!("../../migrations/022_request_token_stats.sql"),
            |s| s.ensure_request_token_stats_table(),
        )?;
        self.apply_sql_or_compat_migration(
            "023_request_token_stats_total_tokens",
            include_str!("../../migrations/023_request_token_stats_total_tokens.sql"),
            |s| s.ensure_request_token_stats_table(),
        )?;
        self.apply_sql_migration(
            "024_model_options_cache",
            include_str!("../../migrations/024_model_options_cache.sql"),
        )?;
        self.apply_sql_or_compat_migration(
            "025_tokens_refresh_schedule",
            include_str!("../../migrations/025_tokens_refresh_schedule.sql"),
            |s| s.ensure_token_refresh_schedule_columns(),
        )?;
        self.apply_sql_migration(
            "026_api_key_profiles_constraints_azure",
            include_str!("../../migrations/026_api_key_profiles_constraints_azure.sql"),
        )?;
        self.apply_sql_or_compat_migration(
            "027_request_logs_trace_context",
            include_str!("../../migrations/027_request_logs_trace_context.sql"),
            |s| s.ensure_request_log_trace_context_columns(),
        )?;
        // 中文注释：旧版 request_logs 里遗留的 token 字段，需要先回填到 request_token_stats，
        // 再做表瘦身；否则压缩后会丢失历史 token 统计。
        self.ensure_request_token_stats_table()?;
        self.apply_compat_migration("028_request_logs_drop_legacy_usage_columns", |s| {
            s.compact_request_logs_legacy_usage_columns()
        })?;
        self.apply_sql_migration(
            "029_app_settings",
            include_str!("../../migrations/029_app_settings.sql"),
        )?;
        self.apply_sql_migration(
            "030_accounts_scale_indexes",
            include_str!("../../migrations/030_accounts_scale_indexes.sql"),
        )?;
        self.apply_sql_or_compat_migration(
            "031_request_logs_duration_ms",
            include_str!("../../migrations/031_request_logs_duration_ms.sql"),
            |s| s.ensure_request_log_duration_column(),
        )?;
        self.apply_sql_or_compat_migration(
            "032_request_logs_attempt_chain",
            include_str!("../../migrations/032_request_logs_attempt_chain.sql"),
            |s| s.ensure_request_log_attempt_chain_columns(),
        )?;
        self.apply_sql_or_compat_migration(
            "033_login_sessions_workspace_id",
            include_str!("../../migrations/033_login_sessions_workspace_id.sql"),
            |s| s.ensure_login_session_workspace_column(),
        )?;
        self.apply_sql_migration(
            "034_conversation_bindings",
            include_str!("../../migrations/034_conversation_bindings.sql"),
        )?;
        self.apply_sql_or_compat_migration(
            "035_api_key_profiles_service_tier",
            include_str!("../../migrations/035_api_key_profiles_service_tier.sql"),
            |s| s.ensure_api_key_service_tier_column(),
        )?;
        self.apply_sql_migration(
            "036_accounts_metadata_and_drop_group_name",
            include_str!("../../migrations/036_accounts_metadata_and_drop_group_name.sql"),
        )?;
        // 中文注释：036 历史迁移会删掉 accounts/login_sessions.group_name；
        // 当前账号组路由仍依赖这两列，因此在迁移后立即通过 compat 补回。
        self.ensure_account_meta_columns()?;
        self.apply_sql_or_compat_migration(
            "037_aggregate_api_routing",
            include_str!("../../migrations/037_aggregate_api_routing.sql"),
            |s| {
                s.ensure_api_key_rotation_columns()?;
                s.ensure_aggregate_apis_table()?;
                s.ensure_aggregate_api_secrets_table()
            },
        )?;
        self.apply_sql_or_compat_migration(
            "038_request_logs_aggregate_api_context",
            include_str!("../../migrations/038_request_logs_aggregate_api_context.sql"),
            |s| s.ensure_request_log_aggregate_api_context_columns(),
        )?;
        self.apply_sql_or_compat_migration(
            "039_request_logs_aggregate_api_attempt_chain",
            include_str!("../../migrations/039_request_logs_aggregate_api_attempt_chain.sql"),
            |s| s.ensure_request_log_aggregate_api_attempt_chain_columns(),
        )?;
        self.apply_sql_migration(
            "040_gateway_affinity_routing",
            include_str!("../../migrations/040_gateway_affinity_routing.sql"),
        )?;
        self.apply_sql_or_compat_migration(
            "041_cli_oauth_child_keys_and_quota",
            include_str!("../../migrations/041_cli_oauth_child_keys_and_quota.sql"),
            |s| {
                s.ensure_cli_child_keys_table()?;
                s.ensure_cli_oauth_sessions_table()?;
                s.ensure_account_quota_exhaustion_table()?;
                s.ensure_request_log_owner_key_column()?;
                s.ensure_request_token_stat_owner_key_column()
            },
        )?;
        self.ensure_api_key_rotation_columns()?;
        self.ensure_aggregate_apis_table()?;
        self.ensure_aggregate_api_secrets_table()?;
        self.ensure_request_token_stats_table()?;
        self.ensure_cli_child_keys_table()?;
        self.ensure_cli_oauth_sessions_table()?;
        self.ensure_account_quota_exhaustion_table()?;
        Ok(())
    }

    pub fn insert_login_session(&self, session: &LoginSession) -> Result<()> {
        self.conn.execute(
            "INSERT INTO login_sessions (login_id, code_verifier, state, status, error, workspace_id, note, tags, group_name, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            (
                &session.login_id,
                &session.code_verifier,
                &session.state,
                &session.status,
                &session.error,
                &session.workspace_id,
                &session.note,
                &session.tags,
                &session.group_name,
                session.created_at,
                session.updated_at,
            ),
        )?;
        Ok(())
    }

    pub fn get_login_session(&self, login_id: &str) -> Result<Option<LoginSession>> {
        let mut stmt = self.conn.prepare(
            "SELECT login_id, code_verifier, state, status, error, workspace_id, note, tags, group_name, created_at, updated_at FROM login_sessions WHERE login_id = ?1",
        )?;
        let mut rows = stmt.query([login_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(LoginSession {
                login_id: row.get(0)?,
                code_verifier: row.get(1)?,
                state: row.get(2)?,
                status: row.get(3)?,
                error: row.get(4)?,
                workspace_id: row.get(5)?,
                note: row.get(6)?,
                tags: row.get(7)?,
                group_name: row.get(8)?,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn update_login_session_status(
        &self,
        login_id: &str,
        status: &str,
        error: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE login_sessions SET status = ?1, error = ?2, updated_at = ?3 WHERE login_id = ?4",
            (status, error, now_ts(), login_id),
        )?;
        Ok(())
    }

    fn ensure_column(&self, table: &str, column: &str, column_type: &str) -> Result<()> {
        if self.has_column(table, column)? {
            return Ok(());
        }
        let sql = format!("ALTER TABLE {table} ADD COLUMN {column} {column_type}");
        self.conn.execute(&sql, [])?;
        Ok(())
    }

    fn has_column(&self, table: &str, column: &str) -> Result<bool> {
        let sql = format!("PRAGMA table_info({table})");
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let name: String = row.get(1)?;
            if name == column {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn ensure_migrations_table(&self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version TEXT PRIMARY KEY,
                applied_at INTEGER NOT NULL
            )",
            [],
        )?;
        Ok(())
    }

    fn has_migration(&self, version: &str) -> Result<bool> {
        let mut stmt = self
            .conn
            .prepare("SELECT 1 FROM schema_migrations WHERE version = ?1 LIMIT 1")?;
        let mut rows = stmt.query([version])?;
        Ok(rows.next()?.is_some())
    }

    fn mark_migration(&self, version: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
            (version, now_ts()),
        )?;
        Ok(())
    }

    fn apply_sql_migration(&self, version: &str, sql: &str) -> Result<()> {
        if self.has_migration(version)? {
            return Ok(());
        }
        self.conn.execute_batch(sql)?;
        self.mark_migration(version)
    }

    fn apply_sql_or_compat_migration<F>(&self, version: &str, sql: &str, compat: F) -> Result<()>
    where
        F: FnOnce(&Self) -> Result<()>,
    {
        if self.has_migration(version)? {
            return Ok(());
        }

        match self.conn.execute_batch(sql) {
            Ok(_) => {}
            Err(err) if Self::is_schema_conflict_error(&err) => {
                // 中文注释：历史库可能已通过旧版 ensure_* 加过列/表，不走 fallback 会让迁移在“重复列/表”上失败。
                compat(self)?;
            }
            Err(err) => return Err(err),
        }

        self.mark_migration(version)
    }

    fn apply_compat_migration<F>(&self, version: &str, compat: F) -> Result<()>
    where
        F: FnOnce(&Self) -> Result<()>,
    {
        if self.has_migration(version)? {
            return Ok(());
        }
        compat(self)?;
        self.mark_migration(version)
    }

    fn is_schema_conflict_error(err: &rusqlite::Error) -> bool {
        match err {
            rusqlite::Error::SqliteFailure(_, maybe_message) => maybe_message
                .as_deref()
                .map(|message| {
                    message.contains("duplicate column name") || message.contains("already exists")
                })
                .unwrap_or(false),
            _ => false,
        }
    }
}

pub fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
