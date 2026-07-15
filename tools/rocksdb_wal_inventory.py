#!/usr/bin/env python3
"""Read-only inventory and extraction of canonical rows from a RocksDB WAL.

This tool intentionally supports only the WriteBatch tags it can parse exactly.
It verifies physical-record CRC32C checksums, logical fragmentation, and batch
operation counts before reporting or extracting any row.
"""

from __future__ import annotations

import argparse
import base64
import json
import struct
import sys
from collections import Counter
from dataclasses import dataclass
from pathlib import Path
from typing import BinaryIO, Iterator


BLOCK_SIZE = 32 * 1024
HEADER_SIZE = 7
CRC32C_MASK_DELTA = 0xA282EAD8


def _crc32c_table() -> tuple[int, ...]:
    polynomial = 0x82F63B78
    table = []
    for value in range(256):
        crc = value
        for _ in range(8):
            crc = (crc >> 1) ^ (polynomial if crc & 1 else 0)
        table.append(crc & 0xFFFFFFFF)
    return tuple(table)


CRC32C_TABLE = _crc32c_table()


def crc32c(data: bytes) -> int:
    crc = 0xFFFFFFFF
    for byte in data:
        crc = CRC32C_TABLE[(crc ^ byte) & 0xFF] ^ (crc >> 8)
    return crc ^ 0xFFFFFFFF


def mask_crc32c(crc: int) -> int:
    rotated = ((crc >> 15) | ((crc << 17) & 0xFFFFFFFF)) & 0xFFFFFFFF
    return (rotated + CRC32C_MASK_DELTA) & 0xFFFFFFFF


class WalError(ValueError):
    pass


@dataclass(frozen=True)
class LogicalRecord:
    offset: int
    payload: bytes


def logical_records(stream: BinaryIO) -> Iterator[LogicalRecord]:
    fragmented = bytearray()
    fragmented_offset: int | None = None
    file_offset = 0

    while True:
        block = stream.read(BLOCK_SIZE)
        if not block:
            break
        block_length = len(block)
        cursor = 0
        while cursor + HEADER_SIZE <= block_length:
            header = block[cursor : cursor + HEADER_SIZE]
            stored_crc, length, record_type = struct.unpack("<IHB", header)
            record_offset = file_offset + cursor

            if stored_crc == 0 and length == 0 and record_type == 0:
                if any(block[cursor:]):
                    raise WalError(f"nonzero bytes after zero record at {record_offset}")
                break

            end = cursor + HEADER_SIZE + length
            if end > block_length:
                raise WalError(
                    f"physical record at {record_offset} crosses its WAL block boundary"
                )
            payload = block[cursor + HEADER_SIZE : end]
            computed_crc = mask_crc32c(crc32c(bytes([record_type]) + payload))
            if computed_crc != stored_crc:
                raise WalError(
                    f"CRC32C mismatch at {record_offset}: "
                    f"stored={stored_crc:08x} computed={computed_crc:08x}"
                )

            if record_type == 1:  # FULL
                if fragmented:
                    raise WalError(f"FULL record interrupted fragments at {record_offset}")
                yield LogicalRecord(record_offset, payload)
            elif record_type == 2:  # FIRST
                if fragmented:
                    raise WalError(f"FIRST record interrupted fragments at {record_offset}")
                fragmented.extend(payload)
                fragmented_offset = record_offset
            elif record_type == 3:  # MIDDLE
                if fragmented_offset is None:
                    raise WalError(f"orphan MIDDLE record at {record_offset}")
                fragmented.extend(payload)
            elif record_type == 4:  # LAST
                if fragmented_offset is None:
                    raise WalError(f"orphan LAST record at {record_offset}")
                fragmented.extend(payload)
                yield LogicalRecord(fragmented_offset, bytes(fragmented))
                fragmented.clear()
                fragmented_offset = None
            else:
                raise WalError(f"unsupported physical record type {record_type} at {record_offset}")

            cursor = end
        file_offset += block_length

    if fragmented_offset is not None:
        raise WalError(f"truncated fragmented record from offset {fragmented_offset}")


