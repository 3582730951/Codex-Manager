use super::*;

pub(super) fn builtin_missing_ui_html(detail: &str) -> String {
    let detail = escape_html(detail);
    format!(
        r#"<!doctype html>
<html lang="zh-CN">
  <head>
    <meta charset="utf-8"/>
    <meta name="viewport" content="width=device-width, initial-scale=1"/>
    <title>CodexManager Web</title>
    <style>
      body {{ font-family: ui-sans-serif, system-ui, -apple-system, Segoe UI, Roboto, Helvetica, Arial; padding: 40px; line-height: 1.5; color: #111; }}
      .box {{ max-width: 860px; margin: 0 auto; border: 1px solid #e5e7eb; border-radius: 12px; padding: 20px 24px; background: #fafafa; }}
      h1 {{ margin: 0 0 8px; font-size: 20px; }}
      p {{ margin: 10px 0; color: #374151; }}
      code {{ background: #111827; color: #f9fafb; padding: 2px 6px; border-radius: 6px; }}
      a {{ color: #2563eb; }}
    </style>
  </head>
  <body>
    <div class="box">
      <h1>前端资源未就绪</h1>
      <p>当前 <code>codexmanager-web</code> 没有找到可用的前端静态资源。</p>
      <p>详情：<code>{detail}</code></p>
      <p>解决方式：</p>
      <p>1) 使用官方发行物（已内置前端资源）；或</p>
      <p>2) 从源码运行：先执行 <code>pnpm -C apps run build:desktop</code>，再设置 <code>CODEXMANAGER_WEB_ROOT=.../apps/out</code> 启动。</p>
      <p>关闭：访问 <a href="/__quit">/__quit</a>。</p>
    </div>
  </body>
</html>
"#
    )
}

pub(super) async fn serve_ui_root(State(state): State<Arc<AppState>>) -> Response {
    serve_ui_path(state.as_ref(), "")
}

pub(super) async fn serve_ui_asset(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(path): axum::extract::Path<String>,
) -> Response {
    serve_ui_path(state.as_ref(), &path)
}

fn serve_ui_path(state: &AppState, path: &str) -> Response {
    match &state.ui_source {
        UiSource::Disk(root) => serve_disk_path(root, path),
        UiSource::Embedded => serve_embedded_path(path),
        UiSource::Missing => Html((*state.missing_ui_html).clone()).into_response(),
    }
}

fn serve_embedded_path(path: &str) -> Response {
    let Some(candidates) = ui_asset_candidates(path) else {
        return (StatusCode::BAD_REQUEST, "bad path").into_response();
    };

    for candidate in &candidates {
        if let Some(bytes) = embedded_ui::read_asset_bytes(candidate) {
            return build_asset_response(StatusCode::OK, bytes.to_vec(), candidate);
        }
    }

    fallback_not_found_response(&candidates, |candidate| {
        embedded_ui::read_asset_bytes(candidate).map(|bytes| bytes.to_vec())
    })
}

fn serve_disk_path(root: &std::path::Path, path: &str) -> Response {
    let Some(candidates) = ui_asset_candidates(path) else {
        return (StatusCode::BAD_REQUEST, "bad path").into_response();
    };

    for candidate in &candidates {
        if let Some(bytes) = read_disk_asset(root, candidate) {
            return build_asset_response(StatusCode::OK, bytes, candidate);
        }
    }

    fallback_not_found_response(&candidates, |candidate| read_disk_asset(root, candidate))
}

fn fallback_not_found_response<F>(candidates: &[String], mut read_asset: F) -> Response
where
    F: FnMut(&str) -> Option<Vec<u8>>,
{
    if candidates.first().is_some_and(|candidate| is_html_route(candidate)) {
        for fallback in ["404.html", "_not-found.html", "index.html"] {
            if let Some(bytes) = read_asset(fallback) {
                let status = if fallback == "index.html" {
                    StatusCode::OK
                } else {
                    StatusCode::NOT_FOUND
                };
                return build_asset_response(status, bytes, fallback);
            }
        }
    }

    (StatusCode::NOT_FOUND, "missing ui").into_response()
}

fn build_asset_response(status: StatusCode, bytes: Vec<u8>, path: &str) -> Response {
    let mime = embedded_ui::guess_mime(path);
    let mut out = Response::new(axum::body::Body::from(bytes));
    *out.status_mut() = status;
    out.headers_mut().insert(
        "content-type",
        axum::http::HeaderValue::from_str(&mime)
            .unwrap_or_else(|_| axum::http::HeaderValue::from_static("application/octet-stream")),
    );
    out
}

fn read_disk_asset(root: &std::path::Path, relative: &str) -> Option<Vec<u8>> {
    let full = root.join(relative);
    std::fs::read(full).ok()
}

fn ui_asset_candidates(path: &str) -> Option<Vec<String>> {
    let raw = path.trim_start_matches('/');
    if raw.contains("..") {
        return None;
    }

    let normalized = raw.trim_matches('/');
    if normalized.is_empty() {
        return Some(vec!["index.html".to_string()]);
    }

    let mut candidates = vec![normalized.to_string()];
    if is_html_route(normalized) {
        candidates.push(format!("{normalized}.html"));
        candidates.push(format!("{normalized}/index.html"));
    }
    candidates.dedup();
    Some(candidates)
}

fn is_html_route(path: &str) -> bool {
    let last = path.rsplit('/').next().unwrap_or(path);
    !last.contains('.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_asset_candidates_map_next_export_routes() {
        assert_eq!(ui_asset_candidates("").unwrap(), vec!["index.html"]);
        assert_eq!(
            ui_asset_candidates("/accounts").unwrap(),
            vec![
                "accounts".to_string(),
                "accounts.html".to_string(),
                "accounts/index.html".to_string()
            ]
        );
        assert_eq!(
            ui_asset_candidates("/_next/static/app.js").unwrap(),
            vec!["_next/static/app.js".to_string()]
        );
    }

    #[test]
    fn ui_asset_candidates_reject_parent_segments() {
        assert!(ui_asset_candidates("../secret").is_none());
        assert!(ui_asset_candidates("/foo/../../bar").is_none());
    }
}
