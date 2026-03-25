use rusqlite::{params, params_from_iter, types::Value, Result, Row};

use super::{
    request_log_query, RequestLog, RequestLogQuerySummary, RequestLogTodaySummary,
    RequestTokenStat, Storage,
};

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
                trace_id, key_id, account_id, initial_account_id, attempted_account_ids_json, initial_aggregate_api_id, attempted_aggregate_api_ids_json,
                request_path, original_path, adapted_path,
                method, model, reasoning_effort, response_adapter, upstream_url, aggregate_api_supplier_name, aggregate_api_url, status_code, duration_ms, error, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)",
            params![
                &log.trace_id,
                &log.key_id,
                &log.account_id,
                &log.initial_account_id,
                &log.attempted_account_ids_json,
                &log.initial_aggregate_api_id,
                &log.attempted_aggregate_api_ids_json,
                &log.request_path,
                &log.original_path,
                &log.adapted_path,
                &log.method,
                &log.model,
                &log.reasoning_effort,
                &log.response_adapter,
                &log.upstream_url,
                &log.aggregate_api_supplier_name,
                &log.aggregate_api_url,
                log.status_code,
                log.duration_ms,
                &log.error,
                log.created_at,
            ],
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
                trace_id, key_id, account_id, initial_account_id, attempted_account_ids_json, initial_aggregate_api_id, attempted_aggregate_api_ids_json,
                request_path, original_path, adapted_path,
                method, model, reasoning_effort, response_adapter, upstream_url, aggregate_api_supplier_name, aggregate_api_url, status_code, duration_ms, error, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)",
            params![
                &log.trace_id,
                &log.key_id,
                &log.account_id,
                &log.initial_account_id,
                &log.attempted_account_ids_json,
                &log.initial_aggregate_api_id,
                &log.attempted_aggregate_api_ids_json,
                &log.request_path,
                &log.original_path,
                &log.adapted_path,
                &log.method,
                &log.model,
                &log.reasoning_effort,
                &log.response_adapter,
                &log.upstream_url,
                &log.aggregate_api_supplier_name,
                &log.aggregate_api_url,
                log.status_code,
                log.duration_ms,
                &log.error,
                log.created_at,
            ],
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
        self.list_request_logs_paginated(query, None, 0, limit)
    }

    pub fn list_request_logs_paginated(
        &self,
        query: Option<&str>,
        status_filter: Option<&str>,
        offset: i64,
        limit: i64,
    ) -> Result<Vec<RequestLog>> {
        let normalized_limit = normalize_request_log_limit(limit);
        let normalized_offset = offset.max(0);
        let filters = build_request_log_filters(query, status_filter);
        let sql = format!(
            "SELECT
                r.trace_id, r.key_id, r.account_id, r.initial_account_id, r.attempted_account_ids_json, r.initial_aggregate_api_id, r.attempted_aggregate_api_ids_json,
                r.request_path, r.original_path, r.adapted_path,
                r.method, r.model, r.reasoning_effort, r.response_adapter, r.upstream_url, r.aggregate_api_supplier_name, r.aggregate_api_url, r.status_code, r.duration_ms,
                t.input_tokens, t.cached_input_tokens, t.output_tokens, t.total_tokens, t.reasoning_output_tokens, t.estimated_cost_usd,
                r.error, r.created_at
             FROM request_logs r
             LEFT JOIN request_token_stats t ON t.request_log_id = r.id
             {where_clause}
             ORDER BY r.created_at DESC, r.id DESC
             LIMIT ? OFFSET ?",
            where_clause = filters.where_clause
        );
        let mut params = filters.params;
        params.push(Value::Integer(normalized_limit));
        params.push(Value::Integer(normalized_offset));

        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query(params_from_iter(params.iter()))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(map_request_log_row(row)?);
        }
        Ok(out)
    }

    pub fn count_request_logs(
        &self,
        query: Option<&str>,
        status_filter: Option<&str>,
    ) -> Result<i64> {
        let filters = build_request_log_filters(query, status_filter);
        let sql = format!(
            "SELECT COUNT(1)
             FROM request_logs r
             LEFT JOIN request_token_stats t ON t.request_log_id = r.id
             {where_clause}",
            where_clause = filters.where_clause
        );
        self.conn
            .query_row(&sql, params_from_iter(filters.params.iter()), |row| {
                row.get(0)
            })
    }

    pub fn summarize_request_logs_filtered(
        &self,
        query: Option<&str>,
        status_filter: Option<&str>,
    ) -> Result<RequestLogQuerySummary> {
        let filters = build_request_log_filters(query, status_filter);
        let sql = format!(
            "SELECT
                COUNT(1),
                IFNULL(SUM(CASE WHEN r.status_code >= 200 AND r.status_code <= 299 THEN 1 ELSE 0 END), 0),
                IFNULL(SUM(CASE WHEN IFNULL(r.status_code, 0) >= 400 OR TRIM(IFNULL(r.error, '')) <> '' THEN 1 ELSE 0 END), 0),
                IFNULL(SUM(
                    CASE
                        WHEN t.total_tokens IS NOT NULL THEN
                            CASE WHEN t.total_tokens > 0 THEN t.total_tokens ELSE 0 END
                        ELSE
                            CASE
                                WHEN IFNULL(t.input_tokens, 0) - IFNULL(t.cached_input_tokens, 0) + IFNULL(t.output_tokens, 0) > 0
                                    THEN IFNULL(t.input_tokens, 0) - IFNULL(t.cached_input_tokens, 0) + IFNULL(t.output_tokens, 0)
                                ELSE 0
                            END
                    END
                ), 0)
             FROM request_logs r
             LEFT JOIN request_token_stats t ON t.request_log_id = r.id
             {where_clause}",
            where_clause = filters.where_clause
        );
        self.conn
            .query_row(&sql, params_from_iter(filters.params.iter()), |row| {
                Ok(RequestLogQuerySummary {
                    count: row.get(0)?,
                    success_count: row.get(1)?,
                    error_count: row.get(2)?,
                    total_tokens: row.get(3)?,
                })
            })
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
                initial_account_id TEXT,
                attempted_account_ids_json TEXT,
                initial_aggregate_api_id TEXT,
                attempted_aggregate_api_ids_json TEXT,
                request_path TEXT NOT NULL,
                original_path TEXT,
                adapted_path TEXT,
                method TEXT NOT NULL,
                model TEXT,
                reasoning_effort TEXT,
                response_adapter TEXT,
                upstream_url TEXT,
                aggregate_api_supplier_name TEXT,
                aggregate_api_url TEXT,
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

    pub(super) fn ensure_request_log_aggregate_api_context_columns(&self) -> Result<()> {
        self.ensure_column("request_logs", "aggregate_api_supplier_name", "TEXT")?;
        self.ensure_column("request_logs", "aggregate_api_url", "TEXT")?;
        Ok(())
    }

    pub(super) fn ensure_request_log_attempt_chain_columns(&self) -> Result<()> {
        self.ensure_column("request_logs", "initial_account_id", "TEXT")?;
        self.ensure_column("request_logs", "attempted_account_ids_json", "TEXT")?;
        Ok(())
    }

    pub(super) fn ensure_request_log_aggregate_api_attempt_chain_columns(&self) -> Result<()> {
        self.ensure_column("request_logs", "initial_aggregate_api_id", "TEXT")?;
        self.ensure_column("request_logs", "attempted_aggregate_api_ids_json", "TEXT")?;
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
                initial_account_id TEXT,
                attempted_account_ids_json TEXT,
                initial_aggregate_api_id TEXT,
                attempted_aggregate_api_ids_json TEXT,
                request_path TEXT NOT NULL,
                original_path TEXT,
                adapted_path TEXT,
                method TEXT NOT NULL,
                model TEXT,
                reasoning_effort TEXT,
                response_adapter TEXT,
                upstream_url TEXT,
                aggregate_api_supplier_name TEXT,
                aggregate_api_url TEXT,
                status_code INTEGER,
                duration_ms INTEGER,
                error TEXT,
                created_at INTEGER NOT NULL
             );
             INSERT INTO request_logs (
                id, trace_id, key_id, account_id, initial_account_id, attempted_account_ids_json, initial_aggregate_api_id, attempted_aggregate_api_ids_json,
                request_path, original_path, adapted_path,
                method, model, reasoning_effort, response_adapter, upstream_url, aggregate_api_supplier_name, aggregate_api_url, status_code, duration_ms, error, created_at
             )
             SELECT
                id, trace_id, key_id, account_id, NULL, NULL, NULL, NULL, request_path, original_path, adapted_path,
                method, model, reasoning_effort, response_adapter, upstream_url, NULL, NULL, status_code, NULL, error, created_at
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
        initial_account_id: row.get(3)?,
        attempted_account_ids_json: row.get(4)?,
        initial_aggregate_api_id: row.get(5)?,
        attempted_aggregate_api_ids_json: row.get(6)?,
        request_path: row.get(7)?,
        original_path: row.get(8)?,
        adapted_path: row.get(9)?,
        method: row.get(10)?,
        model: row.get(11)?,
        reasoning_effort: row.get(12)?,
        response_adapter: row.get(13)?,
        upstream_url: row.get(14)?,
        aggregate_api_supplier_name: row.get(15)?,
        aggregate_api_url: row.get(16)?,
        status_code: row.get(17)?,
        duration_ms: row.get(18)?,
        input_tokens: row.get(19)?,
        cached_input_tokens: row.get(20)?,
        output_tokens: row.get(21)?,
        total_tokens: row.get(22)?,
        reasoning_output_tokens: row.get(23)?,
        estimated_cost_usd: row.get(24)?,
        error: row.get(25)?,
        created_at: row.get(26)?,
    })
}

