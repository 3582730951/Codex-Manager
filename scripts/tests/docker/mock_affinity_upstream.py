#!/usr/bin/env python3
import json
import os
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


PORT = int(os.getenv("MOCK_UPSTREAM_PORT", "18080"))
QUOTA_MESSAGE = (
    "You've hit your usage limit. To get more access now, "
    "send a request to your admin or try again at 12:51 PM."
)
QUOTA_DATE = "Tue, 31 Mar 2026 04:30:00 GMT"


def extract_token(headers):
    auth = headers.get("Authorization", "").strip()
    if auth.startswith("Bearer "):
        return auth[len("Bearer ") :].strip()
    return headers.get("x-api-key", "").strip()


def first_text(items):
    if not isinstance(items, list):
        return ""
    for item in items:
        if not isinstance(item, dict):
            continue
        content = item.get("content")
        if isinstance(content, list):
            for block in content:
                if not isinstance(block, dict):
                    continue
                text = block.get("text")
                if isinstance(text, str) and text.strip():
                    return text.strip()
        text = item.get("text")
        if isinstance(text, str) and text.strip():
            return text.strip()
    return ""


def build_completed_response(token, payload):
    now = int(time.time() * 1000)
    text = first_text(payload.get("input") or [])
    if not text:
        text = "ok"
    output_text = f"mock:{token}:{text}"
    return {
        "id": f"resp_{token}_{now}",
        "object": "response",
        "status": "completed",
        "model": payload.get("model") or "gpt-5.4",
        "output": [
            {
                "id": f"msg_{token}_{now}",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": output_text}],
            }
        ],
        "usage": {"input_tokens": 4, "output_tokens": 3, "total_tokens": 7},
    }


def build_usage_limit_http_error():
    return {
        "error": {
            "type": "usage_limit_reached",
            "code": "usage_limit_reached",
            "message": QUOTA_MESSAGE,
            "plan_type": "pro",
            "resets_at": 1774961460,
        }
    }


def build_quota_failed_event(response_id):
    return {
        "type": "response.failed",
        "response": {
            "id": response_id,
            "status": "failed",
            "error": {
                "type": "insufficient_quota",
                "code": "insufficient_quota",
                "message": "You exceeded your current quota, please check your plan and billing details.",
            },
        },
    }


def build_usage_limit_sse_error():
    return {
        "type": "error",
        "status": 429,
        "error": {
            "type": "usage_limit_reached",
            "code": "usage_limit_reached",
            "message": QUOTA_MESSAGE,
            "plan_type": "pro",
            "resets_at": 1774961460,
        },
    }


class Handler(BaseHTTPRequestHandler):
    server_version = "affinity-mock/1.0"

    def log_message(self, _format, *_args):
        return

    def _write_json(self, status, payload):
        body = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Date", QUOTA_DATE)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
        self.wfile.flush()

    def _write_html(self, status, html):
        body = html.encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
        self.wfile.flush()

    def do_GET(self):
        if self.path == "/health":
            self._write_json(200, {"ok": True})
            return
        self._write_json(404, {"error": "not_found"})

    def do_POST(self):
        if not self.path.startswith("/v1/responses"):
            self._write_json(404, {"error": {"message": "unsupported path"}})
            return

        token = extract_token(self.headers)
        raw = self.rfile.read(int(self.headers.get("Content-Length", "0") or "0"))
        try:
            payload = json.loads(raw or b"{}")
        except json.JSONDecodeError:
            self._write_json(400, {"error": {"message": "invalid_json"}})
            return

        if token.endswith("-unauthorized"):
            self._write_json(401, {"error": {"message": "mock unauthorized"}})
            return
        if token.endswith("-quota-http-json"):
            body = json.dumps(build_usage_limit_http_error()).encode("utf-8")
            self.send_response(429)
            self.send_header("Content-Type", "application/json")
            self.send_header("Retry-After", "1800")
            self.send_header("Date", QUOTA_DATE)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            self.wfile.flush()
            return
        if token.endswith("-quota"):
            self._write_json(
                429,
                {"error": {"message": "insufficient_quota", "code": "insufficient_quota"}},
            )
            return
        if token.endswith("-challenge"):
            self._write_html(403, "<html><body>mock challenge</body></html>")
            return
        if token.endswith("-5xx"):
            self._write_json(503, {"error": {"message": "mock upstream unavailable"}})
            return

        completed = build_completed_response(token or "anon", payload)
        created_at = int(time.time())
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Date", QUOTA_DATE)
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.end_headers()

        delta = {
            "type": "response.output_text.delta",
            "delta": completed["output"][0]["content"][0]["text"],
        }
        frames = [
            (
                "response.created",
                {
                    "type": "response.created",
                    "response": {
                        "id": completed["id"],
                        "model": completed["model"],
                        "created": created_at,
                    },
                },
            ),
            ("response.output_text.delta", delta),
        ]
        if token.endswith("-quota-sse-failed"):
            frames.append(("response.failed", build_quota_failed_event(completed["id"])))
        elif token.endswith("-quota-sse-extra"):
            frames.append(
                ("response.completed", {"type": "response.completed", "response": completed})
            )
            frames.append(("error", build_usage_limit_sse_error()))
        elif not token.endswith("-incomplete"):
            frames.append(
                ("response.completed", {"type": "response.completed", "response": completed})
            )
        for event_name, body in frames:
            frame = (
                f"event: {event_name}\n"
                f"data: {json.dumps(body, ensure_ascii=False)}\n\n"
            ).encode("utf-8")
            self.wfile.write(frame)
            self.wfile.flush()
        if token.endswith("-incomplete"):
            return
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()


def main():
    server = ThreadingHTTPServer(("0.0.0.0", PORT), Handler)
    server.serve_forever()


if __name__ == "__main__":
    main()
