#!/usr/bin/env python3
import argparse
import json
import pathlib
import urllib.error
import urllib.request
from datetime import datetime, timezone
from typing import Any, Dict, List, Optional


def rpc_call(rpc_url: str, method: str, params: List[Any]) -> Any:
    payload = json.dumps({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    }).encode("utf-8")

    request = urllib.request.Request(
        rpc_url,
        data=payload,
        headers={"Content-Type": "application/json"},
        method="POST",
    )

    with urllib.request.urlopen(request, timeout=20) as response:
        data = json.loads(response.read().decode("utf-8"))

    if "error" in data and data["error"] is not None:
        raise RuntimeError(f"RPC error for {method}: {data['error']}")

    return data.get("result")


def extract_contract_id(contract: Dict[str, Any]) -> Optional[str]:
    return (
        contract.get("program_id")
        or contract.get("address")
        or contract.get("id")
        or contract.get("contract_id")
    )


def extract_contract_name(contract: Dict[str, Any], contract_id: str) -> str:
    return (
        contract.get("name")
        or contract.get("program_name")
        or contract.get("symbol")
        or contract_id
    )


def normalize_method_names(abi_payload: Any) -> List[str]:
    if abi_payload is None:
        return []

    method_names: List[str] = []

    if isinstance(abi_payload, dict):
        if isinstance(abi_payload.get("methods"), list):
            for method in abi_payload["methods"]:
                if isinstance(method, dict) and isinstance(method.get("name"), str):
                    method_names.append(method["name"])

        if isinstance(abi_payload.get("functions"), list):
            for function in abi_payload["functions"]:
                if isinstance(function, dict) and isinstance(function.get("name"), str):
                    method_names.append(function["name"])

        if isinstance(abi_payload.get("abi"), list):
            for item in abi_payload["abi"]:
                if isinstance(item, dict):
                    item_type = item.get("type")
                    item_name = item.get("name")
                    if item_type in ("function", "method") and isinstance(item_name, str):
                        method_names.append(item_name)

    if isinstance(abi_payload, list):
        for item in abi_payload:
            if isinstance(item, dict):
                item_type = item.get("type")
                item_name = item.get("name")
                if item_type in ("function", "method") and isinstance(item_name, str):
                    method_names.append(item_name)

    return sorted(set(method_names))


def build_manifest(rpc_url: str) -> Dict[str, Any]:
    chain_status: Any = None
    try:
        chain_status = rpc_call(rpc_url, "getChainStatus", [])
    except Exception:
        chain_status = None

    contracts_result = rpc_call(rpc_url, "getAllContracts", [])
    contracts = contracts_result.get("contracts", []) if isinstance(contracts_result, dict) else []

    entries: List[Dict[str, Any]] = []
    success_count = 0
    failure_count = 0

    for contract in contracts:
        if not isinstance(contract, dict):
            continue

        contract_id = extract_contract_id(contract)
        if not contract_id:
            continue

        name = extract_contract_name(contract, contract_id)

        entry: Dict[str, Any] = {
            "contract_id": contract_id,
            "name": name,
            "source": "getAllContracts",
            "abi_methods": [],
            "abi": None,
            "error": None,
        }

        try:
            abi_result = rpc_call(rpc_url, "getContractAbi", [contract_id])
            entry["abi"] = abi_result
            entry["abi_methods"] = normalize_method_names(abi_result)
            success_count += 1
        except Exception as error:
            entry["error"] = str(error)
            failure_count += 1

        entries.append(entry)

    return {
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "rpc_url": rpc_url,
        "chain_status": chain_status,
        "contract_count": len(entries),
        "success_count": success_count,
        "failure_count": failure_count,
        "contracts": entries,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description="Export deployed contract ABIs into a machine-readable manifest")
    parser.add_argument("--rpc-url", required=True, help="JSON-RPC URL, e.g. http://localhost:8899")
    parser.add_argument(
        "--out",
        default="./artifacts/contract-abi-manifest.json",
        help="Output manifest path",
    )
    args = parser.parse_args()

    manifest = build_manifest(args.rpc_url)

    output_path = pathlib.Path(args.out)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(json.dumps(manifest, indent=2), encoding="utf-8")

    print(f"Wrote ABI manifest: {output_path}")
    print(
        f"Contracts: {manifest['contract_count']} | ABI OK: {manifest['success_count']} | ABI Failed: {manifest['failure_count']}"
    )


if __name__ == "__main__":
    main()