def read_varint32(data: bytes, cursor: int) -> tuple[int, int]:
    value = 0
    shift = 0
    for _ in range(5):
        if cursor >= len(data):
            raise WalError("truncated varint32")
        byte = data[cursor]
        cursor += 1
        value |= (byte & 0x7F) << shift
        if byte < 0x80:
            return value, cursor
        shift += 7
    raise WalError("varint32 exceeds five bytes")


def read_slice(data: bytes, cursor: int) -> tuple[bytes, int]:
    length, cursor = read_varint32(data, cursor)
    end = cursor + length
    if end > len(data):
        raise WalError(f"slice length {length} exceeds remaining WriteBatch bytes")
    return data[cursor:end], end


@dataclass(frozen=True)
class Operation:
    kind: str
    column_family: int
    key: bytes
    value: bytes | None = None


def parse_write_batch(payload: bytes, offset: int) -> tuple[int, list[Operation]]:
    if len(payload) < 12:
        raise WalError(f"WriteBatch at {offset} is shorter than its 12-byte header")
    sequence, expected_count = struct.unpack_from("<QI", payload, 0)
    cursor = 12
    operations: list[Operation] = []

    while cursor < len(payload):
        tag = payload[cursor]
        cursor += 1
        column_family = 0

        if tag in (4, 5, 6, 8, 14, 16):
            column_family, cursor = read_varint32(payload, cursor)

        if tag in (0, 4):
            key, cursor = read_slice(payload, cursor)
            operations.append(Operation("delete", column_family, key))
        elif tag in (1, 5):
            key, cursor = read_slice(payload, cursor)
            value, cursor = read_slice(payload, cursor)
            operations.append(Operation("put", column_family, key, value))
        elif tag in (2, 6):
            key, cursor = read_slice(payload, cursor)
            value, cursor = read_slice(payload, cursor)
            operations.append(Operation("merge", column_family, key, value))
        elif tag == 3:  # LogData does not contribute to WriteBatch::Count().
            _, cursor = read_slice(payload, cursor)
        elif tag in (7, 8):
            key, cursor = read_slice(payload, cursor)
            operations.append(Operation("single_delete", column_family, key))
        elif tag == 13:  # Noop
            operations.append(Operation("noop", 0, b""))
        elif tag in (14, 15):
            key, cursor = read_slice(payload, cursor)
            end_key, cursor = read_slice(payload, cursor)
            operations.append(Operation("range_delete", column_family, key, end_key))
        elif tag in (16, 17):
            key, cursor = read_slice(payload, cursor)
            value, cursor = read_slice(payload, cursor)
            operations.append(Operation("blob_index", column_family, key, value))
        else:
            raise WalError(
                f"unsupported WriteBatch tag {tag} at WAL offset {offset}, byte {cursor - 1}"
            )

    if len(operations) != expected_count:
        raise WalError(
            f"WriteBatch count mismatch at {offset}: "
            f"header={expected_count} parsed={len(operations)}"
        )
    return sequence, operations


def contiguous_ranges(slots: set[int]) -> list[list[int]]:
    if not slots:
        return []
    ordered = sorted(slots)
    ranges: list[list[int]] = []
    start = previous = ordered[0]
    for slot in ordered[1:]:
        if slot != previous + 1:
            ranges.append([start, previous])
            start = slot
        previous = slot
    ranges.append([start, previous])
    return ranges


