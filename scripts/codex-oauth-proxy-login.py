#!/usr/bin/env python3
import base64
import hashlib
import html
import json
import os
import secrets
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import sys


OAUTH_SCOPE = (
    "openid profile email offline_access api.connectors.read api.connectors.invoke"
)


def env(name: str, default: str = "") -> str:
    return os.environ.get(name, default).strip()


def b64url(raw: bytes) -> str:
    return base64.urlsafe_b64encode(raw).decode("ascii").rstrip("=")


def generate_pkce() -> tuple[str, str]:
    verifier = b64url(secrets.token_bytes(64))
    challenge = b64url(hashlib.sha256(verifier.encode("utf-8")).digest())
    return verifier, challenge


class CallbackState:
    def __init__(self, expected_state: str) -> None:
        self.expected_state = expected_state
        self.code: str | None = None
        self.error: str | None = None
        self.event = threading.Event()


class CallbackHandler(BaseHTTPRequestHandler):
    state: CallbackState

    def do_GET(self) -> None:  # noqa: N802
        parsed = urllib.parse.urlparse(self.path)
        if parsed.path != "/callback":
            self.send_error(404, "Not Found")
            return

        params = urllib.parse.parse_qs(parsed.query)
        code = (params.get("code") or [""])[0].strip()
        state = (params.get("state") or [""])[0].strip()
        error = (params.get("error_description") or params.get("error") or [""])[0].strip()

        if state != self.state.expected_state:
            self.state.error = "state mismatch"
            self._respond_page(
                400,
                "OAuth callback rejected",
                "The returned state did not match the pending login session.",
            )
            self.state.event.set()
            return

        if error:
            self.state.error = error
            self._respond_page(400, "OAuth callback failed", error)
            self.state.event.set()
            return

        if not code:
            self.state.error = "authorization code missing"
            self._respond_page(
                400, "OAuth callback failed", "No authorization code was returned."
            )
            self.state.event.set()
            return

        self.state.code = code
        self._respond_page(
            200,
            "Authorization complete",
            "CodexManager OAuth login succeeded. You may close this window.",
        )
        self.state.event.set()

    def log_message(self, fmt: str, *args) -> None:
        return

    def _respond_page(self, status: int, title: str, message: str) -> None:
        body = f"""<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width,initial-scale=1">
  <title>{html.escape(title)}</title>
</head>
<body>
  <h1>{html.escape(title)}</h1>
  <p>{html.escape(message)}</p>
</body>
</html>
"""
        payload = body.encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(payload)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(payload)


