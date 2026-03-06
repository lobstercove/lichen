#!/usr/bin/env python3
"""Send BNB and ETH from custody treasury to deposit address for e2e testing."""

import hmac, hashlib, json, urllib.request, sys

MASTER_SEED = "a5102901e958e2207dbc40d5bcf2e6206e2ebd3374c52f5a6f0bf78e412ae079"
DEPOSIT_ADDR = "0xc3ab72fcbfa21c3db09e1cb45d91da5f74d7d65d"

CHAINS = {
    "bnb": {
        "rpc": "https://data-seed-prebsc-1-s1.bnbchain.org:8545",
        "path": "custody/treasury/bnb",
        "amount": 0.001,  # 0.001 BNB
    },
    "eth": {
        "rpc": "https://sepolia.gateway.tenderly.co",
        "path": "custody/treasury/ethereum",
        "amount": 0.001,  # 0.001 ETH
    },
}

def rpc_call(url, method, params=[]):
    payload = json.dumps({"jsonrpc": "2.0", "method": method, "params": params, "id": 1}).encode()
    req = urllib.request.Request(url, data=payload, headers={"Content-Type": "application/json"})
    resp = urllib.request.urlopen(req, timeout=30)
    data = json.loads(resp.read())
    if "error" in data:
        raise Exception(f"RPC error: {data['error']}")
    return data["result"]

def send_for_chain(chain_name, cfg):
    from eth_account import Account

    # Derive private key
    h = hmac.new(MASTER_SEED.encode(), cfg["path"].encode(), hashlib.sha256)
    privkey = "0x" + h.hexdigest()
    acct = Account.from_key(privkey)
    
    print(f"\n{'='*60}")
    print(f"Chain: {chain_name.upper()}")
    print(f"Treasury: {acct.address}")
    
    rpc = cfg["rpc"]
    
    # Check balance
    bal = int(rpc_call(rpc, "eth_getBalance", [acct.address, "latest"]), 16)
    print(f"Balance: {bal / 1e18:.6f} {chain_name.upper()} ({bal} wei)")
    
    amount_wei = int(cfg["amount"] * 1e18)
    print(f"Sending: {cfg['amount']} {chain_name.upper()} to {DEPOSIT_ADDR}")
    
    if bal < amount_wei + 21000 * 10**10:  # rough gas estimate
        print(f"ERROR: Insufficient balance!")
        return False
    
    nonce = int(rpc_call(rpc, "eth_getTransactionCount", [acct.address, "latest"]), 16)
    gas_price = int(rpc_call(rpc, "eth_gasPrice"), 16)
    chain_id = int(rpc_call(rpc, "eth_chainId"), 16)
    
    print(f"Nonce: {nonce}, Gas: {gas_price} wei, ChainID: {chain_id}")
    
    # Checksum the deposit address
    from eth_utils import to_checksum_address
    to_addr = to_checksum_address(DEPOSIT_ADDR)
    
    tx = {
        "to": to_addr,
        "value": amount_wei,
        "gas": 21000,
        "gasPrice": gas_price,
        "nonce": nonce,
        "chainId": chain_id,
    }
    
    signed = acct.sign_transaction(tx)
    raw_hex = "0x" + signed.raw_transaction.hex()
    
    tx_hash = rpc_call(rpc, "eth_sendRawTransaction", [raw_hex])
    print(f"TX hash: {tx_hash}")
    return True

if __name__ == "__main__":
    chains_to_send = sys.argv[1:] if len(sys.argv) > 1 else ["bnb", "eth"]
    for chain in chains_to_send:
        if chain in CHAINS:
            try:
                send_for_chain(chain, CHAINS[chain])
            except Exception as e:
                print(f"ERROR on {chain}: {e}")
        else:
            print(f"Unknown chain: {chain}")
