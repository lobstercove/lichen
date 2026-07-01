#!/usr/bin/env python3
"""Public exchange-readiness gate.

This is intentionally stricter than a generic uptime probe. It checks the
public surfaces an exchange would rely on and fails closed when operator-only
approvals are still missing.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import socket
import ssl
import struct
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[2]
DEFAULT_REPORT = ROOT / "tests" / "artifacts" / "exchange-public-readiness-report.json"

TESTNET_RPC = "https://testnet-rpc.lichen.network"
TESTNET_WS = "wss://testnet-rpc.lichen.network/ws"
MAINNET_RPC = "https://rpc.lichen.network"
MAINNET_WS = "wss://rpc.lichen.network/ws"
EXPLORER = "https://explorer.lichen.network"
LOGO_URL = "https://lichen.network/Lichen_Logo_256.png"
STATUS_URL = "https://monitoring.lichen.network"
DEVELOPER_EXCHANGE_URL = "https://developers.lichen.network/exchange-integration"
ROLLBACK_TAG = "v0.5.215"
ROLLBACK_RELEASE_API = (
    "https://api.github.com/repos/lobstercove/lichen/releases/tags/" + ROLLBACK_TAG
)
ROLLBACK_RELEASE_PAGE = "https://github.com/lobstercove/lichen/releases/tag/" + ROLLBACK_TAG
EXPECTED_LOGO_SHA256 = "bfa0986bc4bde64c3c7ce590782beba78980985f301fbd0fbd4a39dc045ca876"
DEVELOPER_EXCHANGE_REQUIRED_SNIPPETS = (
    "Exchange Integration",
    "Exchange Integration Guide",
    "Exchange Chain Metadata",
    "Exchange Operations Pack",
    "testnet-only",
)
PACKAGE_SCOPES = ("testnet", "full")


class Gate:
    def __init__(self) -> None:
        self.checks: list[dict[str, Any]] = []

    def add(
        self,
        name: str,
        ok: bool,
        *,
        blocking: bool = True,
        detail: Any = None,
    ) -> None:
        self.checks.append(
            {
                "name": name,
                "ok": bool(ok),
                "blocking": bool(blocking),
                "detail": detail,
            }
        )

    def failed(self) -> list[dict[str, Any]]:
        return [check for check in self.checks if check["blocking"] and not check["ok"]]


def default_package_scope() -> str:
    scope = os.environ.get("LICHEN_EXCHANGE_PACKAGE_SCOPE", "testnet").strip().lower()
    return scope if scope in PACKAGE_SCOPES else "testnet"


def package_includes_mainnet(scope: str) -> bool:
    return scope == "full"


def request_json(url: str, body: dict[str, Any] | None = None, timeout: int = 15) -> tuple[int, Any]:
    data = None
    headers = {
        "Accept": "application/json",
        "User-Agent": "lichen-exchange-public-readiness/1.0",
    }
    if body is not None:
        data = json.dumps(body).encode("utf-8")
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(url, data=data, headers=headers)
    with urllib.request.urlopen(request, timeout=timeout) as response:
        payload = response.read()
        return response.status, json.loads(payload.decode("utf-8"))


def request_bytes(url: str, timeout: int = 15) -> tuple[int, dict[str, str], bytes]:
    request = urllib.request.Request(
        url,
        headers={
            "Accept": "*/*",
            "User-Agent": "lichen-exchange-public-readiness/1.0",
        },
    )
    with urllib.request.urlopen(request, timeout=timeout) as response:
        headers = {key.lower(): value for key, value in response.headers.items()}
        return response.status, headers, response.read()


def rpc_call(url: str, method: str, params: list[Any] | None = None) -> dict[str, Any]:
    _, payload = request_json(
        url,
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params or [],
        },
    )
    return payload


def rpc_health_ready(result: dict[str, Any], max_block_age_secs: int) -> bool:
    status = result.get("status")
    age = result.get("block_age_secs")
    return status == "ok" and isinstance(age, int) and age <= max_block_age_secs


def check_rpc_health(
    gate: Gate,
    name: str,
    url: str,
    max_block_age_secs: int,
) -> dict[str, Any] | None:
    try:
        payload = rpc_call(url, "getHealth")
    except Exception as exc:
        gate.add(name, False, detail=str(exc))
        return None

    result = payload.get("result") if isinstance(payload, dict) else None
    if not isinstance(result, dict):
        gate.add(name, False, detail=payload)
        return None

    status = result.get("status")
    age = result.get("block_age_secs")
    ok = rpc_health_ready(result, max_block_age_secs)
    gate.add(
        name,
        ok,
        detail={
            "status": status,
            "slot": result.get("slot"),
            "block_age_secs": age,
            "max_block_age_secs": max_block_age_secs,
            "reason": result.get("reason"),
            "disk": result.get("disk"),
        },
    )
    return result


def check_rpc_method(gate: Gate, name: str, url: str, method: str, params: list[Any] | None = None) -> None:
    try:
        payload = rpc_call(url, method, params)
    except Exception as exc:
        gate.add(name, False, detail=str(exc))
        return
    if payload.get("error"):
        gate.add(name, False, detail=payload["error"])
        return
    gate.add(name, "result" in payload, detail=payload.get("result"))


def ws_upgrade(url: str, timeout: int = 8) -> tuple[bool, str]:
    if not url.startswith("wss://"):
        return False, "only wss:// URLs are supported"
    rest = url[len("wss://") :]
    host_port, _, path = rest.partition("/")
    path = "/" + path
    host, _, port_text = host_port.partition(":")
    port = int(port_text or "443")
    key = base64.b64encode(os.urandom(16)).decode("ascii")
    request = (
        f"GET {path} HTTP/1.1\r\n"
        f"Host: {host}\r\n"
        "Upgrade: websocket\r\n"
        "Connection: Upgrade\r\n"
        f"Sec-WebSocket-Key: {key}\r\n"
        "Sec-WebSocket-Version: 13\r\n"
        "User-Agent: lichen-exchange-public-readiness/1.0\r\n"
        "\r\n"
    ).encode("ascii")
    try:
        with socket.create_connection((host, port), timeout=timeout) as raw:
            context = ssl.create_default_context()
            with context.wrap_socket(raw, server_hostname=host) as sock:
                sock.settimeout(timeout)
                sock.sendall(request)
                data = sock.recv(4096).decode("iso-8859-1", errors="replace")
    except Exception as exc:
        return False, str(exc)
    first_line = data.splitlines()[0] if data else ""
    return " 101 " in first_line, first_line


def check_http(
    gate: Gate,
    name: str,
    url: str,
    *,
    contains: str | None = None,
    contains_all: tuple[str, ...] = (),
) -> bytes:
    try:
        status, headers, body = request_bytes(url)
    except Exception as exc:
        gate.add(name, False, detail=str(exc))
        return b""
    required = []
    if contains is not None:
        required.append(contains)
    required.extend(contains_all)
    missing = [snippet for snippet in required if snippet.encode("utf-8") not in body]
    ok = status == 200 and not missing
    gate.add(
        name,
        ok,
        detail={
            "status": status,
            "content_type": headers.get("content-type"),
            "content_length": len(body),
            "missing": missing,
        },
    )
    return body


def png_dimensions(body: bytes) -> tuple[int, int] | None:
    if len(body) < 24 or body[:8] != b"\x89PNG\r\n\x1a\n" or body[12:16] != b"IHDR":
        return None
    return struct.unpack(">II", body[16:24])


def check_logo(gate: Gate) -> None:
    try:
        status, headers, body = request_bytes(LOGO_URL)
    except Exception as exc:
        gate.add("logo public asset", False, detail=str(exc))
        return
    digest = hashlib.sha256(body).hexdigest()
    dimensions = png_dimensions(body)
    ok = (
        status == 200
        and headers.get("content-type", "").startswith("image/png")
        and dimensions == (256, 256)
        and digest == EXPECTED_LOGO_SHA256
    )
    gate.add(
        "logo public asset",
        ok,
        detail={
            "status": status,
            "content_type": headers.get("content-type"),
            "bytes": len(body),
            "dimensions": dimensions,
            "sha256": digest,
        },
    )


def check_release(gate: Gate) -> None:
    try:
        status, payload = request_json(ROLLBACK_RELEASE_API)
    except Exception as exc:
        gate.add("rollback release API", False, detail=str(exc))
        return
    assets = payload.get("assets") if isinstance(payload, dict) else []
    asset_names = sorted(asset.get("name") for asset in assets if isinstance(asset, dict))
    required = {
        "SHA256SUMS",
        "SHA256SUMS.sig",
        "lichen-validator-darwin-aarch64.tar.gz",
        "lichen-validator-darwin-x86_64.tar.gz",
        "lichen-validator-linux-aarch64.tar.gz",
        "lichen-validator-linux-x86_64.tar.gz",
        "lichen-validator-windows-x86_64.tar.gz",
    }
    ok = (
        status == 200
        and payload.get("tag_name") == ROLLBACK_TAG
        and payload.get("draft") is False
        and payload.get("prerelease") is False
        and required.issubset(set(asset_names))
    )
    gate.add(
        "rollback release API",
        ok,
        detail={
            "release": ROLLBACK_RELEASE_PAGE,
            "published_at": payload.get("published_at"),
            "draft": payload.get("draft"),
            "prerelease": payload.get("prerelease"),
            "assets": asset_names,
        },
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report", default=str(DEFAULT_REPORT))
    parser.add_argument("--max-block-age-secs", type=int, default=15)
    parser.add_argument(
        "--scope",
        choices=PACKAGE_SCOPES,
        default=default_package_scope(),
        help=(
            "exchange package scope: 'testnet' skips mainnet readiness until "
            "mainnet launch; 'full' requires mainnet RPC/WS readiness too"
        ),
    )
    parser.add_argument(
        "--status-approved",
        action="store_true",
        default=os.environ.get("LICHEN_EXCHANGE_STATUS_APPROVED") == "1",
        help="mark status-page approval complete; default also reads LICHEN_EXCHANGE_STATUS_APPROVED=1",
    )
    parser.add_argument(
        "--release-tag-selected",
        action="store_true",
        default=os.environ.get("LICHEN_EXCHANGE_RELEASE_TAG_SELECTED") == "1",
        help="mark final exchange package release tag selected; default also reads LICHEN_EXCHANGE_RELEASE_TAG_SELECTED=1",
    )
    args = parser.parse_args()

    gate = Gate()
    started = int(time.time())
    includes_mainnet = package_includes_mainnet(args.scope)

    gate.add(
        "exchange package scope",
        True,
        detail=(
            "testnet-only until mainnet launch"
            if not includes_mainnet
            else "full package; mainnet readiness checks are blocking"
        ),
    )

    check_rpc_health(gate, "testnet public RPC health", TESTNET_RPC, args.max_block_age_secs)
    check_rpc_method(gate, "testnet getFeeConfig", TESTNET_RPC, "getFeeConfig")
    check_rpc_method(gate, "testnet finalized slot", TESTNET_RPC, "getSlot", [{"commitment": "finalized"}])
    check_rpc_method(gate, "testnet latest block", TESTNET_RPC, "getLatestBlock")
    if includes_mainnet:
        check_rpc_health(gate, "mainnet public RPC health", MAINNET_RPC, args.max_block_age_secs)
    else:
        gate.add(
            "mainnet public RPC health",
            True,
            blocking=False,
            detail="deferred by current testnet-only package scope; rerun with --scope full for mainnet launch",
        )

    ws_targets = [("testnet public WebSocket", TESTNET_WS)]
    if includes_mainnet:
        ws_targets.append(("mainnet public WebSocket", MAINNET_WS))
    for name, url in ws_targets:
        ok, detail = ws_upgrade(url)
        gate.add(name, ok, detail=detail)
    if not includes_mainnet:
        gate.add(
            "mainnet public WebSocket",
            True,
            blocking=False,
            detail="deferred by current testnet-only package scope; rerun with --scope full for mainnet launch",
        )

    check_http(gate, "explorer root", EXPLORER + "/", contains="Lichen Explorer")
    check_http(
        gate,
        "explorer address route",
        EXPLORER + "/address?address=7YKDTkwQWmDx9auTwhAJMVEkBdmFPeeE485dgM5fHxy",
    )
    check_http(
        gate,
        "explorer transaction route",
        EXPLORER
        + "/transaction?sig=c99c0b7f1b984cf48773080fbdc72c834431625eae8e2c340ec3d435498c4bd0",
    )
    check_http(gate, "explorer block route", EXPLORER + "/block?slot=1")
    check_http(
        gate,
        "developer exchange page",
        DEVELOPER_EXCHANGE_URL,
        contains_all=DEVELOPER_EXCHANGE_REQUIRED_SNIPPETS,
    )
    check_http(gate, "candidate monitoring page", STATUS_URL, contains="Lichen Mission Control")
    check_logo(gate)
    check_release(gate)

    gate.add(
        "operator-approved exchange status page",
        args.status_approved,
        detail="set LICHEN_EXCHANGE_STATUS_APPROVED=1 only after operator approval",
    )
    gate.add(
        "final exchange package release tag selected",
        args.release_tag_selected,
        detail="set LICHEN_EXCHANGE_RELEASE_TAG_SELECTED=1 only after the signed package tag is selected",
    )

    report = {
        "scope": args.scope,
        "started_at_unix": started,
        "completed_at_unix": int(time.time()),
        "checks": gate.checks,
        "failed": gate.failed(),
        "ok": not gate.failed(),
    }
    report_path = Path(args.report)
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")

    for check in gate.checks:
        status = "ok" if check["ok"] else "FAIL"
        print(f"{status:4} {check['name']}")
        if not check["ok"]:
            print(f"     {check['detail']}")
    print(f"report: {report_path}")
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