def post_form(url: str, payload: dict[str, str]) -> dict:
    encoded = urllib.parse.urlencode(payload).encode("utf-8")
    request = urllib.request.Request(
        url,
        data=encoded,
        headers={"Content-Type": "application/x-www-form-urlencoded"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            body = response.read().decode("utf-8")
    except urllib.error.HTTPError as exc:
        body = exc.read().decode("utf-8", errors="replace")
        raise SystemExit(f"OAuth token request failed: HTTP {exc.code}: {body}") from exc
    except urllib.error.URLError as exc:
        raise SystemExit(f"OAuth token request failed: {exc}") from exc

    try:
        return json.loads(body)
    except json.JSONDecodeError as exc:
        raise SystemExit(f"OAuth token response is not valid JSON: {body}") from exc


def write_auth_files(
    codex_home: Path,
    child_access_token: str,
    browser_issuer_base_url: str,
    token_issuer_base_url: str,
    callback_public_url: str,
    client_id: str,
) -> None:
    codex_home.mkdir(parents=True, exist_ok=True)
    auth_path = codex_home / "auth.json"
    auth_path.write_text(
        json.dumps(
            {
                "auth_mode": "apikey",
                "OPENAI_API_KEY": child_access_token,
            },
            ensure_ascii=False,
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    auth_path.chmod(0o600)

    meta_path = codex_home / "codexmanager-oauth.json"
    meta_path.write_text(
        json.dumps(
            {
                "type": "codexmanager_oauth_proxy",
                "browser_issuer_base_url": browser_issuer_base_url,
                "token_issuer_base_url": token_issuer_base_url,
                "callback_public_url": callback_public_url,
                "client_id": client_id,
                "saved_at": int(time.time()),
            },
            ensure_ascii=False,
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    meta_path.chmod(0o600)


def main() -> int:
    sys.stdout.reconfigure(line_buffering=True)
    codex_home = Path(env("CODEX_HOME", "/root/.codex"))
    browser_issuer_base_url = env("CODEX_OAUTH_BROWSER_ISSUER_BASE_URL")
    token_issuer_base_url = env(
        "CODEX_OAUTH_TOKEN_ISSUER_BASE_URL", browser_issuer_base_url
    )
    client_id = env("CODEX_OAUTH_CLIENT_ID", "codex-cli")
    callback_public_url = env(
        "CODEX_OAUTH_CALLBACK_PUBLIC_URL", "http://localhost:1455/callback"
    )
    callback_bind_host = env("CODEX_OAUTH_CALLBACK_BIND_HOST", "0.0.0.0")
    callback_bind_port = int(env("CODEX_OAUTH_CALLBACK_BIND_PORT", "1455"))
    timeout_seconds = int(env("CODEX_OAUTH_LOGIN_TIMEOUT_SECONDS", "600"))

    if not browser_issuer_base_url:
        raise SystemExit("CODEX_OAUTH_BROWSER_ISSUER_BASE_URL is required")
    if not token_issuer_base_url:
        raise SystemExit("CODEX_OAUTH_TOKEN_ISSUER_BASE_URL is required")

    verifier, challenge = generate_pkce()
    state = b64url(secrets.token_bytes(32))

    authorize_url = (
        f"{browser_issuer_base_url.rstrip('/')}/oauth/authorize?"
        + urllib.parse.urlencode(
            {
                "response_type": "code",
                "client_id": client_id,
                "redirect_uri": callback_public_url,
                "scope": OAUTH_SCOPE,
                "code_challenge": challenge,
                "code_challenge_method": "S256",
                "state": state,
                "originator": "codexmanager_container_proxy",
            }
        )
    )

    callback_state = CallbackState(expected_state=state)
    CallbackHandler.state = callback_state
    server = ThreadingHTTPServer((callback_bind_host, callback_bind_port), CallbackHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()

    print(
        f"Starting CodexManager OAuth callback server on http://{callback_bind_host}:{callback_bind_port}."
    )
    print("If your browser did not open, navigate to this URL to authenticate:\n")
    print(authorize_url)
    print("")
    print(
        f"Browser callback target: {callback_public_url}\nToken issuer: {token_issuer_base_url.rstrip('/')}"
    )

    if not callback_state.event.wait(timeout_seconds):
        server.shutdown()
        raise SystemExit("Timed out waiting for the OAuth callback.")

    server.shutdown()
    thread.join(timeout=5)

    if callback_state.error:
        raise SystemExit(f"OAuth login failed: {callback_state.error}")
    if not callback_state.code:
        raise SystemExit("OAuth login failed: no authorization code received.")

    token_base = token_issuer_base_url.rstrip("/")
    token_response = post_form(
        f"{token_base}/oauth/token",
        {
            "grant_type": "authorization_code",
            "code": callback_state.code,
            "redirect_uri": callback_public_url,
            "client_id": client_id,
            "code_verifier": verifier,
        },
    )
    id_token = str(token_response.get("id_token", "")).strip()
    if not id_token:
        raise SystemExit(f"OAuth token response is missing id_token: {token_response}")

    exchange_response = post_form(
        f"{token_base}/oauth/token",
        {
            "grant_type": "urn:ietf:params:oauth:grant-type:token-exchange",
            "client_id": client_id,
            "requested_token": "openai-api-key",
            "subject_token_type": "urn:ietf:params:oauth:token-type:id_token",
            "subject_token": id_token,
        },
    )
    child_access_token = str(exchange_response.get("access_token", "")).strip()
    if not child_access_token:
        raise SystemExit(
            f"OAuth token exchange is missing child access token: {exchange_response}"
        )

    write_auth_files(
        codex_home=codex_home,
        child_access_token=child_access_token,
        browser_issuer_base_url=browser_issuer_base_url,
        token_issuer_base_url=token_issuer_base_url,
        callback_public_url=callback_public_url,
        client_id=client_id,
    )
    print("\nCodexManager OAuth login succeeded. The child API key has been saved to auth.json.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
