#!/usr/bin/env python3
from __future__ import annotations

import struct
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace


ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))

import rocksdb_wal_inventory as wal  # noqa: E402


def varint32(value: int) -> bytes:
    encoded = bytearray()
    while value >= 0x80:
        encoded.append((value & 0x7F) | 0x80)
        value >>= 7
    encoded.append(value)
    return bytes(encoded)


def length_prefixed(value: bytes) -> bytes:
    return varint32(len(value)) + value


def delete_cf(column_family: int, key: bytes, *, single: bool = False) -> bytes:
    tag = 8 if single else 4
    return bytes([tag]) + varint32(column_family) + length_prefixed(key)


def full_wal_record(payload: bytes) -> bytes:
    record_type = 1
    checksum = wal.mask_crc32c(wal.crc32c(bytes([record_type]) + payload))
    return struct.pack("<IHB", checksum, len(payload), record_type) + payload


class RocksDbWalInventoryTests(unittest.TestCase):
    def test_reports_deleted_slot_ranges_and_block_rows(self) -> None:
        operations = [
            delete_cf(25, (10).to_bytes(8, "big")),
            delete_cf(25, (11).to_bytes(8, "big")),
            delete_cf(25, (15).to_bytes(8, "big")),
            delete_cf(25, b"last_slot"),
            delete_cf(3, bytes(range(32)), single=True),
        ]
        payload = struct.pack("<QI", 100, len(operations)) + b"".join(operations)

        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "000001.log"
            path.write_bytes(full_wal_record(payload))
            report = wal.inventory(
                SimpleNamespace(
                    wal=path,
                    blocks_cf=3,
                    slots_cf=25,
                    extract_jsonl=None,
                    from_slot=0,
                    to_slot=(1 << 64) - 1,
                )
            )

        self.assertEqual(report["deleted_slot_row_count"], 3)
        self.assertEqual(report["deleted_slot_row_ranges"], [[10, 11], [15, 15]])
        self.assertEqual(report["deleted_last_slot_row_count"], 1)
        self.assertEqual(report["deleted_block_row_count"], 1)
        self.assertEqual(report["operation_counts"], {"delete": 4, "single_delete": 1})

    def test_rejects_corrupted_physical_record(self) -> None:
        operation = delete_cf(25, (10).to_bytes(8, "big"))
        payload = struct.pack("<QI", 100, 1) + operation
        record = bytearray(full_wal_record(payload))
        record[-1] ^= 0x01

        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "000001.log"
            path.write_bytes(record)
            with path.open("rb") as stream:
                with self.assertRaisesRegex(wal.WalError, "CRC32C mismatch"):
                    list(wal.logical_records(stream))


if __name__ == "__main__":
    unittest.main()
