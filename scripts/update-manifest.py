#!/usr/bin/env python3
"""Regenerate deploy-manifest.json from live symbol registry.

This is an additive live-registry refresh. It does not create, delete, reset,
or rewrite DEX pairs, pools, routes, custody state, or historical chain state.
"""
import json, subprocess, sys, os, tempfile

script_dir = os.path.dirname(os.path.abspath(__file__))
root_dir = os.path.dirname(script_dir)
rpc_url = os.environ.get('LICHEN_RPC_URL') or os.environ.get('CUSTODY_LICHEN_RPC_URL') or 'http://localhost:8899'

raw = subprocess.check_output([
    'curl', '-sS', rpc_url,
    '-X', 'POST', '-H', 'Content-Type: application/json',
    '-d', json.dumps({
        'jsonrpc': '2.0', 'id': 1,
        'method': 'getAllSymbolRegistry', 'params': [100]
    })
])
d = json.loads(raw)
result = d.get('result')
if not isinstance(result, dict) or 'entries' not in result:
    print(f"ERROR: unexpected RPC response for getAllSymbolRegistry: {d}")
    sys.exit(1)
raw_entries = result['entries']
if not isinstance(raw_entries, list):
    print(f"ERROR: entries is not a list: {type(raw_entries)}")
    sys.exit(1)
entries = {}
for e in raw_entries:
    if isinstance(e, dict) and 'symbol' in e and 'program' in e:
        entries[e['symbol']] = e['program']

required_symbols = [
    'LICN', 'LUSD', 'WSOL', 'WETH', 'WBNB', 'WNEO', 'WGAS', 'WBTC',
    'DEX', 'DEXAMM', 'DEXROUTER', 'DEXMARGIN', 'DEXREWARDS', 'DEXGOV', 'ANALYTICS',
]
missing = [sym for sym in required_symbols if not entries.get(sym)]
if missing:
    print(f"ERROR: required registry symbols missing; refusing to write deploy-manifest.json: {', '.join(missing)}")
    sys.exit(1)

out_path = os.path.join(root_dir, 'deploy-manifest.json')
existing_manifest = {}
if os.path.exists(out_path):
    with open(out_path, 'r') as f:
        existing_manifest = json.load(f)
if not isinstance(existing_manifest, dict):
    existing_manifest = {}

manifest = {
    **existing_manifest,
    'deployer': entries.get('LICN', ''),  # deployer is implied by LICN owner
    'deployed_at': existing_manifest.get('deployed_at', '2026-02-19T00:00:00Z'),
    'note': 'Updated from live genesis symbol registry; additive refresh only, no chain-state mutation',
    'contracts': {
        **existing_manifest.get('contracts', {}),
        'lusd_token': entries.get('LUSD', ''),
        'wsol_token': entries.get('WSOL', ''),
        'weth_token': entries.get('WETH', ''),
        'wbnb_token': entries.get('WBNB', ''),
        'wgas_token': entries.get('WGAS', ''),
        'wneo_token': entries.get('WNEO', ''),
        'wbtc_token': entries.get('WBTC', ''),
        'dex_core': entries.get('DEX', ''),
        'dex_amm': entries.get('DEXAMM', ''),
        'dex_router': entries.get('DEXROUTER', ''),
        'dex_margin': entries.get('DEXMARGIN', ''),
        'dex_rewards': entries.get('DEXREWARDS', ''),
        'dex_governance': entries.get('DEXGOV', ''),
        'dex_analytics': entries.get('ANALYTICS', ''),
        'prediction_market': entries.get('PREDICT', ''),
        'neo_gas_rewards': entries.get('NEOGASRWD', ''),
    },
    'token_contracts': {
        **existing_manifest.get('token_contracts', {}),
        'LICN': entries.get('LICN', ''),
        'lUSD': entries.get('LUSD', ''),
        'wSOL': entries.get('WSOL', ''),
        'wETH': entries.get('WETH', ''),
        'wBNB': entries.get('WBNB', ''),
        'wGAS': entries.get('WGAS', ''),
        'wNEO': entries.get('WNEO', ''),
        'wBTC': entries.get('WBTC', ''),
        'MOSS': entries.get('MOSS', ''),
    },
    'dex_contracts': {
        **existing_manifest.get('dex_contracts', {}),
        'dex_core': entries.get('DEX', ''),
        'dex_amm': entries.get('DEXAMM', ''),
        'dex_router': entries.get('DEXROUTER', ''),
        'dex_margin': entries.get('DEXMARGIN', ''),
        'dex_rewards': entries.get('DEXREWARDS', ''),
        'dex_governance': entries.get('DEXGOV', ''),
        'dex_analytics': entries.get('ANALYTICS', ''),
        'prediction_market': entries.get('PREDICT', ''),
    },
    'product_contracts': {
        **existing_manifest.get('product_contracts', {}),
        'neo_gas_rewards': entries.get('NEOGASRWD', ''),
    },
    'trading_pairs': [
        'LICN/lUSD', 'wSOL/lUSD', 'wETH/lUSD', 'wBNB/lUSD',
        'wSOL/LICN', 'wETH/LICN', 'wBNB/LICN',
        'wNEO/lUSD', 'wNEO/LICN', 'wGAS/lUSD', 'wGAS/LICN',
        'wBTC/lUSD', 'wBTC/LICN',
    ],
}

fd, tmp_path = tempfile.mkstemp(prefix='deploy-manifest.', suffix='.json', dir=root_dir)
with os.fdopen(fd, 'w') as f:
    json.dump(manifest, f, indent=2)
    f.write('\n')
os.replace(tmp_path, out_path)

print(f'OK — wrote {out_path} from {rpc_url}')
for k, v in manifest['dex_contracts'].items():
    print(f'  {k:20s} → {v}')
for k, v in manifest['product_contracts'].items():
    print(f'  {k:20s} → {v}')
