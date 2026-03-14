use super::*;

use axum::extract::Query;
use serde::Deserialize;

const WEB_AUTH_TAB_SESSION_STORAGE_KEY: &str = "codexmanager_web_auth_tab";

#[derive(Debug, Deserialize)]
pub(super) struct LoginForm {
    password: String,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct LoginQuery {
    force: Option<String>,
}

fn current_web_access_password_hash() -> Option<String> {
    codexmanager_service::current_web_access_password_hash()
}

pub(super) fn generate_web_auth_session_key() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

pub(super) fn build_web_auth_cookie_value(
    password_hash: &str,
    rpc_token: &str,
    session_key: &str,
) -> String {
    let scoped_rpc_token = format!("{rpc_token}:{session_key}");
    codexmanager_service::build_web_access_session_token(password_hash, &scoped_rpc_token)
}

pub(super) fn parse_cookie_value(headers: &HeaderMap, cookie_name: &str) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|segment| {
        let (name, value) = segment.trim().split_once('=')?;
        if name.trim() == cookie_name {
            Some(value.trim().to_string())
        } else {
            None
        }
    })
}

fn set_cookie_header_value(value: &str) -> Option<HeaderValue> {
    HeaderValue::from_str(&format!(
        "{WEB_AUTH_COOKIE_NAME}={value}; Path=/; HttpOnly; SameSite=Lax"
    ))
    .ok()
}

fn clear_cookie_header_value() -> Option<HeaderValue> {
    HeaderValue::from_str(&format!(
        "{WEB_AUTH_COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"
    ))
    .ok()
}

fn append_no_store_headers(response: &mut Response) {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, no-cache, must-revalidate"),
    );
    response
        .headers_mut()
        .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    response
        .headers_mut()
        .insert(header::EXPIRES, HeaderValue::from_static("0"));
}

fn login_force_requested(query: &LoginQuery) -> bool {
    query
        .force
        .as_deref()
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
}

fn request_is_authenticated(headers: &HeaderMap, state: &AppState) -> bool {
    let Some(password_hash) = current_web_access_password_hash() else {
        return true;
    };
    let Some(cookie_value) = parse_cookie_value(headers, WEB_AUTH_COOKIE_NAME) else {
        return false;
    };
    let expected = build_web_auth_cookie_value(
        &password_hash,
        &state.rpc_token,
        &state.web_auth_session_key,
    );
    cookie_value == expected
}

