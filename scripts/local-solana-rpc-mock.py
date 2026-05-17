#!/usr/bin/env python3
"""Deterministic local-only Solana JSON-RPC service for custody E2E runs."""

from __future__ import annotations

import hashlib
import json
import os
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any


STARTED_AT = int(time.time())
SENT_SIGNATURES: set[str] = set()
ZERO_BLOCKHASH = "11111111111111111111111111111111"


def slot() -> int:
    return STARTED_AT + int(time.time() - STARTED_AT)


def fake_signature(seed: str) -> str:
    # Solana signatures are base58, but custody only treats this as an opaque ID.
    return hashlib.sha256(seed.encode("utf-8")).hexdigest()


def rpc_result(method: str, params: list[Any]) -> Any:
    if method == "getHealth":
        return "ok"
    if method == "getSlot":
        return slot()
    if method == "getBlockHeight":
        return slot()
    if method == "getVersion":
        return {"solana-core": "lichen-local-solana-rpc/1.0", "feature-set": 0}
    if method in {"getLatestBlockhash", "getRecentBlockhash"}:
        return {
            "context": {"slot": slot()},
            "value": {
                "blockhash": ZERO_BLOCKHASH,
                "lastValidBlockHeight": slot() + 150,
            },
        }
    if method == "sendTransaction":
        raw_tx = str(params[0]) if params else ""
        signature = fake_signature(raw_tx)
        SENT_SIGNATURES.add(signature)
        return signature
    if method in {"getSignatureStatuses", "getSignatureStatus"}:
        signatures = []
        if params and isinstance(params[0], list):
            signatures = [str(value) for value in params[0]]
        values = []
        for signature in signatures:
            if signature in SENT_SIGNATURES:
                values.append({
                    "slot": slot(),
                    "confirmations": 1,
                    "confirmation_status": "finalized",
                    "err": None,
                })
            else:
                values.append(None)
        return {"context": {"slot": slot()}, "value": values}
    if method == "getSignaturesForAddress":
        return []
    if method == "getBalance":
        return {"context": {"slot": slot()}, "value": 0}
    if method == "getAccountInfo":
        return {"context": {"slot": slot()}, "value": None}
    if method == "getTokenAccountBalance":
        return {
            "context": {"slot": slot()},
            "value": {
                "amount": "0",
                "decimals": 6,
                "uiAmount": 0,
                "uiAmountString": "0",
            },
        }
    return None


def handle_rpc(payload: Any) -> Any:
    if isinstance(payload, list):
        return [handle_rpc(item) for item in payload]
    if not isinstance(payload, dict):
        return {"jsonrpc": "2.0", "id": None, "error": {"code": -32600, "message": "Invalid Request"}}

    method = str(payload.get("method") or "")
    params = payload.get("params") if isinstance(payload.get("params"), list) else []
    return {
        "jsonrpc": "2.0",
        "id": payload.get("id", 1),
        "result": rpc_result(method, params),
    }


class Handler(BaseHTTPRequestHandler):
    server_version = "LichenLocalSolanaRpc/1.0"

    def do_GET(self) -> None:
        if self.path == "/health":
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(b'{"status":"ok"}')
            return
        self.send_error(404)

    def do_POST(self) -> None:
        length = int(self.headers.get("Content-Length", "0") or "0")
        try:
            payload = json.loads(self.rfile.read(length).decode("utf-8") or "{}")
            response = handle_rpc(payload)
            encoded = json.dumps(response, separators=(",", ":")).encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(encoded)))
            self.end_headers()
            self.wfile.write(encoded)
        except Exception as exc:
            encoded = json.dumps({
                "jsonrpc": "2.0",
                "id": None,
                "error": {"code": -32603, "message": str(exc)},
            }).encode("utf-8")
            self.send_response(500)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(encoded)))
            self.end_headers()
            self.wfile.write(encoded)

    def log_message(self, fmt: str, *args: Any) -> None:
        print(f"{self.address_string()} - {fmt % args}", flush=True)


def main() -> None:
    host = os.getenv("LICHEN_LOCAL_SOLANA_HOST", "127.0.0.1")
    port = int(os.getenv("LICHEN_LOCAL_SOLANA_PORT", "18899"))
    server = ThreadingHTTPServer((host, port), Handler)
    print(f"local Solana RPC listening on http://{host}:{port}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
