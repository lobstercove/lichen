#!/usr/bin/env python3
import argparse
import json
import re
import sys
from pathlib import Path
from typing import List, Set

ROOT = Path(__file__).resolve().parent.parent
GENESIS_LIB = ROOT / "genesis" / "src" / "lib.rs"
DEFAULT_OUTPUT = ROOT / "tests" / "expected-contracts.json"


def discover_contracts() -> List[str]:
    if not GENESIS_LIB.exists():
        return []
    try:
        source = GENESIS_LIB.read_text(encoding="utf-8")
    except Exception:
        return []

    catalog_match = re.search(
        r"pub const GENESIS_CONTRACT_CATALOG:\s*&\[\(&str,\s*&str,\s*&str,\s*&str\)\]\s*=\s*&\[(.*?)\];",
        source,
        re.DOTALL,
    )
    if not catalog_match:
        return []

    discovered: List[str] = []
    pattern = re.compile(r'\(\s*"([^"]+)"\s*,\s*"[^"]+"\s*,\s*"[^"]+"\s*,\s*"[^"]+"\s*\)')
    for name in pattern.findall(catalog_match.group(1)):
        if name not in discovered:
            discovered.append(name)
    return sorted(discovered)


def load_existing(path: Path) -> List[str]:
    if not path.exists():
        return []
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
    except Exception:
        return []
    if isinstance(raw, dict) and isinstance(raw.get("contracts"), list):
        return sorted(str(v) for v in raw["contracts"] if isinstance(v, str))
    if isinstance(raw, list):
        return sorted(str(v) for v in raw if isinstance(v, str))
    return []


def diff_sets(existing: List[str], discovered: List[str]) -> tuple[List[str], List[str]]:
    existing_set: Set[str] = set(existing)
    discovered_set: Set[str] = set(discovered)
    missing = sorted(discovered_set - existing_set)
    stale = sorted(existing_set - discovered_set)
    return missing, stale


def write_output(path: Path, contracts: List[str]) -> None:
    payload = {"contracts": contracts}
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser(description="Regenerate tests/expected-contracts.json from the genesis contract catalog")
    parser.add_argument("--output", default=str(DEFAULT_OUTPUT), help="Output JSON path")
    parser.add_argument("--write", action="store_true", help="Write updated lockfile")
    parser.add_argument("--check", action="store_true", help="Exit non-zero if lockfile is out of date")
    parser.add_argument("--names-only", action="store_true", help="Print discovered contract names, one per line")
    args = parser.parse_args()

    out_path = Path(args.output).resolve()
    discovered = discover_contracts()

    if args.names_only:
        for name in discovered:
            print(name)
        return 0

    existing = load_existing(out_path)
    missing, stale = diff_sets(existing, discovered)

    print(f"Discovered contracts: {len(discovered)}")
    print(f"Lockfile contracts:   {len(existing)}")

    if missing:
        print("\nMissing from lockfile (add):")
        for name in missing:
            print(f"  + {name}")
    if stale:
        print("\nStale in lockfile (remove):")
        for name in stale:
            print(f"  - {name}")
    if not missing and not stale:
        print("\nLockfile is up to date.")

    if args.write:
        write_output(out_path, discovered)
        print(f"\nWrote {out_path}")

    if args.check and (missing or stale):
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
