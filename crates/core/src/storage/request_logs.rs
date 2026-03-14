use rusqlite::{Result, Row};

use super::{request_log_query, RequestLog, RequestLogTodaySummary, RequestTokenStat, Storage};

impl Storage {
    fn ensure_request_logs_indexes(&self) -> Result<()> {
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_request_logs_created_at ON request_logs(created_at DESC)",
            [],
        )?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_request_logs_status_code_created_at ON request_logs(status_code, created_at DESC)",
            [],
        )?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_request_logs_method_created_at ON request_logs(method, created_at DESC)",
            [],
        )?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_request_logs_key_id_created_at ON request_logs(key_id, created_at DESC)",
            [],
        )?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_request_logs_account_id_created_at ON request_logs(account_id, created_at DESC)",
            [],
        )?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_request_logs_created_at_id ON request_logs(created_at DESC, id DESC)",
            [],
        )?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_request_logs_trace_id_created_at ON request_logs(trace_id, created_at DESC)",
            [],
        )?;
        Ok(())
    }

    pub fn insert_request_log(&self, log: &RequestLog) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO request_logs (
                trace_id, key_id, account_id, request_path, original_path, adapted_path,
                method, model, reasoning_effort, response_adapter, upstream_url, status_code, duration_ms, error, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            (
                &log.trace_id,
                &log.key_id,
                &log.account_id,
                &log.request_path,
                &log.original_path,
                &log.adapted_path,
                &log.method,
                &log.model,
                &log.reasoning_effort,
                &log.response_adapter,
                &log.upstream_url,
                log.status_code,
                log.duration_ms,
                &log.error,
                log.created_at,
            ),
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn insert_request_log_with_token_stat(
        &self,
        log: &RequestLog,
        stat: &RequestTokenStat,
    ) -> Result<(i64, Option<String>)> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO request_logs (
                trace_id, key_id, account_id, request_path, original_path, adapted_path,
                method, model, reasoning_effort, response_adapter, upstream_url, status_code, duration_ms, error, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            (
                &log.trace_id,
                &log.key_id,
                &log.account_id,
                &log.request_path,
                &log.original_path,
                &log.adapted_path,
                &log.method,
                &log.model,
                &log.reasoning_effort,
                &log.response_adapter,
                &log.upstream_url,
                log.status_code,
                log.duration_ms,
                &log.error,
                log.created_at,
            ),
        )?;
        let request_log_id = tx.last_insert_rowid();

        // 中文注释：token 统计写入失败不应阻塞 request log 保留（例如 sqlite busy/锁竞争）。
        // 这里保持“单事务单提交”，但 stat 失败时仍 commit request log。
        let token_stat_error = tx
            .execute(
                "INSERT INTO request_token_stats (
                    request_log_id, key_id, account_id, model,
                    input_tokens, cached_input_tokens, output_tokens, total_tokens, reasoning_output_tokens,
                    estimated_cost_usd, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                (
                    request_log_id,
                    &stat.key_id,
                    &stat.account_id,
                    &stat.model,
                    stat.input_tokens,
                    stat.cached_input_tokens,
                    stat.output_tokens,
                    stat.total_tokens,
                    stat.reasoning_output_tokens,
                    stat.estimated_cost_usd,
                    stat.created_at,
                ),
            )
            .err()
            .map(|err| err.to_string());

        tx.commit()?;
        Ok((request_log_id, token_stat_error))
    }

    pub fn list_request_logs(&self, query: Option<&str>, limit: i64) -> Result<Vec<RequestLog>> {
        let normalized_limit = if limit <= 0 { 200 } else { limit.min(1000) };
        let mut out = Vec::new();

        match request_log_query::parse_request_log_query(query) {
            request_log_query::RequestLogQuery::All => {
                let mut stmt = self.conn.prepare(
                    "SELECT
                        r.trace_id, r.key_id, r.account_id, r.request_path, r.original_path, r.adapted_path,
                        r.method, r.model, r.reasoning_effort, r.response_adapter, r.upstream_url, r.status_code, r.duration_ms,
                        t.input_tokens, t.cached_input_tokens, t.output_tokens, t.total_tokens, t.reasoning_output_tokens, t.estimated_cost_usd,
                        r.error, r.created_at
                     FROM request_logs r
                     LEFT JOIN request_token_stats t ON t.request_log_id = r.id
                     ORDER BY r.created_at DESC, r.id DESC
                     LIMIT ?1",
                )?;
                let mut rows = stmt.query([normalized_limit])?;
                while let Some(row) = rows.next()? {
                    out.push(map_request_log_row(row)?);
                }
            }
            request_log_query::RequestLogQuery::FieldLike { column, pattern } => {
                let sql = format!(
                    "SELECT
                        r.trace_id, r.key_id, r.account_id, r.request_path, r.original_path, r.adapted_path,
                        r.method, r.model, r.reasoning_effort, r.response_adapter, r.upstream_url, r.status_code, r.duration_ms,
                        t.input_tokens, t.cached_input_tokens, t.output_tokens, t.total_tokens, t.reasoning_output_tokens, t.estimated_cost_usd,
                        r.error, r.created_at
                     FROM request_logs r
                     LEFT JOIN request_token_stats t ON t.request_log_id = r.id
                     WHERE IFNULL(r.{column}, '') LIKE ?1
                     ORDER BY r.created_at DESC, r.id DESC
                     LIMIT ?2"
                );
                let mut stmt = self.conn.prepare(&sql)?;
                let mut rows = stmt.query((pattern, normalized_limit))?;
                while let Some(row) = rows.next()? {
                    out.push(map_request_log_row(row)?);
                }
            }
            request_log_query::RequestLogQuery::FieldExact { column, value } => {
                let sql = format!(
                    "SELECT
                        r.trace_id, r.key_id, r.account_id, r.request_path, r.original_path, r.adapted_path,
                        r.method, r.model, r.reasoning_effort, r.response_adapter, r.upstream_url, r.status_code, r.duration_ms,
                        t.input_tokens, t.cached_input_tokens, t.output_tokens, t.total_tokens, t.reasoning_output_tokens, t.estimated_cost_usd,
                        r.error, r.created_at
                     FROM request_logs r
                     LEFT JOIN request_token_stats t ON t.request_log_id = r.id
                     WHERE r.{column} = ?1
                     ORDER BY r.created_at DESC, r.id DESC
                     LIMIT ?2"
                );
                let mut stmt = self.conn.prepare(&sql)?;
                let mut rows = stmt.query((value, normalized_limit))?;
                while let Some(row) = rows.next()? {
                    out.push(map_request_log_row(row)?);
                }
            }
            request_log_query::RequestLogQuery::StatusExact(status) => {
                let mut stmt = self.conn.prepare(
                    "SELECT
                        r.trace_id, r.key_id, r.account_id, r.request_path, r.original_path, r.adapted_path,
                        r.method, r.model, r.reasoning_effort, r.response_adapter, r.upstream_url, r.status_code, r.duration_ms,
                        t.input_tokens, t.cached_input_tokens, t.output_tokens, t.total_tokens, t.reasoning_output_tokens, t.estimated_cost_usd,
                        r.error, r.created_at
                     FROM request_logs r
                     LEFT JOIN request_token_stats t ON t.request_log_id = r.id
                     WHERE r.status_code = ?1
                     ORDER BY r.created_at DESC, r.id DESC
                     LIMIT ?2",
                )?;
                let mut rows = stmt.query((status, normalized_limit))?;
                while let Some(row) = rows.next()? {
                    out.push(map_request_log_row(row)?);
                }
            }
            request_log_query::RequestLogQuery::StatusRange(start, end) => {
                let mut stmt = self.conn.prepare(
                    "SELECT
                        r.trace_id, r.key_id, r.account_id, r.request_path, r.original_path, r.adapted_path,
                        r.method, r.model, r.reasoning_effort, r.response_adapter, r.upstream_url, r.status_code, r.duration_ms,
                        t.input_tokens, t.cached_input_tokens, t.output_tokens, t.total_tokens, t.reasoning_output_tokens, t.estimated_cost_usd,
                        r.error, r.created_at
                     FROM request_logs r
                     LEFT JOIN request_token_stats t ON t.request_log_id = r.id
                     WHERE r.status_code >= ?1 AND r.status_code <= ?2
                     ORDER BY r.created_at DESC, r.id DESC
                     LIMIT ?3",
                )?;
                let mut rows = stmt.query((start, end, normalized_limit))?;
                while let Some(row) = rows.next()? {
                    out.push(map_request_log_row(row)?);
                }
            }
            request_log_query::RequestLogQuery::GlobalLike(pattern) => {
                let mut stmt = self.conn.prepare(
                    "SELECT
                        r.trace_id, r.key_id, r.account_id, r.request_path, r.original_path, r.adapted_path,
                        r.method, r.model, r.reasoning_effort, r.response_adapter, r.upstream_url, r.status_code, r.duration_ms,
                        t.input_tokens, t.cached_input_tokens, t.output_tokens, t.total_tokens, t.reasoning_output_tokens, t.estimated_cost_usd,
                        r.error, r.created_at
                     FROM request_logs r
                     LEFT JOIN request_token_stats t ON t.request_log_id = r.id
                     WHERE r.request_path LIKE ?1
                        OR IFNULL(r.original_path,'') LIKE ?1
                        OR IFNULL(r.adapted_path,'') LIKE ?1
                        OR r.method LIKE ?1
                        OR IFNULL(r.account_id,'') LIKE ?1
                        OR IFNULL(r.model,'') LIKE ?1
                        OR IFNULL(r.reasoning_effort,'') LIKE ?1
                        OR IFNULL(r.response_adapter,'') LIKE ?1
                        OR IFNULL(r.error,'') LIKE ?1
                        OR IFNULL(r.key_id,'') LIKE ?1
                        OR IFNULL(r.trace_id,'') LIKE ?1
                        OR IFNULL(r.upstream_url,'') LIKE ?1
                        OR IFNULL(CAST(r.status_code AS TEXT),'') LIKE ?1
                        OR IFNULL(CAST(t.input_tokens AS TEXT),'') LIKE ?1
                        OR IFNULL(CAST(t.cached_input_tokens AS TEXT),'') LIKE ?1
                        OR IFNULL(CAST(t.output_tokens AS TEXT),'') LIKE ?1
                        OR IFNULL(CAST(t.total_tokens AS TEXT),'') LIKE ?1
                        OR IFNULL(CAST(t.reasoning_output_tokens AS TEXT),'') LIKE ?1
                        OR IFNULL(CAST(t.estimated_cost_usd AS TEXT),'') LIKE ?1
                     ORDER BY r.created_at DESC, r.id DESC
                     LIMIT ?2",
                )?;
                let mut rows = stmt.query((pattern, normalized_limit))?;
                while let Some(row) = rows.next()? {
                    out.push(map_request_log_row(row)?);
                }
            }
        }

        Ok(out)
    }

    pub fn clear_request_logs(&self) -> Result<()> {
        // 只清理请求明细日志，保留 token 统计用于仪表盘历史用量与费用汇总。
        self.conn.execute("DELETE FROM request_logs", [])?;
        Ok(())
    }

    pub fn summarize_request_logs_between(
        &self,
        start_ts: i64,
        end_ts: i64,
    ) -> Result<RequestLogTodaySummary> {
        self.summarize_request_token_stats_between(start_ts, end_ts)
    }

    pub(super) fn ensure_request_logs_table(&self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS request_logs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                trace_id TEXT,
                key_id TEXT,
                account_id TEXT,
                request_path TEXT NOT NULL,
                original_path TEXT,
                adapted_path TEXT,
                method TEXT NOT NULL,
                model TEXT,
                reasoning_effort TEXT,
                response_adapter TEXT,
                upstream_url TEXT,
                status_code INTEGER,
                duration_ms INTEGER,
                error TEXT,
                created_at INTEGER NOT NULL
            )",
            [],
        )?;
        self.ensure_request_logs_indexes()?;
        Ok(())
    }

    pub(super) fn ensure_request_log_reasoning_column(&self) -> Result<()> {
        self.ensure_column("request_logs", "reasoning_effort", "TEXT")?;
        Ok(())
    }

    pub(super) fn ensure_request_log_account_tokens_cost_columns(&self) -> Result<()> {
        self.ensure_column("request_logs", "account_id", "TEXT")?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_request_logs_account_id_created_at ON request_logs(account_id, created_at DESC)",
            [],
        )?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_request_logs_created_at_id ON request_logs(created_at DESC, id DESC)",
            [],
        )?;
        Ok(())
    }

    pub(super) fn ensure_request_log_cached_reasoning_columns(&self) -> Result<()> {
        Ok(())
    }

    pub(super) fn ensure_request_log_trace_context_columns(&self) -> Result<()> {
        self.ensure_column("request_logs", "trace_id", "TEXT")?;
        self.ensure_column("request_logs", "original_path", "TEXT")?;
        self.ensure_column("request_logs", "adapted_path", "TEXT")?;
        self.ensure_column("request_logs", "response_adapter", "TEXT")?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_request_logs_trace_id_created_at ON request_logs(trace_id, created_at DESC)",
            [],
        )?;
        Ok(())
    }

    pub(super) fn ensure_request_log_duration_column(&self) -> Result<()> {
        self.ensure_column("request_logs", "duration_ms", "INTEGER")?;
        Ok(())
    }

    pub(super) fn compact_request_logs_legacy_usage_columns(&self) -> Result<()> {
        self.ensure_request_logs_table()?;
        self.ensure_request_log_reasoning_column()?;
        self.ensure_request_log_account_tokens_cost_columns()?;
        self.ensure_request_log_trace_context_columns()?;

        let legacy_columns = [
            "input_tokens",
            "output_tokens",
            "estimated_cost_usd",
            "cached_input_tokens",
            "reasoning_output_tokens",
        ];
        let mut has_legacy_columns = false;
        for column in legacy_columns {
            if self.has_column("request_logs", column)? {
                has_legacy_columns = true;
                break;
            }
        }
        if !has_legacy_columns {
            return Ok(());
        }

        let tx = self.conn.unchecked_transaction()?;
        tx.execute_batch(
            "ALTER TABLE request_logs RENAME TO request_logs_legacy_028;
             CREATE TABLE request_logs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                trace_id TEXT,
                key_id TEXT,
                account_id TEXT,
                request_path TEXT NOT NULL,
                original_path TEXT,
                adapted_path TEXT,
                method TEXT NOT NULL,
                model TEXT,
                reasoning_effort TEXT,
                response_adapter TEXT,
                upstream_url TEXT,
                status_code INTEGER,
                duration_ms INTEGER,
                error TEXT,
                created_at INTEGER NOT NULL
             );
             INSERT INTO request_logs (
                id, trace_id, key_id, account_id, request_path, original_path, adapted_path,
                method, model, reasoning_effort, response_adapter, upstream_url, status_code, duration_ms, error, created_at
             )
             SELECT
                id, trace_id, key_id, account_id, request_path, original_path, adapted_path,
                method, model, reasoning_effort, response_adapter, upstream_url, status_code, NULL, error, created_at
             FROM request_logs_legacy_028;
             DROP TABLE request_logs_legacy_028;",
        )?;
        tx.commit()?;

        self.ensure_request_logs_indexes()?;
        Ok(())
    }
}

fn map_request_log_row(row: &Row<'_>) -> Result<RequestLog> {
    Ok(RequestLog {
        trace_id: row.get(0)?,
        key_id: row.get(1)?,
        account_id: row.get(2)?,
        request_path: row.get(3)?,
        original_path: row.get(4)?,
        adapted_path: row.get(5)?,
        method: row.get(6)?,
        model: row.get(7)?,
        reasoning_effort: row.get(8)?,
        response_adapter: row.get(9)?,
        upstream_url: row.get(10)?,
        status_code: row.get(11)?,
        duration_ms: row.get(12)?,
        input_tokens: row.get(13)?,
        cached_input_tokens: row.get(14)?,
        output_tokens: row.get(15)?,
        total_tokens: row.get(16)?,
        reasoning_output_tokens: row.get(17)?,
        estimated_cost_usd: row.get(18)?,
        error: row.get(19)?,
        created_at: row.get(20)?,
    })
}

#[cfg(test)]
#[path = "tests/request_logs_tests.rs"]
mod tests;
