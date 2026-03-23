#!/usr/bin/env python3
import argparse
import glob
import json
import os
import re
import sys
import urllib.error
import urllib.request

SOURCE_ALIAS = {
    "compute_market": "compute",
    "moss_storage": "storage",
    "dex_core": "dex-core",
    "dex_amm": "dex-amm",
    "dex_router": "dex-router",
    "dex_governance": "dex-governance",
    "dex_rewards": "dex-rewards",
    "dex_margin": "dex-margin",
    "dex_analytics": "dex-analytics",
    "prediction_market": "prediction-market",
    "lusd_token": "lusd-token",
    "weth_token": "weth-token",
    "wsol_token": "wsol-token",
}


def extract_source_exports(repo_root: str):
    exports = {}
    pattern = os.path.join(repo_root, "contracts", "*", "src", "lib.rs")
    for path in glob.glob(pattern):
        contract = os.path.basename(os.path.dirname(os.path.dirname(path)))
        with open(path, "r", encoding="utf-8") as f:
            text = f.read()
        functions = [
            m.group(1)
            for m in re.finditer(
                r"#\[no_mangle\]\s*\n\s*pub extern \"C\" fn\s+([a-zA-Z0-9_]+)\s*\(",
                text,
            )
        ]
        exports[contract] = functions
    return exports


def extract_html_live_matrix(html_path: str):
    with open(html_path, "r", encoding="utf-8") as f:
        html = f.read()

    block_match = re.search(
        r'<div[^>]*id="live-exports"[^>]*>(.*?)</div>\s*\n\s*<!-- CROSS-CONTRACT INTEGRATIONS -->',
        html,
        re.S,
    )
    if not block_match:
        raise RuntimeError("Could not find #live-exports authoritative matrix block in contract-reference.html")

    block = block_match.group(1)
    rows = re.findall(r"<tr><td>([^<]+)</td><td>([^<]+)</td></tr>", block)
    matrix = {}
    for contract, fn_text in rows:
        funcs = [x.strip() for x in fn_text.split(",") if x.strip()]
        matrix[contract.strip()] = funcs
    return matrix


def extract_skill_contracts(skill_path: str):
    with open(skill_path, "r", encoding="utf-8") as f:
        text = f.read()

    names = re.findall(r"^- `([^`]+)`:\s", text, re.M)
    return set(names)


def post_json(url: str, payload: dict):
    data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        url,
        data=data,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=20) as response:
        return json.loads(response.read().decode("utf-8"))


def check_rpc_abis(rpc_url: str):
    errors = []
    try:
        response = post_json(
            rpc_url,
            {"jsonrpc": "2.0", "id": 1, "method": "getAllContracts", "params": []},
        )
    except Exception as exc:
        return [f"RPC getAllContracts failed: {exc}"]

    contracts = (
        (response.get("result") or {}).get("contracts")
        if isinstance(response, dict)
        else None
    )

    if not isinstance(contracts, list):
        return ["RPC getAllContracts returned unexpected shape (expected result.contracts array)"]

    for entry in contracts:
        if not isinstance(entry, dict):
            continue
        contract_id = entry.get("program_id") or entry.get("address") or entry.get("id")
        if not contract_id:
            continue
        try:
            abi_resp = post_json(
                rpc_url,
                {
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "getContractAbi",
                    "params": [contract_id],
                },
            )
        except Exception as exc:
            errors.append(f"RPC getContractAbi failed for {contract_id}: {exc}")
            continue

        if abi_resp.get("error") is not None:
            errors.append(f"RPC getContractAbi returned error for {contract_id}: {abi_resp['error']}")
            continue

        result = abi_resp.get("result")
        if result in (None, {}, []):
            errors.append(f"RPC getContractAbi returned empty result for {contract_id}")

    return errors


def main():
    parser = argparse.ArgumentParser(description="Strict coverage self-test for contracts/docs/skillbook")
    parser.add_argument("--repo-root", default=os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    parser.add_argument("--rpc-url", default=None, help="Optional RPC URL for live ABI coverage checks")
    args = parser.parse_args()

    repo_root = args.repo_root
    html_path = os.path.join(repo_root, "developers", "contract-reference.html")
    skill_path = os.path.join(repo_root, "skill.md")

    source_exports = extract_source_exports(repo_root)
    html_matrix = extract_html_live_matrix(html_path)
    skill_contracts = extract_skill_contracts(skill_path)

    failures = []

    for source_contract, funcs in sorted(source_exports.items()):
        alias = SOURCE_ALIAS.get(source_contract, source_contract)

        if source_contract not in html_matrix:
            failures.append(f"contract-reference missing live matrix row for {source_contract}")
        else:
            html_funcs = html_matrix[source_contract]
            missing = [fn for fn in funcs if fn not in html_funcs]
            extra = [fn for fn in html_funcs if fn not in funcs]
            if missing:
                failures.append(
                    f"contract-reference {source_contract} missing functions: {', '.join(missing)}"
                )
            if extra:
                failures.append(
                    f"contract-reference {source_contract} has non-source functions: {', '.join(extra)}"
                )

        if source_contract not in skill_contracts and alias not in skill_contracts:
            failures.append(
                f"skill.md missing contract surface entry for {source_contract} (or alias {alias})"
            )

    if args.rpc_url:
        failures.extend(check_rpc_abis(args.rpc_url))

    if failures:
        print("COVERAGE SELF-TEST: FAIL")
        for item in failures:
            print(f"- {item}")
        sys.exit(1)

    print("COVERAGE SELF-TEST: PASS")
    print(f"- source contracts checked: {len(source_exports)}")
    print(f"- contract-reference live matrix rows: {len(html_matrix)}")
    print(f"- skill contract entries parsed: {len(skill_contracts)}")
    if args.rpc_url:
        print(f"- rpc abi checks: ok ({args.rpc_url})")


if __name__ == "__main__":
    main()
