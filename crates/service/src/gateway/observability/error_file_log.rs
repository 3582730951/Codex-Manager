use chrono::{Local, LocalResult, TimeZone};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

pub(super) struct GatewayErrorLogEntry<'a> {
    pub(super) ts: i64,
    pub(super) trace_id: Option<&'a str>,
    pub(super) key_id: Option<&'a str>,
    pub(super) account_id: Option<&'a str>,
    pub(super) method: &'a str,
    pub(super) request_path: &'a str,
    pub(super) original_path: Option<&'a str>,
    pub(super) adapted_path: Option<&'a str>,
    pub(super) model: Option<&'a str>,
    pub(super) reasoning_effort: Option<&'a str>,
    pub(super) status_code: Option<u16>,
    pub(super) error: Option<&'a str>,
}

pub(super) fn append_gateway_error_log(entry: GatewayErrorLogEntry<'_>) {
    if !should_record_error_log(entry.status_code, entry.error) {
        return;
    }
    let path = error_file_path_from_env();
    let line = format_gateway_error_log_line(&entry);
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(mut file) => {
            if let Err(err) = writeln!(file, "{line}") {
                log::warn!(
                    "gateway error file write failed: path={}, err={}",
                    path.display(),
                    err
                );
            }
        }
        Err(err) => {
            log::warn!(
                "gateway error file open failed: path={}, err={}",
                path.display(),
                err
            );
        }
    }
}

fn should_record_error_log(status_code: Option<u16>, error: Option<&str>) -> bool {
    if status_code.is_some_and(|status| status >= 400) {
        return true;
    }
    error
        .map(str::trim)
        .is_some_and(|message| !message.is_empty() && message != "-")
}

fn error_file_path_from_env() -> PathBuf {
    if let Ok(db_path) = std::env::var("CODEXMANAGER_DB_PATH") {
        let path = PathBuf::from(db_path);
        if let Some(parent) = path.parent() {
            return parent.join("gateway-error.txt");
        }
    }
    PathBuf::from("gateway-error.txt")
}

fn sanitize_text(value: &str) -> String {
    value.trim().replace(['\r', '\n'], " ")
}

fn format_ts(ts: i64) -> String {
    match Local.timestamp_opt(ts, 0) {
        LocalResult::Single(dt) => dt.format("%Y-%m-%d %H:%M:%S").to_string(),
        _ => ts.to_string(),
    }
}

fn format_gateway_error_log_line(entry: &GatewayErrorLogEntry<'_>) -> String {
    let path = entry
        .adapted_path
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(entry.request_path);
    let original_path = entry
        .original_path
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("-");
    let error = entry
        .error
        .map(sanitize_text)
        .unwrap_or_else(|| "-".to_string());
    let model = entry
        .model
        .map(sanitize_text)
        .unwrap_or_else(|| "-".to_string());
    let reasoning_effort = entry
        .reasoning_effort
        .map(sanitize_text)
        .unwrap_or_else(|| "-".to_string());
    format!(
        "[{}] trace_id={} key_id={} account_id={} method={} path={} original_path={} model={} reasoning={} status={} error={}",
        format_ts(entry.ts),
        sanitize_text(entry.trace_id.unwrap_or("-")),
        sanitize_text(entry.key_id.unwrap_or("-")),
        sanitize_text(entry.account_id.unwrap_or("-")),
        sanitize_text(entry.method),
        sanitize_text(path),
        sanitize_text(original_path),
        model,
        reasoning_effort,
        entry
            .status_code
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        error,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        append_gateway_error_log, error_file_path_from_env, format_gateway_error_log_line,
        should_record_error_log, GatewayErrorLogEntry,
    };
    use std::fs;

    #[test]
    fn should_record_error_log_for_http_error_status() {
        assert!(should_record_error_log(Some(502), None));
        assert!(!should_record_error_log(Some(200), None));
        assert!(should_record_error_log(
            Some(200),
            Some("上游流中途中断（未正常结束）")
        ));
    }

    #[test]
    fn error_file_path_uses_db_parent_directory() {
        let temp_root = std::env::temp_dir().join(format!(
            "codexmanager-error-path-test-{}",
            std::process::id()
        ));
        let db_path = temp_root.join("codexmanager.db");
        std::env::set_var("CODEXMANAGER_DB_PATH", &db_path);
        assert_eq!(
            error_file_path_from_env(),
            temp_root.join("gateway-error.txt")
        );
        std::env::remove_var("CODEXMANAGER_DB_PATH");
    }

    #[test]
    fn format_gateway_error_log_line_contains_core_fields() {
        let line = format_gateway_error_log_line(&GatewayErrorLogEntry {
            ts: 1_772_000_000,
            trace_id: Some("trc_1"),
            key_id: Some("gk_1"),
            account_id: Some("acc_1"),
            method: "POST",
            request_path: "/v1/responses",
            original_path: Some("/v1/responses"),
            adapted_path: Some("/v1/responses"),
            model: Some("gpt-5.3-codex"),
            reasoning_effort: Some("xhigh"),
            status_code: Some(502),
            error: Some("<html><title>Just a moment...</title></html>"),
        });
        assert!(line.contains("trace_id=trc_1"));
        assert!(line.contains("account_id=acc_1"));
        assert!(line.contains("path=/v1/responses"));
        assert!(line.contains("model=gpt-5.3-codex"));
        assert!(line.contains("status=502"));
        assert!(line.contains("error=<html><title>Just a moment...</title></html>"));
    }

    #[test]
    fn append_gateway_error_log_writes_text_file() {
        let temp_root = std::env::temp_dir().join(format!(
            "codexmanager-error-log-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp_root);
        fs::create_dir_all(&temp_root).expect("create temp dir");
        std::env::set_var(
            "CODEXMANAGER_DB_PATH",
            temp_root.join("codexmanager.db").display().to_string(),
        );

        append_gateway_error_log(GatewayErrorLogEntry {
            ts: 1_772_000_000,
            trace_id: Some("trc_write"),
            key_id: Some("gk_write"),
            account_id: Some("acc_write"),
            method: "POST",
            request_path: "/v1/responses",
            original_path: Some("/v1/responses"),
            adapted_path: Some("/v1/responses"),
            model: Some("gpt-5.3-codex"),
            reasoning_effort: Some("xhigh"),
            status_code: Some(502),
            error: Some("上游流中途中断（未正常结束）"),
        });

        let log_path = temp_root.join("gateway-error.txt");
        let content = fs::read_to_string(&log_path).expect("read gateway error log");
        assert!(content.contains("trace_id=trc_write"));
        assert!(content.contains("account_id=acc_write"));
        assert!(content.contains("error=上游流中途中断（未正常结束）"));

        std::env::remove_var("CODEXMANAGER_DB_PATH");
        let _ = fs::remove_dir_all(&temp_root);
    }
}
