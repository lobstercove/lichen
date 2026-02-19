#!/usr/bin/env python3
"""Regenerate deploy-manifest.json from live symbol registry."""
import json, subprocess, sys, os

script_dir = os.path.dirname(os.path.abspath(__file__))
root_dir = os.path.dirname(script_dir)

raw = subprocess.check_output([
    'curl', '-sS', 'http://localhost:8899',
    '-X', 'POST', '-H', 'Content-Type: application/json',
    '-d', json.dumps({
        'jsonrpc': '2.0', 'id': 1,
        'method': 'getAllSymbolRegistry', 'params': [100]
    })
])
d = json.loads(raw)
entries = {e['symbol']: e['program'] for e in d['result']['entries']}

manifest = {
    'deployer': entries.get('MOLT', ''),  # deployer is implied by MOLT owner
    'deployed_at': '2026-02-19T00:00:00Z',
    'note': 'Updated from live genesis symbol registry',
    'contracts': {
        'musd_token': entries.get('MUSD', ''),
        'wsol_token': entries.get('WSOL', ''),
        'weth_token': entries.get('WETH', ''),
        'dex_core': entries.get('DEX', ''),
        'dex_amm': entries.get('DEXAMM', ''),
        'dex_router': entries.get('DEXROUTER', ''),
        'dex_margin': entries.get('DEXMARGIN', ''),
        'dex_rewards': entries.get('DEXREWARDS', ''),
        'dex_governance': entries.get('DEXGOV', ''),
        'dex_analytics': entries.get('ANALYTICS', ''),
        'prediction_market': entries.get('PREDICT', ''),
    },
    'token_contracts': {
        'MOLT': entries.get('MOLT', ''),
        'mUSD': entries.get('MUSD', ''),
        'wSOL': entries.get('WSOL', ''),
        'wETH': entries.get('WETH', ''),
        'REEF': entries.get('REEF', ''),
    },
    'dex_contracts': {
        'dex_core': entries.get('DEX', ''),
        'dex_amm': entries.get('DEXAMM', ''),
        'dex_router': entries.get('DEXROUTER', ''),
        'dex_margin': entries.get('DEXMARGIN', ''),
        'dex_rewards': entries.get('DEXREWARDS', ''),
        'dex_governance': entries.get('DEXGOV', ''),
        'dex_analytics': entries.get('ANALYTICS', ''),
        'prediction_market': entries.get('PREDICT', ''),
    },
    'trading_pairs': [
        'MOLT/mUSD', 'wSOL/mUSD', 'wETH/mUSD', 'REEF/mUSD',
        'wSOL/MOLT', 'wETH/MOLT', 'REEF/MOLT',
    ],
}

out_path = os.path.join(root_dir, 'deploy-manifest.json')
with open(out_path, 'w') as f:
    json.dump(manifest, f, indent=2)
    f.write('\n')

print(f'OK — wrote {out_path}')
for k, v in manifest['dex_contracts'].items():
    print(f'  {k:20s} → {v}')
