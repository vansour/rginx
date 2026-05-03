#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import threading
import time
from collections import Counter
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlsplit

REVALIDATE_ETAG = '"cache-bench-etag"'


class OriginState:
    def __init__(self, body_bytes: int, slice_payload_bytes: int, hot_fill_delay_ms: int) -> None:
        self.lock = threading.Lock()
        self.counts = Counter()
        self.body = (b"x" * body_bytes) or b"x"
        payload = bytearray()
        alphabet = b"abcdefghijklmnopqrstuvwxyz"
        while len(payload) < slice_payload_bytes:
            payload.extend(alphabet)
        self.slice_payload = bytes(payload[:slice_payload_bytes])
        self.hot_fill_delay_ms = hot_fill_delay_ms

    def bump(self, key: str) -> None:
        with self.lock:
            self.counts[key] += 1

    def snapshot(self) -> dict[str, int]:
        with self.lock:
            return dict(self.counts)

    def reset(self) -> None:
        with self.lock:
            self.counts.clear()


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, _format: str, *_args: object) -> None:
        return

    def do_POST(self) -> None:  # noqa: N802
        path = urlsplit(self.path).path
        state: OriginState = self.server.state  # type: ignore[attr-defined]
        if path == "/-/reset":
            state.reset()
            self._respond_json({"ok": True})
            return
        self.send_error(HTTPStatus.NOT_FOUND)

    def do_GET(self) -> None:  # noqa: N802
        parsed = urlsplit(self.path)
        path = parsed.path
        query = parse_qs(parsed.query)
        state: OriginState = self.server.state  # type: ignore[attr-defined]

        if path == "/-/ready":
            self._respond(200, b"ready\n", {"Cache-Control": "no-store"})
            return
        if path == "/-/stats":
            self._respond_json(state.snapshot())
            return
        if path.startswith("/fill/"):
            state.bump("fill")
            self._respond(200, state.body, {"Cache-Control": "max-age=60"})
            return
        if path.startswith("/hit/"):
            state.bump("hit")
            self._respond(200, state.body, {"Cache-Control": "max-age=60"})
            return
        if path == "/revalidate":
            state.bump("revalidate")
            headers = {
                "X-Accel-Expires": "@1",
                "ETag": REVALIDATE_ETAG,
                "Cache-Control": "max-age=60",
            }
            if self.headers.get("If-None-Match") == REVALIDATE_ETAG:
                self._respond(304, b"", headers)
            else:
                self._respond(200, state.body, headers)
            return
        if path == "/slice":
            state.bump("slice")
            self._respond_range(state.slice_payload)
            return
        if path == "/hot-fill":
            state.bump("hot_fill")
            time.sleep(state.hot_fill_delay_ms / 1000)
            body = b"hot-fill\n"
            ttl = query.get("ttl", ["60"])[0]
            self._respond(200, body, {"Cache-Control": f"max-age={ttl}"})
            return
        self._respond(404, b"not found\n", {"Cache-Control": "no-store"})

    def _respond_range(self, payload: bytes) -> None:
        range_header = self.headers.get("Range")
        if not range_header:
            self._respond(200, payload, {"Cache-Control": "max-age=60"})
            return
        parsed = parse_single_range(range_header, len(payload))
        if parsed is None:
            self._respond(416, b"", {"Content-Range": f"bytes */{len(payload)}"})
            return
        start, end = parsed
        body = payload[start : end + 1]
        self._respond(
            206,
            body,
            {
                "Cache-Control": "max-age=60",
                "Content-Range": f"bytes {start}-{end}/{len(payload)}",
            },
        )

    def _respond_json(self, payload: dict[str, object]) -> None:
        body = json.dumps(payload, sort_keys=True).encode()
        self._respond(200, body, {"Content-Type": "application/json", "Cache-Control": "no-store"})

    def _respond(self, status: int, body: bytes, headers: dict[str, str]) -> None:
        self.send_response(status)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        for name, value in headers.items():
            self.send_header(name, value)
        self.end_headers()
        if body:
            self.wfile.write(body)


def parse_single_range(header_value: str, payload_len: int) -> tuple[int, int] | None:
    value = header_value.strip()
    if not value.startswith("bytes="):
        return None
    raw_range = value[len("bytes=") :].strip()
    if "," in raw_range or "-" not in raw_range:
        return None
    raw_start, raw_end = raw_range.split("-", 1)
    raw_start = raw_start.strip()
    raw_end = raw_end.strip()
    if not raw_start and not raw_end:
        return None
    try:
        if raw_start:
            start = int(raw_start)
            if start < 0 or start >= payload_len:
                return None
            if raw_end:
                end = int(raw_end)
                if end < start:
                    return None
            else:
                end = payload_len - 1
            return start, min(end, payload_len - 1)
        suffix_len = int(raw_end)
    except ValueError:
        return None
    if suffix_len <= 0:
        return None
    suffix_len = min(suffix_len, payload_len)
    return payload_len - suffix_len, payload_len - 1


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=9000)
    parser.add_argument("--body-bytes", type=int, default=65536)
    parser.add_argument("--slice-payload-bytes", type=int, default=32768)
    parser.add_argument("--hot-fill-delay-ms", type=int, default=250)
    args = parser.parse_args()

    state = OriginState(args.body_bytes, args.slice_payload_bytes, args.hot_fill_delay_ms)
    server = ThreadingHTTPServer(("0.0.0.0", args.port), Handler)
    server.state = state  # type: ignore[attr-defined]
    server.serve_forever()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