struct RequestLogSqlFilters {
    where_clause: String,
    params: Vec<Value>,
}

fn normalize_request_log_limit(value: i64) -> i64 {
    if value <= 0 {
        200
    } else {
        value.min(1000)
    }
}

fn build_request_log_filters(
    query: Option<&str>,
    status_filter: Option<&str>,
) -> RequestLogSqlFilters {
    let mut clauses = Vec::new();
    let mut params = Vec::new();

    append_request_log_query_clause(
        request_log_query::parse_request_log_query(query),
        &mut clauses,
        &mut params,
    );
    append_status_filter_clause(status_filter, &mut clauses, &mut params);

    RequestLogSqlFilters {
        where_clause: if clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", clauses.join(" AND "))
        },
        params,
    }
}

fn append_request_log_query_clause(
    query: request_log_query::RequestLogQuery,
    clauses: &mut Vec<String>,
    params: &mut Vec<Value>,
) {
    match query {
        request_log_query::RequestLogQuery::All => {}
        request_log_query::RequestLogQuery::FieldLike { column, pattern } => {
            clauses.push(format!("IFNULL(r.{column}, '') LIKE ?"));
            params.push(Value::Text(pattern));
        }
        request_log_query::RequestLogQuery::FieldExact { column, value } => {
            clauses.push(format!("r.{column} = ?"));
            params.push(Value::Text(value));
        }
        request_log_query::RequestLogQuery::StatusExact(status) => {
            clauses.push("r.status_code = ?".to_string());
            params.push(Value::Integer(status));
        }
        request_log_query::RequestLogQuery::StatusRange(start, end) => {
            clauses.push("r.status_code >= ? AND r.status_code <= ?".to_string());
            params.push(Value::Integer(start));
            params.push(Value::Integer(end));
        }
        request_log_query::RequestLogQuery::GlobalLike(pattern) => {
            clauses.push(
                "(r.request_path LIKE ?
                    OR IFNULL(r.initial_account_id,'') LIKE ?
                    OR IFNULL(r.attempted_account_ids_json,'') LIKE ?
                    OR IFNULL(r.initial_aggregate_api_id,'') LIKE ?
                    OR IFNULL(r.attempted_aggregate_api_ids_json,'') LIKE ?
                    OR IFNULL(r.aggregate_api_supplier_name,'') LIKE ?
                    OR IFNULL(r.aggregate_api_url,'') LIKE ?
                    OR IFNULL(r.original_path,'') LIKE ?
                    OR IFNULL(r.adapted_path,'') LIKE ?
                    OR r.method LIKE ?
                    OR IFNULL(r.account_id,'') LIKE ?
                    OR IFNULL(r.model,'') LIKE ?
                    OR IFNULL(r.reasoning_effort,'') LIKE ?
                    OR IFNULL(r.response_adapter,'') LIKE ?
                    OR IFNULL(r.error,'') LIKE ?
                    OR IFNULL(r.key_id,'') LIKE ?
                    OR IFNULL(r.trace_id,'') LIKE ?
                    OR IFNULL(r.upstream_url,'') LIKE ?
                    OR IFNULL(CAST(r.status_code AS TEXT),'') LIKE ?
                    OR IFNULL(CAST(t.input_tokens AS TEXT),'') LIKE ?
                    OR IFNULL(CAST(t.cached_input_tokens AS TEXT),'') LIKE ?
                    OR IFNULL(CAST(t.output_tokens AS TEXT),'') LIKE ?
                    OR IFNULL(CAST(t.total_tokens AS TEXT),'') LIKE ?
                    OR IFNULL(CAST(t.reasoning_output_tokens AS TEXT),'') LIKE ?
                    OR IFNULL(CAST(t.estimated_cost_usd AS TEXT),'') LIKE ?)"
                    .to_string(),
            );
            for _ in 0..25 {
                params.push(Value::Text(pattern.clone()));
            }
        }
    }
}

fn append_status_filter_clause(
    status_filter: Option<&str>,
    clauses: &mut Vec<String>,
    params: &mut Vec<Value>,
) {
    let normalized = status_filter
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase();
    match normalized.as_str() {
        "" | "all" => {}
        "2xx" => {
            clauses.push("r.status_code >= ? AND r.status_code <= ?".to_string());
            params.push(Value::Integer(200));
            params.push(Value::Integer(299));
        }
        "4xx" => {
            clauses.push("r.status_code >= ? AND r.status_code <= ?".to_string());
            params.push(Value::Integer(400));
            params.push(Value::Integer(499));
        }
        "5xx" => {
            clauses.push("r.status_code >= ?".to_string());
            params.push(Value::Integer(500));
        }
        _ => {}
    }
}

#[cfg(test)]
#[path = "tests/request_logs_tests.rs"]
mod tests;
