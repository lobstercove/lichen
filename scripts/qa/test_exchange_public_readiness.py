#!/usr/bin/env python3
"""Unit tests for the exchange public-readiness gate.

These tests intentionally avoid network calls; the live gate itself remains
`scripts/qa/exchange_public_readiness.py`.
"""

from __future__ import annotations

import importlib.util
import os
import struct
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "qa" / "exchange_public_readiness.py"


def load_readiness_module():
    spec = importlib.util.spec_from_file_location("exchange_public_readiness", SCRIPT)
    if spec is None or spec.loader is None:
        raise RuntimeError("failed to load exchange_public_readiness.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


readiness = load_readiness_module()


class ExchangePublicReadinessTests(unittest.TestCase):
    def test_rpc_health_ready_requires_ok_and_fresh_tip(self) -> None:
        self.assertTrue(
            readiness.rpc_health_ready(
                {"status": "ok", "block_age_secs": 2, "slot": 42},
                max_block_age_secs=15,
            )
        )
        self.assertFalse(
            readiness.rpc_health_ready(
                {"status": "behind", "reason": "stale_tip", "block_age_secs": 2, "slot": 42},
                max_block_age_secs=15,
            )
        )
        self.assertFalse(
            readiness.rpc_health_ready(
                {"status": "ok", "block_age_secs": 16, "slot": 42},
                max_block_age_secs=15,
            )
        )
        self.assertFalse(
            readiness.rpc_health_ready(
                {"status": "ok", "block_age_secs": "2", "slot": 42},
                max_block_age_secs=15,
            )
        )

    def test_developer_exchange_page_check_requires_inline_exchange_cookbook(self) -> None:
        original_request_bytes = readiness.request_bytes

        def fake_generic_page(_url: str):
            return 200, {"content-type": "text/html"}, b"<html>Lichen Developer Hub</html>"

        readiness.request_bytes = fake_generic_page
        try:
            gate = readiness.Gate()
            readiness.check_http(
                gate,
                "developer exchange page",
                readiness.DEVELOPER_EXCHANGE_URL,
                contains_all=readiness.DEVELOPER_EXCHANGE_REQUIRED_SNIPPETS,
            )
            self.assertFalse(gate.checks[0]["ok"])
            self.assertIn("Exchange Integration", gate.checks[0]["detail"]["missing"])
            self.assertIn("Exchange Operations Pack", gate.checks[0]["detail"]["missing"])
            self.assertIn("Deposit Cookbook", gate.checks[0]["detail"]["missing"])
            self.assertIn("Withdrawal Cookbook", gate.checks[0]["detail"]["missing"])
            self.assertIn("Mainnet Handoff", gate.checks[0]["detail"]["missing"])
            self.assertIn("testnet-only", gate.checks[0]["detail"]["missing"])
        finally:
            readiness.request_bytes = original_request_bytes

    def test_developer_exchange_page_check_accepts_complete_page(self) -> None:
        original_request_bytes = readiness.request_bytes
        body = "\n".join(readiness.DEVELOPER_EXCHANGE_REQUIRED_SNIPPETS).encode("utf-8")

        def fake_exchange_page(_url: str):
            return 200, {"content-type": "text/html"}, body

        readiness.request_bytes = fake_exchange_page
        try:
            gate = readiness.Gate()
            readiness.check_http(
                gate,
                "developer exchange page",
                readiness.DEVELOPER_EXCHANGE_URL,
                contains_all=readiness.DEVELOPER_EXCHANGE_REQUIRED_SNIPPETS,
            )
            self.assertTrue(gate.checks[0]["ok"])
            self.assertEqual(gate.checks[0]["detail"]["missing"], [])
        finally:
            readiness.request_bytes = original_request_bytes

    def test_developer_exchange_page_check_rejects_admin_monitoring_host(self) -> None:
        original_request_bytes = readiness.request_bytes
        body = (
            "\n".join(readiness.DEVELOPER_EXCHANGE_REQUIRED_SNIPPETS)
            + f"\nhttps://{readiness.ADMIN_MONITORING_HOST}"
        ).encode("utf-8")

        def fake_exchange_page(_url: str):
            return 200, {"content-type": "text/html"}, body

        readiness.request_bytes = fake_exchange_page
        try:
            gate = readiness.Gate()
            readiness.check_http(
                gate,
                "developer exchange page",
                readiness.DEVELOPER_EXCHANGE_URL,
                contains_all=readiness.DEVELOPER_EXCHANGE_REQUIRED_SNIPPETS,
                forbidden_all=readiness.DEVELOPER_EXCHANGE_FORBIDDEN_SNIPPETS,
            )
            self.assertFalse(gate.checks[0]["ok"])
            self.assertIn(readiness.ADMIN_MONITORING_HOST, gate.checks[0]["detail"]["forbidden"])
        finally:
            readiness.request_bytes = original_request_bytes

    def test_exchange_status_page_rejects_missing_url(self) -> None:
        gate = readiness.Gate()
        readiness.check_exchange_status_page(gate, "")
        self.assertFalse(gate.checks[0]["ok"])
        self.assertIn("missing public exchange status URL", gate.checks[0]["detail"])
        self.assertEqual(len(gate.checks), 1)

    def test_exchange_status_page_rejects_admin_monitoring_url(self) -> None:
        gate = readiness.Gate()
        readiness.check_exchange_status_page(gate, f"https://{readiness.ADMIN_MONITORING_HOST}")
        self.assertFalse(gate.checks[0]["ok"])
        self.assertIn("admin-only", gate.checks[0]["detail"])
        self.assertEqual(len(gate.checks), 1)

    def test_exchange_status_page_accepts_public_status_page(self) -> None:
        original_request_bytes = readiness.request_bytes
        body = "\n".join(readiness.STATUS_REQUIRED_SNIPPETS).encode("utf-8")

        def fake_status_page(_url: str):
            return 200, {"content-type": "text/html"}, body

        readiness.request_bytes = fake_status_page
        try:
            gate = readiness.Gate()
            readiness.check_exchange_status_page(gate, "https://status.lichen.network/exchange")
            self.assertTrue(gate.checks[0]["ok"])
            self.assertTrue(gate.checks[1]["ok"])
            self.assertTrue(gate.checks[2]["ok"])
        finally:
            readiness.request_bytes = original_request_bytes

    def test_exchange_status_page_accepts_cloudflare_protected_contact_email(self) -> None:
        original_request_bytes = readiness.request_bytes

        def cfemail(value: str, key: int = 0x2A) -> str:
            encoded = bytes([key]) + bytes(byte ^ key for byte in value.encode("utf-8"))
            return encoded.hex()

        snippets = [
            snippet
            for snippet in readiness.STATUS_REQUIRED_SNIPPETS
            if snippet != "exchange-ops@lichen.network"
        ]
        body = (
            "\n".join(snippets)
            + f'\n<span class="__cf_email__" data-cfemail="{cfemail("exchange-ops@lichen.network")}">[email&#160;protected]</span>'
        ).encode("utf-8")

        def fake_status_page(_url: str):
            return 200, {"content-type": "text/html"}, body

        readiness.request_bytes = fake_status_page
        try:
            gate = readiness.Gate()
            readiness.check_exchange_status_page(gate, "https://exchanges.lichen.network")
            self.assertTrue(gate.checks[0]["ok"])
            self.assertTrue(gate.checks[1]["ok"])
            self.assertTrue(gate.checks[2]["ok"])
        finally:
            readiness.request_bytes = original_request_bytes

    def test_exchange_status_page_rejects_admin_monitoring_content(self) -> None:
        original_request_bytes = readiness.request_bytes

        def fake_admin_page(_url: str):
            return (
                200,
                {"content-type": "text/html"},
                (
                    "\n".join(readiness.STATUS_REQUIRED_SNIPPETS)
                    + "\nLichen Mission Control - Network Monitoring"
                ).encode("utf-8"),
            )

        readiness.request_bytes = fake_admin_page
        try:
            gate = readiness.Gate()
            readiness.check_exchange_status_page(gate, "https://status.lichen.network/exchange")
            self.assertTrue(gate.checks[0]["ok"])
            self.assertTrue(gate.checks[1]["ok"])
            self.assertFalse(gate.checks[2]["ok"])
            self.assertIn("Lichen Mission Control", gate.checks[2]["detail"]["forbidden_snippets"])
            self.assertIn("Network Monitoring", gate.checks[2]["detail"]["forbidden_snippets"])
        finally:
            readiness.request_bytes = original_request_bytes

    def test_png_dimensions_rejects_non_png_and_reads_ihdr(self) -> None:
        png = b"\x89PNG\r\n\x1a\n" + b"\x00\x00\x00\rIHDR" + struct.pack(">II", 256, 256)
        self.assertEqual(readiness.png_dimensions(png), (256, 256))
        self.assertIsNone(readiness.png_dimensions(b"not a png"))

    def test_scope_controls_mainnet_readiness(self) -> None:
        self.assertFalse(readiness.package_includes_mainnet("testnet"))
        self.assertTrue(readiness.package_includes_mainnet("full"))
        self.assertIn(readiness.default_package_scope(), readiness.PACKAGE_SCOPES)

    def test_default_exchange_status_url_uses_exchange_subdomain(self) -> None:
        self.assertEqual("https://exchanges.lichen.network", readiness.EXCHANGE_STATUS_URL)
        expected = os.environ.get("LICHEN_EXCHANGE_STATUS_URL", readiness.EXCHANGE_STATUS_URL)
        self.assertEqual(expected, readiness.DEFAULT_STATUS_URL)

    def test_exchange_package_release_requires_published_assets(self) -> None:
        original_request_json = readiness.request_json

        def fake_missing_asset(_url: str):
            return 200, {
                "tag_name": readiness.EXCHANGE_PACKAGE_TAG,
                "draft": False,
                "prerelease": False,
                "assets": [{"name": "SHA256SUMS"}],
            }

        readiness.request_json = fake_missing_asset
        try:
            gate = readiness.Gate()
            readiness.check_exchange_package_release(gate)
            self.assertFalse(gate.checks[0]["ok"])
            self.assertIn(
                "lichen-exchange-testnet-v0.5.221.tar.gz",
                gate.checks[0]["detail"]["required_assets"],
            )
        finally:
            readiness.request_json = original_request_json

    def test_exchange_package_release_accepts_published_package(self) -> None:
        original_request_json = readiness.request_json

        def fake_release(_url: str):
            return 200, {
                "tag_name": readiness.EXCHANGE_PACKAGE_TAG,
                "draft": False,
                "prerelease": False,
                "assets": [{"name": name} for name in readiness.EXCHANGE_PACKAGE_REQUIRED_ASSETS],
            }

        readiness.request_json = fake_release
        try:
            gate = readiness.Gate()
            readiness.check_exchange_package_release(gate)
            self.assertTrue(gate.checks[0]["ok"])
            self.assertEqual(
                sorted(readiness.EXCHANGE_PACKAGE_REQUIRED_ASSETS),
                gate.checks[0]["detail"]["required_assets"],
            )
        finally:
            readiness.request_json = original_request_json


if __name__ == "__main__":
    unittest.main()
