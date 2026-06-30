#!/usr/bin/env python3
"""Unit tests for the exchange public-readiness gate.

These tests intentionally avoid network calls; the live gate itself remains
`scripts/qa/exchange_public_readiness.py`.
"""

from __future__ import annotations

import importlib.util
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

    def test_developer_exchange_page_check_requires_exchange_pack_links(self) -> None:
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

    def test_png_dimensions_rejects_non_png_and_reads_ihdr(self) -> None:
        png = b"\x89PNG\r\n\x1a\n" + b"\x00\x00\x00\rIHDR" + struct.pack(">II", 256, 256)
        self.assertEqual(readiness.png_dimensions(png), (256, 256))
        self.assertIsNone(readiness.png_dimensions(b"not a png"))


if __name__ == "__main__":
    unittest.main()
