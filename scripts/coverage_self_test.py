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
    "wbnb_token": "wbnb-token",
    "wbtc_token": "wbtc-token",
    "wgas_token": "wgas-token",
    "wneo_token": "wneo-token",
    "neo_gas_rewards": "neo-gas-rewards",
    "shielded_pool": "shielded-pool",
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
                r"#\[no_mangle\](?:\s*#\[[^\]]+\][^\n]*)*\s*pub extern \"C\" fn\s+([a-zA-Z0-9_]+)\s*\(",
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
    # The live-export table is followed in the same visual section by a
    # separate ABI-opcode completion table. Contracts can appear in both; do
    # not let the later documentation-only row overwrite the authoritative
    # named-WASM-export row.
    block = block.split("<h3>Opcode-dispatched", 1)[0]
    rows = re.findall(
        r"<tr>\s*<td>\s*([^<]+?)\s*</td>\s*<td>\s*(.*?)\s*</td>\s*</tr>",
        block,
        re.S,
    )
    matrix = {}
    for contract, fn_text in rows:
        plain_text = re.sub(r"<[^>]+>", "", fn_text)
        funcs = [re.sub(r"\s+", " ", x.strip()) for x in plain_text.split(",") if x.strip()]
        matrix[contract.strip()] = funcs
    return matrix


def extract_html_card_functions(html_path: str):
    with open(html_path, "r", encoding="utf-8") as f:
        html = f.read()

    starts = list(
        re.finditer(r'<div\s+class="contract-section"\s+id="([^"]+)"[^>]*>', html)
    )
    cards = {}
    for index, match in enumerate(starts):
        end = starts[index + 1].start() if index + 1 < len(starts) else len(html)
        block = html[match.end():end]
        functions = re.findall(r'<span\s+class="fn-chip">\s*([^<]+?)\s*</span>', block)
        cards[match.group(1)] = [re.sub(r"\s+", " ", fn.strip()) for fn in functions]
    return cards


def extract_abi_functions(repo_root: str):
    functions = {}
    pattern = os.path.join(repo_root, "contracts", "*", "abi.json")
    for path in glob.glob(pattern):
        contract = os.path.basename(os.path.dirname(path))
        with open(path, "r", encoding="utf-8") as f:
            abi = json.load(f)
        functions[contract] = {
            entry.get("name")
            for entry in abi.get("functions", [])
            if isinstance(entry, dict) and isinstance(entry.get("name"), str)
        }
    return functions


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
    parser = argparse.ArgumentParser(description="Strict coverage self-test for contract source and public docs")
    parser.add_argument("--repo-root", default=os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    parser.add_argument("--rpc-url", default=None, help="Optional RPC URL for live ABI coverage checks")
    parser.add_argument(
        "--skill-path",
        default=None,
        help="Optional private operator skillbook to compare with source; public clones do not ship one",
    )
    args = parser.parse_args()

    repo_root = args.repo_root
    html_path = os.path.join(repo_root, "developers", "contract-reference.html")

    source_exports = extract_source_exports(repo_root)
    html_matrix = extract_html_live_matrix(html_path)
    html_cards = extract_html_card_functions(html_path)
    abi_functions = extract_abi_functions(repo_root)
    skill_contracts = extract_skill_contracts(args.skill_path) if args.skill_path else None

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

        card_id = SOURCE_ALIAS.get(source_contract, source_contract)
        if card_id in html_cards:
            callable_functions = set(funcs) | abi_functions.get(source_contract, set())
            non_callable = [fn for fn in html_cards[card_id] if fn not in callable_functions]
            if non_callable:
                failures.append(
                    f"contract-reference card {card_id} has non-callable function chips: "
                    f"{', '.join(non_callable)}"
                )

        if skill_contracts is not None and source_contract not in skill_contracts and alias not in skill_contracts:
            failures.append(
                f"skillbook missing contract surface entry for {source_contract} (or alias {alias})"
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
    if skill_contracts is not None:
        print(f"- optional skill contract entries parsed: {len(skill_contracts)}")
    else:
        print("- optional private skillbook check: skipped")
    if args.rpc_url:
        print(f"- rpc abi checks: ok ({args.rpc_url})")


if __name__ == "__main__":
    main()