def inventory(args: argparse.Namespace) -> dict[str, object]:
    op_counts: Counter[str] = Counter()
    cf_counts: Counter[int] = Counter()
    slot_rows: set[int] = set()
    deleted_slot_rows: set[int] = set()
    matched_block_slots: set[int] = set()
    last_slot_values: list[int] = []
    deleted_block_rows = 0
    deleted_last_slot_rows = 0
    physical_logical_records = 0
    batches = 0
    sequence_first: int | None = None
    sequence_last: int | None = None
    extract_handle = args.extract_jsonl.open("w", encoding="ascii") if args.extract_jsonl else None

    try:
        with args.wal.open("rb") as stream:
            for record in logical_records(stream):
                physical_logical_records += 1
                sequence, operations = parse_write_batch(record.payload, record.offset)
                batches += 1
                sequence_first = sequence if sequence_first is None else min(sequence_first, sequence)
                sequence_last = max(sequence_last or sequence, sequence + max(len(operations), 1) - 1)

                block_puts: dict[bytes, bytes] = {}
                batch_slots: dict[int, bytes] = {}
                for operation in operations:
                    op_counts[operation.kind] += 1
                    cf_counts[operation.column_family] += 1
                    if operation.kind in ("delete", "single_delete"):
                        if operation.column_family == args.blocks_cf and len(operation.key) == 32:
                            deleted_block_rows += 1
                        elif operation.column_family == args.slots_cf:
                            if len(operation.key) == 8:
                                deleted_slot_rows.add(int.from_bytes(operation.key, "big"))
                            elif operation.key == b"last_slot":
                                deleted_last_slot_rows += 1
                    if operation.kind != "put" or operation.value is None:
                        continue
                    if operation.column_family == args.blocks_cf and len(operation.key) == 32:
                        block_puts[operation.key] = operation.value
                    elif operation.column_family == args.slots_cf:
                        if len(operation.key) == 8 and len(operation.value) == 32:
                            slot = int.from_bytes(operation.key, "big")
                            batch_slots[slot] = operation.value
                            slot_rows.add(slot)
                        elif operation.key == b"last_slot" and len(operation.value) == 8:
                            last_slot_values.append(int.from_bytes(operation.value, "big"))

                for slot, block_hash in batch_slots.items():
                    block_value = block_puts.get(block_hash)
                    if block_value is None:
                        continue
                    matched_block_slots.add(slot)
                    if extract_handle and args.from_slot <= slot <= args.to_slot:
                        for category, key, value in (
                            ("blocks", block_hash, block_value),
                            ("slots", slot.to_bytes(8, "big"), block_hash),
                        ):
                            extract_handle.write(
                                json.dumps(
                                    {
                                        "sequence": sequence,
                                        "wal_offset": record.offset,
                                        "category": category,
                                        "slot": slot,
                                        "key_b64": base64.b64encode(key).decode("ascii"),
                                        "value_b64": base64.b64encode(value).decode("ascii"),
                                    },
                                    separators=(",", ":"),
                                    sort_keys=True,
                                )
                                + "\n"
                            )
    finally:
        if extract_handle:
            extract_handle.close()

    return {
        "wal": str(args.wal),
        "wal_bytes": args.wal.stat().st_size,
        "logical_records": physical_logical_records,
        "write_batches": batches,
        "sequence_first": sequence_first,
        "sequence_last": sequence_last,
        "operation_counts": dict(sorted(op_counts.items())),
        "column_family_operation_counts": {
            str(key): value for key, value in sorted(cf_counts.items())
        },
        "slot_row_count": len(slot_rows),
        "slot_row_ranges": contiguous_ranges(slot_rows),
        "deleted_slot_row_count": len(deleted_slot_rows),
        "deleted_slot_row_ranges": contiguous_ranges(deleted_slot_rows),
        "deleted_block_row_count": deleted_block_rows,
        "deleted_last_slot_row_count": deleted_last_slot_rows,
        "matched_block_count": len(matched_block_slots),
        "matched_block_slot_ranges": contiguous_ranges(matched_block_slots),
        "last_slot_value_count": len(last_slot_values),
        "last_slot_value_ranges": contiguous_ranges(set(last_slot_values)),
        "min_last_slot": min(last_slot_values) if last_slot_values else None,
        "max_last_slot": max(last_slot_values) if last_slot_values else None,
        "extract_jsonl": str(args.extract_jsonl) if args.extract_jsonl else None,
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("wal", type=Path)
    parser.add_argument("--blocks-cf", type=int, default=3)
    parser.add_argument("--slots-cf", type=int, default=25)
    parser.add_argument("--extract-jsonl", type=Path)
    parser.add_argument("--from-slot", type=int, default=0)
    parser.add_argument("--to-slot", type=int, default=(1 << 64) - 1)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    if args.from_slot > args.to_slot:
        raise WalError("--from-slot must be <= --to-slot")
    report = inventory(args)
    json.dump(report, sys.stdout, indent=2, sort_keys=True)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, WalError) as error:
        print(f"rocksdb WAL inventory failed: {error}", file=sys.stderr)
        raise SystemExit(1)