fn builtin_login_html(error: Option<&str>) -> String {
    let error_html = error
        .map(|text| format!(r#"<div class="error">{}</div>"#, escape_html(text)))
        .unwrap_or_default();
    format!(
        r#"<!doctype html>
<html lang="zh-CN">
  <head>
    <meta charset="utf-8"/>
    <meta name="viewport" content="width=device-width, initial-scale=1"/>
    <title>CodexManager Web 登录</title>
    <style>
      :root {{
        color-scheme: light;
        --bg: #eef3f8;
        --panel: rgba(255,255,255,.92);
        --text: #142033;
        --muted: #627389;
        --accent: #0f6fff;
        --accent-strong: #0a57ca;
        --border: rgba(20,32,51,.12);
        --error-bg: rgba(193, 45, 45, .1);
        --error-fg: #b42318;
      }}
      * {{ box-sizing: border-box; }}
      body {{
        margin: 0;
        min-height: 100vh;
        display: grid;
        place-items: center;
        padding: 24px;
        font-family: "Segoe UI", "PingFang SC", "Microsoft YaHei", sans-serif;
        background:
          radial-gradient(circle at top left, rgba(15,111,255,.18), transparent 32%),
          radial-gradient(circle at bottom right, rgba(45,164,78,.14), transparent 26%),
          linear-gradient(160deg, #f6f9fc 0%, #e8eef6 100%);
        color: var(--text);
      }}
      .card {{
        width: min(100%, 420px);
        padding: 28px;
        border: 1px solid var(--border);
        border-radius: 20px;
        background: var(--panel);
        box-shadow: 0 24px 60px rgba(15, 23, 42, .12);
        backdrop-filter: blur(14px);
      }}
      .mark {{
        display: inline-flex;
        align-items: center;
        justify-content: center;
        width: 44px;
        height: 44px;
        border-radius: 14px;
        background: linear-gradient(135deg, #0f6fff, #2bb673);
        color: #fff;
        font-weight: 700;
      }}
      h1 {{ margin: 16px 0 6px; font-size: 22px; }}
      p {{ margin: 0 0 18px; color: var(--muted); line-height: 1.6; }}
      label {{ display: block; margin-bottom: 10px; font-size: 14px; color: var(--muted); }}
      input {{
        width: 100%;
        border: 1px solid rgba(20,32,51,.16);
        border-radius: 14px;
        padding: 13px 14px;
        font-size: 15px;
        outline: none;
        background: rgba(255,255,255,.92);
      }}
      input:focus {{
        border-color: rgba(15,111,255,.58);
        box-shadow: 0 0 0 4px rgba(15,111,255,.12);
      }}
      button {{
        width: 100%;
        margin-top: 16px;
        border: 0;
        border-radius: 14px;
        padding: 13px 16px;
        font-size: 15px;
        font-weight: 600;
        color: #fff;
        background: linear-gradient(135deg, var(--accent), var(--accent-strong));
        cursor: pointer;
      }}
      button:hover {{ filter: brightness(.98); }}
      .error {{
        margin-bottom: 14px;
        padding: 12px 14px;
        border-radius: 12px;
        background: var(--error-bg);
        color: var(--error-fg);
        font-size: 14px;
      }}
      .foot {{
        margin-top: 14px;
        font-size: 12px;
        color: var(--muted);
        text-align: center;
      }}
    </style>
  </head>
  <body>
    <form class="card" method="post" action="/__login">
      <div class="mark">CM</div>
      <h1>访问受保护</h1>
      <p>当前 CodexManager Web 已启用访问密码，请先验证后再进入管理页面。</p>
      {error_html}
      <label for="password">访问密码</label>
      <input id="password" name="password" type="password" autocomplete="current-password" autofocus />
      <button type="submit">进入控制台</button>
      <div class="foot">密码可在桌面端右上角安全入口或设置页中修改。</div>
    </form>
  </body>
</html>
"#
    )
}

fn login_success_html() -> String {
    format!(
        r#"<!doctype html>
<html lang="zh-CN">
  <head>
    <meta charset="utf-8"/>
    <meta name="viewport" content="width=device-width, initial-scale=1"/>
    <title>CodexManager Web 登录</title>
  </head>
  <body>
    <script>
      try {{
        window.sessionStorage.setItem("{WEB_AUTH_TAB_SESSION_STORAGE_KEY}", "1");
      }} catch (_err) {{}}
      window.location.replace("/");
    </script>
  </body>
</html>
"#
    )
}

fn logout_success_html() -> String {
    format!(
        r#"<!doctype html>
<html lang="zh-CN">
  <head>
    <meta charset="utf-8"/>
    <meta name="viewport" content="width=device-width, initial-scale=1"/>
    <title>CodexManager Web 已退出</title>
  </head>
  <body>
    <script>
      try {{
        window.sessionStorage.removeItem("{WEB_AUTH_TAB_SESSION_STORAGE_KEY}");
      }} catch (_err) {{}}
      window.location.replace("/__login?force=1");
    </script>
  </body>
</html>
"#
    )
}

pub(super) async fn web_auth_middleware(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_string();
    if path == "/__login" || path == "/__logout" {
        return next.run(request).await;
    }
    if request_is_authenticated(request.headers(), state.as_ref()) {
        return next.run(request).await;
    }
    if path.starts_with("/api/") {
        return (
            StatusCode::UNAUTHORIZED,
            axum::Json(serde_json::json!({ "error": "web_auth_required" })),
        )
            .into_response();
    }
    Redirect::to("/__login").into_response()
}

pub(super) async fn login_page(
    State(state): State<Arc<AppState>>,
    Query(query): Query<LoginQuery>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if current_web_access_password_hash().is_none() {
        return Redirect::to("/").into_response();
    }
    if request_is_authenticated(&headers, state.as_ref()) && !login_force_requested(&query) {
        return Redirect::to("/").into_response();
    }
    let mut response = Html(builtin_login_html(None)).into_response();
    append_no_store_headers(&mut response);
    response
}

pub(super) async fn login_submit(
    State(state): State<Arc<AppState>>,
    axum::Form(form): axum::Form<LoginForm>,
) -> impl IntoResponse {
    let Some(password_hash) = current_web_access_password_hash() else {
        return Redirect::to("/").into_response();
    };
    if !codexmanager_service::verify_web_access_password(&form.password) {
        let mut response = (
            StatusCode::UNAUTHORIZED,
            Html(builtin_login_html(Some("密码错误，请重试。"))),
        )
            .into_response();
        append_no_store_headers(&mut response);
        return response;
    }
    let token = build_web_auth_cookie_value(
        &password_hash,
        &state.rpc_token,
        &state.web_auth_session_key,
    );
    let mut response = Html(login_success_html()).into_response();
    if let Some(header_value) = set_cookie_header_value(&token) {
        response
            .headers_mut()
            .append(header::SET_COOKIE, header_value);
    }
    append_no_store_headers(&mut response);
    response
}

pub(super) async fn logout() -> impl IntoResponse {
    let mut response = Html(logout_success_html()).into_response();
    if let Some(header_value) = clear_cookie_header_value() {
        response
            .headers_mut()
            .append(header::SET_COOKIE, header_value);
    }
    append_no_store_headers(&mut response);
    response
}

pub(super) async fn auth_status() -> impl IntoResponse {
    let mut response = axum::Json(serde_json::json!({
        "passwordConfigured": current_web_access_password_hash().is_some(),
    }))
    .into_response();
    append_no_store_headers(&mut response);
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_force_requested_accepts_truthy_flags() {
        for value in ["1", "true", "TRUE", "yes", "on"] {
            let query = LoginQuery {
                force: Some(value.to_string()),
            };
            assert!(login_force_requested(&query), "value={value}");
        }
        for value in ["", "0", "false", "no", "off"] {
            let query = LoginQuery {
                force: Some(value.to_string()),
            };
            assert!(!login_force_requested(&query), "value={value}");
        }
        assert!(!login_force_requested(&LoginQuery::default()));
    }

    #[test]
    fn login_success_html_marks_current_tab_session() {
        let html = login_success_html();
        assert!(html.contains("sessionStorage.setItem"));
        assert!(html.contains(WEB_AUTH_TAB_SESSION_STORAGE_KEY));
        assert!(html.contains("location.replace(\"/\")"));
    }
}
