#!/usr/bin/env python3
"""Deterministic local-only EVM JSON-RPC service for custody E2E runs."""

from __future__ import annotations

import hashlib
import json
import os
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any


STARTED_AT = int(time.time())
CHAIN_ID = int(os.getenv("LICHEN_LOCAL_EVM_CHAIN_ID", "1"))
SENT_TRANSACTIONS: dict[str, dict[str, Any]] = {}


def hex_quantity(value: int) -> str:
    return hex(max(0, value))


def zero_hash(seed: str) -> str:
    return "0x" + hashlib.sha256(seed.encode("utf-8")).hexdigest()


def rpc_result(method: str, params: list[Any]) -> Any:
    if method == "web3_clientVersion":
        return "lichen-local-evm-rpc/1.0"
    if method == "net_version":
        return str(CHAIN_ID)
    if method == "eth_chainId":
        return hex_quantity(CHAIN_ID)
    if method == "eth_blockNumber":
        return hex_quantity(STARTED_AT + int(time.time() - STARTED_AT))
    if method == "eth_getBalance":
        return "0x0"
    if method == "eth_getLogs":
        return []
    if method == "eth_getTransactionCount":
        return "0x0"
    if method == "eth_gasPrice":
        return "0x4a817c800"
    if method == "eth_maxPriorityFeePerGas":
        return "0x3b9aca00"
    if method == "eth_estimateGas":
        return "0x55f0"
    if method == "eth_call":
        return "0x" + "0" * 64
    if method == "eth_getTransactionReceipt":
        tx_hash = str(params[0]) if params else ""
        return SENT_TRANSACTIONS.get(tx_hash)
    if method == "eth_sendRawTransaction":
        raw_tx = str(params[0]) if params else ""
        tx_hash = zero_hash(raw_tx)
        SENT_TRANSACTIONS[tx_hash] = {
            "transactionHash": tx_hash,
            "status": "0x1",
            "blockNumber": hex_quantity(STARTED_AT + int(time.time() - STARTED_AT)),
            "gasUsed": "0x5208",
            "logs": [],
        }
        return tx_hash
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
    server_version = "LichenLocalEvmRpc/1.0"

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
    host = os.getenv("LICHEN_LOCAL_EVM_HOST", "127.0.0.1")
    port = int(os.getenv("LICHEN_LOCAL_EVM_PORT", "18545"))
    server = ThreadingHTTPServer((host, port), Handler)
    print(f"local EVM RPC listening on http://{host}:{port} chain_id={CHAIN_ID}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
