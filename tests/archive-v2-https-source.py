#!/usr/bin/env python3
"""Bounded TLS fixture for Archive V2 authenticated object retrieval tests."""

import argparse
import hmac
import os
from pathlib import Path
import re
import ssl
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


OBJECT_PATH = re.compile(r"^/objects/([0-9a-f]{64})\.av2s$")
MAX_FIXTURE_OBJECT_BYTES = 2 * 1024 * 1024 * 1024


class ArchiveHandler(BaseHTTPRequestHandler):
    server_version = "LichenArchiveV2TestSource/1"

    def do_GET(self) -> None:
        expected = os.environ.get("LICHEN_TEST_ARCHIVE_BEARER_TOKEN", "")
        supplied = self.headers.get("Authorization", "")
        if not expected or not hmac.compare_digest(supplied, f"Bearer {expected}"):
            self.send_response(401)
            self.send_header("WWW-Authenticate", "Bearer")
            self.send_header("Content-Length", "0")
            self.end_headers()
            return

        match = OBJECT_PATH.fullmatch(self.path)
        if match is None:
            self.send_response(404)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        path = self.server.archive_root / "objects" / f"{match.group(1)}.av2s"
        try:
            size = path.stat().st_size
        except FileNotFoundError:
            self.send_response(404)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        if size > MAX_FIXTURE_OBJECT_BYTES:
            self.send_response(413)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return

        self.send_response(200)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Length", str(size))
        self.send_header("Cache-Control", "public, max-age=31536000, immutable")
        self.end_headers()
        with path.open("rb") as source:
            while chunk := source.read(1024 * 1024):
                self.wfile.write(chunk)

    def log_message(self, message: str, *args: object) -> None:
        print(f"{self.address_string()} {message % args}", flush=True)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--cert", type=Path, required=True)
    parser.add_argument("--key", type=Path, required=True)
    parser.add_argument("--port", type=int, required=True)
    args = parser.parse_args()

    root = args.root.resolve(strict=True)
    if not (1 <= args.port <= 65535):
        raise SystemExit("port is out of range")
    if not os.environ.get("LICHEN_TEST_ARCHIVE_BEARER_TOKEN"):
        raise SystemExit("LICHEN_TEST_ARCHIVE_BEARER_TOKEN is required")

    server = ThreadingHTTPServer(("127.0.0.1", args.port), ArchiveHandler)
    server.archive_root = root
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.minimum_version = ssl.TLSVersion.TLSv1_2
    context.load_cert_chain(args.cert, args.key)
    server.socket = context.wrap_socket(server.socket, server_side=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
