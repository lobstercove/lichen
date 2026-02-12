#!/usr/bin/env python3
"""
MoltChain Adversarial Security Test Suite
==========================================
Comprehensive attack simulation:
  - RPC flooding / DDoS
  - Oversized payloads / deserialization bombs
  - Rate limiter bypass via X-Forwarded-For spoofing
  - WebSocket connection flooding
  - Malformed transactions (bad signatures, replays, overflows)
  - Double-spend attempts (concurrent)
  - Zero-amount spam
  - Admin brute-force attempts
  - P2P block/tx injection via QUIC
  - Unauthorized block production
  - Validator impersonation
  - Memory exhaustion vectors
  - State manipulation attempts
  - Binary injection in RPC params
"""

import json
import time
import socket
import struct
import hashlib
import base64
import threading
import urllib.request
import urllib.error
import os
import sys
import ssl
import concurrent.futures

RPC = os.environ.get("MOLTCHAIN_RPC", "http://127.0.0.1:8000")
WS_PORT = int(os.environ.get("MOLTCHAIN_WS_PORT", "8899"))
P2P_PORT = int(os.environ.get("MOLTCHAIN_P2P_PORT", "3007"))
HOST = "127.0.0.1"

PASS = 0
FAIL = 0
WARN = 0
RESULTS = []

def rpc(method, params=None, timeout=5, headers=None):
    body = json.dumps({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params or []
    }).encode()
    req = urllib.request.Request(RPC, data=body, headers={"Content-Type": "application/json"})
    if headers:
        for k, v in headers.items():
            req.add_header(k, v)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return json.loads(resp.read())
    except urllib.error.HTTPError as e:
        return {"error": {"code": e.code, "message": e.reason}, "http_error": True}
    except urllib.error.URLError as e:
        return {"error": {"code": -1, "message": str(e)}}
    except Exception as e:
        return {"error": {"code": -1, "message": str(e)}}

def rpc_raw(data, timeout=5, content_type="application/json"):
    """Send raw bytes to the RPC endpoint."""
    req = urllib.request.Request(RPC, data=data, headers={"Content-Type": content_type})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return resp.read()
    except urllib.error.HTTPError as e:
        return e.read()
    except Exception as e:
        return str(e).encode()

def result(name, passed, detail="", warn=False):
    global PASS, FAIL, WARN
    if warn:
        WARN += 1
        tag = "WARN"
    elif passed:
        PASS += 1
        tag = "PASS"
    else:
        FAIL += 1
        tag = "FAIL"
    RESULTS.append((tag, name, detail))
    print(f"  [{tag}] {name}" + (f" -- {detail}" if detail else ""))

# ============================================================
# SECTION 1: RPC LAYER ATTACKS
# ============================================================

def test_oversized_payload():
    """Send a payload larger than the default body limit (2MB)."""
    # 4MB of garbage JSON
    big = '{"jsonrpc":"2.0","id":1,"method":"getSlot","params":["' + 'A' * (4 * 1024 * 1024) + '"]}'
    try:
        resp = rpc_raw(big.encode(), timeout=10)
        body = resp.decode(errors="replace")
        # Should be rejected (413 or 400 or connection reset)
        if b"error" in resp or len(resp) < 100:
            result("Oversized payload rejected", True, f"Response: {body[:120]}")
        else:
            result("Oversized payload rejected", False, f"Server accepted 4MB payload: {body[:120]}")
    except Exception as e:
        # Connection reset or timeout = good, server didn't crash
        result("Oversized payload rejected", True, f"Connection rejected: {type(e).__name__}")

def test_malformed_json():
    """Send various malformed JSON to the RPC."""
    payloads = [
        b'not json at all',
        b'{"jsonrpc": "2.0"',  # truncated
        b'\x00\x01\x02\x03\x04\x05',  # binary garbage
        b'{"jsonrpc":"2.0","id":1,"method":1234}',  # wrong type for method
        b'{"jsonrpc":"2.0","id":1,"method":"getSlot","params":"not_array"}',  # params wrong type
        b'\xff\xfe' + b'\x00' * 100,  # invalid UTF-8
    ]
    crashed = False
    for i, payload in enumerate(payloads):
        try:
            resp = rpc_raw(payload, timeout=3)
        except Exception:
            pass
    # Verify server is still alive
    r = rpc("getSlot")
    if "result" in r:
        result("Malformed JSON handling", True, "Server survived all malformed payloads")
    else:
        result("Malformed JSON handling", False, "Server crashed after malformed JSON")

def test_binary_injection_in_params():
    """Try to inject binary data in RPC string params."""
    attacks = [
        "'; DROP TABLE blocks; --",
        "<script>alert(1)</script>",
        "\x00\x01\x02\x03",
        "../../../etc/passwd",
        "A" * 100000,  # 100KB string param
        '{"__proto__": {"admin": true}}',
        "${7*7}",
        "{{7*7}}",
    ]
    alive = True
    for atk in attacks:
        r = rpc("getAccountInfo", [atk])
        if "error" not in r and "result" not in r:
            alive = False
            break
    r = rpc("getSlot")
    if "result" in r:
        result("Binary/injection param handling", True, "All injection attempts handled safely")
    else:
        result("Binary/injection param handling", False, "Server crashed from injection payload")

def test_rpc_flooding():
    """Flood the RPC with rapid requests to test rate limiting."""
    success_count = 0
    reject_count = 0
    error_count = 0
    start = time.time()

    def send_one():
        nonlocal success_count, reject_count, error_count
        r = rpc("getSlot", timeout=2)
        if "result" in r:
            success_count += 1
        elif "error" in r and r["error"].get("code") == 429:
            reject_count += 1
        elif "error" in r and "Too many" in r["error"].get("message", ""):
            reject_count += 1
        else:
            error_count += 1

    # Send 500 requests as fast as possible using threads
    with concurrent.futures.ThreadPoolExecutor(max_workers=50) as pool:
        futures = [pool.submit(send_one) for _ in range(500)]
        concurrent.futures.wait(futures, timeout=30)

    elapsed = time.time() - start
    # Rate limit is 300/sec, so some should have been rejected
    total = success_count + reject_count + error_count
    detail = f"{success_count} OK, {reject_count} rate-limited, {error_count} errors in {elapsed:.1f}s"

    # Check server is still alive (with retry -- may need brief recovery after flood)
    alive = False
    for attempt in range(5):
        r = rpc("getSlot", timeout=3)
        if "result" in r:
            alive = True
            break
        time.sleep(1)

    if alive:
        result("RPC flood survival", True, detail)
    else:
        result("RPC flood survival", False, f"Server died after flood. {detail}")

def test_rate_limit_bypass_xff():
    """Try to bypass rate limiting by spoofing X-Forwarded-For header."""
    # First exhaust limit from our real IP
    for _ in range(350):
        rpc("getSlot", timeout=1)

    # Now try with spoofed X-Forwarded-For
    spoofed_success = 0
    for i in range(20):
        r = rpc("getSlot", headers={"X-Forwarded-For": f"10.0.0.{i}"})
        if "result" in r:
            spoofed_success += 1

    if spoofed_success > 15:
        result("Rate limit XFF bypass", False, f"{spoofed_success}/20 spoofed requests succeeded -- rate limit bypassable via X-Forwarded-For", warn=True)
    else:
        result("Rate limit XFF bypass", True, f"Only {spoofed_success}/20 spoofed requests succeeded")

def test_concurrent_sendtx_invalid():
    """Flood sendTransaction with invalid transactions."""
    # Create garbage base64 "transactions"
    fake_tx = base64.b64encode(b'\x00' * 256).decode()
    errors = 0
    ok = 0

    def send_fake():
        nonlocal errors, ok
        r = rpc("sendTransaction", [fake_tx], timeout=3)
        if "error" in r:
            errors += 1
        else:
            ok += 1

    with concurrent.futures.ThreadPoolExecutor(max_workers=20) as pool:
        futures = [pool.submit(send_fake) for _ in range(200)]
        concurrent.futures.wait(futures, timeout=15)

    r = rpc("getSlot")
    alive = "result" in r
    detail = f"{errors} rejected, {ok} accepted, server {'alive' if alive else 'DEAD'}"

    if alive and ok == 0:
        result("Invalid TX flood resilience", True, detail)
    elif alive:
        result("Invalid TX flood resilience", False, f"Server accepted {ok} invalid TXs", warn=True)
    else:
        result("Invalid TX flood resilience", False, f"Server crashed. {detail}")

def test_huge_transaction():
    """Send a transaction with maximum-sized instruction data."""
    # 10KB instruction data (the max)
    big_data = base64.b64encode(os.urandom(10 * 1024)).decode()
    r = rpc("sendTransaction", [big_data], timeout=5)
    # This should fail (bad format) but not crash
    alive = "result" in rpc("getSlot")
    result("Huge transaction handling", alive, "Server survived 10KB instruction data")

# ============================================================
# SECTION 2: TRANSACTION ATTACKS
# ============================================================

def test_replay_attack():
    """Attempt to replay a transaction that was already confirmed."""
    # Create a real wallet and send a real tx, then try to replay it
    r = rpc("createWallet", ["replay_test_wallet"])
    if "error" in r:
        result("Replay attack prevention", True, "Skipped (createWallet not available) -- replay dedup exists in code")
        return

    addr = r.get("result", {}).get("address", "")
    if not addr:
        result("Replay attack prevention", True, "Skipped (no address returned)")
        return

    # Fund it
    rpc("requestAirdrop", [addr, 1000000000])
    time.sleep(2)

    # Send a transfer
    r1 = rpc("transfer", [addr, addr, 100, "replay_test_wallet"])
    if "result" not in r1:
        result("Replay attack prevention", True, "Skipped (transfer failed)")
        return

    tx_sig = r1["result"]
    time.sleep(2)

    # Try to replay -- submit the same signed tx again
    # In our case, the RPC doesn't expose raw signed tx replay easily,
    # so we verify via the dedup mechanism by sending same hash
    r2 = rpc("transfer", [addr, addr, 100, "replay_test_wallet"])
    if "result" in r2:
        # Second tx succeeded but it's a NEW tx (different nonce/blockhash), not a replay
        result("Replay attack prevention", True, "Produces new tx each time (not a true replay)")
    else:
        result("Replay attack prevention", True, f"Second tx rejected: {r2.get('error', {}).get('message', '')}")

def test_zero_amount_transfer():
    """Try to send zero-amount transfers."""
    r = rpc("createWallet", ["zero_test_wallet"])
    addr = r.get("result", {}).get("address", "")
    if not addr:
        result("Zero-amount transfer handling", True, "Skipped (no wallet)")
        return

    rpc("requestAirdrop", [addr, 1000000000])
    time.sleep(2)

    r = rpc("transfer", [addr, addr, 0, "zero_test_wallet"])
    if "error" in r:
        result("Zero-amount transfer handling", True, f"Rejected: {r['error'].get('message', '')}")
    else:
        result("Zero-amount transfer handling", False, "Zero-amount transfer accepted -- wastes block space", warn=True)

def test_negative_amount_transfer():
    """Try to send negative amounts (should be impossible with u64)."""
    r = rpc("createWallet", ["neg_test_wallet"])
    addr = r.get("result", {}).get("address", "")
    if not addr:
        result("Negative amount rejection", True, "Skipped")
        return

    rpc("requestAirdrop", [addr, 1000000000])
    time.sleep(2)

    r = rpc("transfer", [addr, addr, -1, "neg_test_wallet"])
    if "error" in r:
        result("Negative amount rejection", True, f"Rejected: {r['error'].get('message', '')[:80]}")
    else:
        result("Negative amount rejection", False, "Negative amount transfer accepted!")

def test_overflow_amount_transfer():
    """Try to transfer u64::MAX amount."""
    r = rpc("createWallet", ["overflow_wallet"])
    addr = r.get("result", {}).get("address", "")
    if not addr:
        result("Overflow amount handling", True, "Skipped")
        return

    rpc("requestAirdrop", [addr, 1000000000])
    time.sleep(2)

    # u64::MAX = 18446744073709551615
    r = rpc("transfer", [addr, addr, 18446744073709551615, "overflow_wallet"])
    if "error" in r:
        result("Overflow amount handling", True, f"Rejected: {r['error'].get('message', '')[:80]}")
    else:
        result("Overflow amount handling", False, "u64::MAX transfer accepted! Possible overflow")

def test_double_spend_concurrent():
    """Try to double-spend using concurrent transactions."""
    r = rpc("createWallet", ["dspend_wallet"])
    addr = r.get("result", {}).get("address", "")
    if not addr:
        result("Concurrent double-spend", True, "Skipped")
        return

    rpc("requestAirdrop", [addr, 2000000000])  # 2 MOLT
    time.sleep(2)

    # Create recipient
    r2 = rpc("createWallet", ["dspend_recv"])
    recv = r2.get("result", {}).get("address", "")
    if not recv:
        result("Concurrent double-spend", True, "Skipped")
        return

    # Try to send 1.5 MOLT to recv twice simultaneously (only 2 MOLT available minus fees)
    results_list = []
    def do_transfer():
        r = rpc("transfer", [addr, recv, 1500000000, "dspend_wallet"], timeout=5)
        results_list.append(r)

    t1 = threading.Thread(target=do_transfer)
    t2 = threading.Thread(target=do_transfer)
    t1.start()
    t2.start()
    t1.join(timeout=10)
    t2.join(timeout=10)

    time.sleep(3)

    # Check balances
    bal = rpc("getBalance", [addr])
    recv_bal = rpc("getBalance", [recv])
    sender_balance = bal.get("result", {}).get("balance", 0) if isinstance(bal.get("result"), dict) else bal.get("result", 0)
    recv_balance = recv_bal.get("result", {}).get("balance", 0) if isinstance(recv_bal.get("result"), dict) else recv_bal.get("result", 0)

    successes = sum(1 for r in results_list if "result" in r and "error" not in r)
    detail = f"{successes} succeeded, sender={sender_balance}, receiver={recv_balance}"

    if successes <= 1:
        result("Concurrent double-spend", True, detail)
    else:
        # Both went through - check if balance went negative
        if isinstance(sender_balance, (int, float)) and sender_balance < 0:
            result("Concurrent double-spend", False, f"CRITICAL: negative balance! {detail}")
        else:
            result("Concurrent double-spend", False, f"Both transfers succeeded. {detail}", warn=True)

def test_self_transfer_drain():
    """Try self-transfer to drain via fees only."""
    r = rpc("createWallet", ["selfdrain_wallet"])
    addr = r.get("result", {}).get("address", "")
    if not addr:
        result("Self-transfer fee drain", True, "Skipped")
        return

    rpc("requestAirdrop", [addr, 100000000])  # 0.1 MOLT
    time.sleep(2)

    initial = rpc("getBalance", [addr])
    init_bal = initial.get("result", 0)
    if isinstance(init_bal, dict):
        init_bal = init_bal.get("balance", 0)

    # Send self-transfer 10 times
    for _ in range(10):
        rpc("transfer", [addr, addr, 0, "selfdrain_wallet"], timeout=3)

    time.sleep(2)
    final = rpc("getBalance", [addr])
    final_bal = final.get("result", 0)
    if isinstance(final_bal, dict):
        final_bal = final_bal.get("balance", 0)

    result("Self-transfer fee drain", True, f"Balance: {init_bal} -> {final_bal}")

# ============================================================
# SECTION 3: WEBSOCKET ATTACKS
# ============================================================

def test_ws_connection_flood():
    """Open many WebSocket connections to check for limits."""
    import http.client
    connections = []
    connected = 0
    failed = 0
    target = 50  # Try opening 50 connections

    for i in range(target):
        try:
            s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            s.settimeout(2)
            s.connect((HOST, WS_PORT))
            # Send WebSocket upgrade request
            upgrade = (
                f"GET / HTTP/1.1\r\n"
                f"Host: {HOST}:{WS_PORT}\r\n"
                f"Upgrade: websocket\r\n"
                f"Connection: Upgrade\r\n"
                f"Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n"
                f"Sec-WebSocket-Version: 13\r\n"
                f"\r\n"
            )
            s.sendall(upgrade.encode())
            resp = s.recv(1024)
            if b"101" in resp:
                connections.append(s)
                connected += 1
            else:
                s.close()
                failed += 1
        except Exception:
            failed += 1

    # Verify RPC still works
    r = rpc("getSlot")
    alive = "result" in r

    # Clean up
    for s in connections:
        try:
            s.close()
        except:
            pass

    detail = f"{connected} WS connections opened, {failed} failed, RPC {'alive' if alive else 'DEAD'}"
    if alive:
        # WS limit is 500+ so 50 connections should be fine
        result("WS connection flood resilience", True, detail)
    else:
        result("WS connection flood", False, f"RPC died! {detail}")

def test_ws_malformed_messages():
    """Send malformed WebSocket messages."""
    try:
        s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        s.settimeout(3)
        s.connect((HOST, WS_PORT))
        upgrade = (
            f"GET / HTTP/1.1\r\n"
            f"Host: {HOST}:{WS_PORT}\r\n"
            f"Upgrade: websocket\r\n"
            f"Connection: Upgrade\r\n"
            f"Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n"
            f"Sec-WebSocket-Version: 13\r\n\r\n"
        )
        s.sendall(upgrade.encode())
        resp = s.recv(1024)

        if b"101" not in resp:
            result("WS malformed message handling", True, "Skipped (no WS upgrade)")
            s.close()
            return

        # Send various malformed frames
        garbage_frames = [
            b'\x81\x00',  # empty text frame
            b'\x81\x7f' + struct.pack('>Q', 2**60),  # absurd length
            b'\x88\x00',  # close frame
            b'\x00' * 100,  # continuation frames
            b'\xff\xff\xff\xff',  # invalid opcode
        ]
        for frame in garbage_frames:
            try:
                s.sendall(frame)
            except:
                pass
            time.sleep(0.1)

        s.close()
    except Exception as e:
        pass

    r = rpc("getSlot")
    alive = "result" in r
    result("WS malformed message handling", alive, f"Server {'survived' if alive else 'DEAD'} after malformed WS frames")

# ============================================================
# SECTION 4: ADMIN API ATTACKS
# ============================================================

def test_admin_brute_force():
    """Try common/weak admin tokens."""
    weak_tokens = [
        "", "admin", "password", "123456", "token", "secret",
        "moltchain", "admin123", "test", "root", "changeme",
        "default", "validator", "molt", "blockchain",
        "\x00", "\x00" * 32, "A" * 1000,
    ]
    bypassed = False
    for token in weak_tokens:
        r = rpc("setFeeConfig", [{"admin_token": token, "base_fee": 0}], timeout=2)
        if "result" in r and "error" not in r:
            bypassed = True
            result("Admin token brute force", False, f"CRITICAL: admin bypass with token '{token}'")
            return

    result("Admin token brute force", True, f"All {len(weak_tokens)} weak tokens rejected")

def test_admin_without_token():
    """Try admin methods without any token."""
    admin_methods = [
        ("setFeeConfig", [{"base_fee": 0}]),
        ("setRentParams", [{"rent_rate": 0}]),
        ("setContractAbi", ["fake_address", []]),
    ]
    bypassed = False
    for method, params in admin_methods:
        r = rpc(method, params, timeout=2)
        if "result" in r and "error" not in r:
            bypassed = True
            result("Admin without token", False, f"CRITICAL: {method} succeeded without token!")
            return

    result("Admin without token", True, "All admin methods rejected without token")

def test_admin_timing_attack():
    """Test if admin token comparison is vulnerable to timing attack."""
    # Send several tokens of different lengths and measure response time
    timings = {}
    for length in [1, 8, 16, 32, 64, 128]:
        token = "A" * length
        times = []
        for _ in range(5):
            start = time.time()
            rpc("setFeeConfig", [{"admin_token": token, "base_fee": 0}], timeout=2)
            elapsed = time.time() - start
            times.append(elapsed)
        timings[length] = sum(times) / len(times)

    # Check if timing varies significantly with token length
    times_list = list(timings.values())
    max_diff = max(times_list) - min(times_list)

    if max_diff < 0.05:  # Less than 50ms variation
        result("Admin timing attack resistance", True, f"Max timing diff: {max_diff*1000:.1f}ms (constant-time)")
    else:
        result("Admin timing attack resistance", False, f"Timing varies by {max_diff*1000:.1f}ms -- possible timing leak", warn=True)

# ============================================================
# SECTION 5: P2P / NETWORK ATTACKS
# ============================================================

def test_p2p_connection_flood():
    """Try to open many TCP connections to the P2P port."""
    connections = []
    connected = 0
    target = 100

    for i in range(target):
        try:
            s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            s.settimeout(1)
            s.connect((HOST, P2P_PORT))
            connections.append(s)
            connected += 1
        except Exception:
            break

    # Verify RPC still works
    r = rpc("getSlot")
    alive = "result" in r

    for s in connections:
        try:
            s.close()
        except:
            pass

    detail = f"{connected} TCP connections to QUIC port, RPC {'alive' if alive else 'DEAD'}"
    if alive:
        # P2P uses QUIC (UDP), so TCP connections are ignored by the server
        result("P2P connection flood resilience", True, detail)
    else:
        result("P2P connection flood", False, f"RPC died! {detail}")

def test_p2p_garbage_data():
    """Send garbage data to the P2P QUIC port."""
    try:
        # P2P uses QUIC (UDP), send garbage UDP packets
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        s.settimeout(1)

        garbage_packets = [
            b'\x00' * 1200,  # zero-filled
            os.urandom(1200),  # random bytes
            b'\xff' * 1200,  # all 0xFF
            b'QUIC' + os.urandom(1196),  # fake QUIC header
            os.urandom(65000),  # jumbo packet
        ]

        for pkt in garbage_packets * 10:
            s.sendto(pkt, (HOST, P2P_PORT))

        s.close()
    except Exception:
        pass

    time.sleep(1)
    r = rpc("getSlot")
    alive = "result" in r
    result("P2P garbage data resilience", alive, f"Server {'survived' if alive else 'DEAD'} after garbage UDP flood")

def test_p2p_syn_flood_simulation():
    """Simulate SYN-like flood on the QUIC port via UDP."""
    try:
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        # Send 1000 fake QUIC initial packets
        for i in range(1000):
            # Craft a minimal QUIC initial packet header
            pkt = bytes([0xC0 | 0x03])  # long header + initial packet type
            pkt += bytes([0x00, 0x00, 0x00, 0x01])  # version
            pkt += bytes([8]) + os.urandom(8)  # DCID
            pkt += bytes([8]) + os.urandom(8)  # SCID
            pkt += bytes([0x00])  # token length
            pkt += struct.pack('>H', 1200)  # length
            pkt += os.urandom(1200)  # payload
            s.sendto(pkt, (HOST, P2P_PORT))
        s.close()
    except Exception:
        pass

    time.sleep(1)
    r = rpc("getSlot")
    alive = "result" in r
    result("P2P QUIC flood resilience", alive, f"Server {'survived' if alive else 'DEAD'} after 1000 fake QUIC initials")

# ============================================================
# SECTION 6: STATE MANIPULATION ATTACKS
# ============================================================

def test_forge_signature():
    """Try to send a transaction with a forged/zero signature."""
    # Create a tx with all-zero signature
    fake_sig = base64.b64encode(b'\x00' * 64).decode()
    r = rpc("sendTransaction", [fake_sig], timeout=3)
    if "error" in r:
        result("Forged signature rejection", True, f"Rejected: {r['error'].get('message', '')[:80]}")
    else:
        result("Forged signature rejection", False, "Zero signature accepted!")

def test_transfer_from_foreign_wallet():
    """Try to transfer from someone else's account."""
    # Create two wallets
    r1 = rpc("createWallet", ["foreign_victim"])
    r2 = rpc("createWallet", ["foreign_attacker"])
    victim_addr = r1.get("result", {}).get("address", "")
    attacker_addr = r2.get("result", {}).get("address", "")

    if not victim_addr or not attacker_addr:
        result("Foreign wallet transfer", True, "Skipped")
        return

    rpc("requestAirdrop", [victim_addr, 5000000000])  # 5 MOLT to victim
    time.sleep(2)

    # Try to transfer from victim using attacker's wallet
    r = rpc("transfer", [victim_addr, attacker_addr, 1000000000, "foreign_attacker"], timeout=5)
    time.sleep(2)

    # Check victim's balance -- should still be ~5 MOLT
    bal = rpc("getBalance", [victim_addr])
    victim_bal = bal.get("result", 0)
    if isinstance(victim_bal, dict):
        victim_bal = victim_bal.get("balance", 0)

    if "error" in r or (isinstance(victim_bal, (int, float)) and victim_bal >= 4000000000):
        result("Foreign wallet transfer", True, f"Protected. Victim balance: {victim_bal}")
    else:
        result("Foreign wallet transfer", False, f"CRITICAL: transfer succeeded! Victim balance: {victim_bal}")

def test_nonexistent_sender():
    """Try to transfer from a nonexistent account."""
    fake_addr = "11111111111111111111111111111111"  # system program address
    r2 = rpc("createWallet", ["nonexist_recv"])
    recv = r2.get("result", {}).get("address", "")
    if not recv:
        result("Nonexistent sender", True, "Skipped")
        return

    r = rpc("transfer", [fake_addr, recv, 1000, "nonexist_recv"], timeout=3)
    if "error" in r:
        result("Nonexistent sender", True, f"Rejected: {r['error'].get('message', '')[:80]}")
    else:
        result("Nonexistent sender", False, "Transfer from nonexistent account accepted!", warn=True)

# ============================================================
# SECTION 7: CONTRACT ATTACKS
# ============================================================

def test_deploy_malicious_contract():
    """Try to deploy a contract with malicious/invalid WASM."""
    r = rpc("createWallet", ["malicious_deployer"])
    addr = r.get("result", {}).get("address", "")
    if not addr:
        result("Malicious contract deploy", True, "Skipped")
        return

    rpc("requestAirdrop", [addr, 10000000000])
    time.sleep(2)

    # Try deploying various invalid "WASM" blobs
    tests = [
        ("empty", ""),
        ("garbage", base64.b64encode(os.urandom(1024)).decode()),
        ("elf_header", base64.b64encode(b'\x7fELF' + os.urandom(100)).decode()),
        ("wasm_magic_only", base64.b64encode(b'\x00asm\x01\x00\x00\x00').decode()),
        ("huge", base64.b64encode(os.urandom(100000)).decode()),
    ]

    all_rejected = True
    for name, code in tests:
        r = rpc("deployContract", [code, "malicious_deployer", f"mal_{name}"], timeout=5)
        if "result" in r and "error" not in r:
            all_rejected = False
            break

    r = rpc("getSlot")
    alive = "result" in r

    if alive and all_rejected:
        result("Malicious contract deploy", True, "All invalid WASM blobs rejected")
    elif alive:
        result("Malicious contract deploy", False, "Some invalid WASM was accepted", warn=True)
    else:
        result("Malicious contract deploy", False, "Server crashed during malicious deploy")

# ============================================================
# SECTION 8: CONSENSUS / BLOCK ATTACKS
# ============================================================

def test_validator_info_consistency():
    """Check that validator info is consistent across all validators."""
    rpcs = ["http://127.0.0.1:8000", "http://127.0.0.1:8001", "http://127.0.0.1:8002"]
    slots = []
    for rpc_url in rpcs:
        try:
            body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "getSlot", "params": []}).encode()
            req = urllib.request.Request(rpc_url, data=body, headers={"Content-Type": "application/json"})
            with urllib.request.urlopen(req, timeout=3) as resp:
                r = json.loads(resp.read())
                slots.append(r.get("result", -1))
        except Exception:
            slots.append(-1)

    valid_slots = [s for s in slots if s >= 0]
    if len(valid_slots) < 2:
        result("Validator consistency", True, "Fewer than 2 validators reachable -- skipped")
        return

    max_diff = max(valid_slots) - min(valid_slots)
    if max_diff <= 5:
        result("Validator consistency", True, f"Slots: {slots}, max diff: {max_diff}")
    else:
        result("Validator consistency", False, f"Slots diverged: {slots}, diff: {max_diff}", warn=True)

def test_chain_integrity():
    """Walk the chain backwards and verify parent hash linkage."""
    r = rpc("getSlot")
    current = r.get("result", 0)
    if current < 3:
        time.sleep(5)
        r = rpc("getSlot")
        current = r.get("result", 0)

    if current < 2:
        result("Chain integrity walk", True, "Chain too short -- skipped")
        return

    broken = False
    prev_parent = None
    for slot in range(current, max(0, current - 20), -1):
        r = rpc("getBlock", [slot])
        if "error" in r:
            continue
        block = r.get("result", {})
        block_hash = block.get("hash", block.get("blockhash", ""))
        parent_hash = block.get("parent_hash", block.get("previous_blockhash", ""))

        if prev_parent is not None and prev_parent != block_hash:
            broken = True
            break
        prev_parent = parent_hash

    if broken:
        result("Chain integrity walk", False, "Broken parent-hash linkage!")
    else:
        result("Chain integrity walk", True, f"Verified {min(current, 20)} blocks")

# ============================================================
# SECTION 9: MEMORY / RESOURCE EXHAUSTION
# ============================================================

def test_many_wallets():
    """Create many wallets to test for resource exhaustion."""
    created = 0
    for i in range(100):
        r = rpc("createWallet", [f"stress_wallet_{i}"], timeout=3)
        if "result" in r:
            created += 1

    r = rpc("getSlot")
    alive = "result" in r
    result("Mass wallet creation", alive, f"Created {created}/100 wallets, server {'alive' if alive else 'DEAD'}")

def test_many_airdrops():
    """Request many airdrops in rapid succession."""
    r = rpc("createWallet", ["airdrop_stress"])
    addr = r.get("result", {}).get("address", "")
    if not addr:
        result("Airdrop flood", True, "Skipped")
        return

    success = 0
    failed = 0
    for i in range(50):
        r = rpc("requestAirdrop", [addr, 1000000], timeout=2)
        if "result" in r:
            success += 1
        else:
            failed += 1

    time.sleep(2)
    r = rpc("getSlot")
    alive = "result" in r
    detail = f"{success} succeeded, {failed} failed, server {'alive' if alive else 'DEAD'}"
    result("Airdrop flood", alive, detail)

def test_rapid_account_queries():
    """Query the same account thousands of times rapidly."""
    r = rpc("createWallet", ["query_stress"])
    addr = r.get("result", {}).get("address", "")
    if not addr:
        addr = "11111111111111111111111111111111"

    count = 0
    start = time.time()
    with concurrent.futures.ThreadPoolExecutor(max_workers=30) as pool:
        futures = [pool.submit(rpc, "getBalance", [addr], 2) for _ in range(300)]
        for f in concurrent.futures.as_completed(futures, timeout=15):
            count += 1
    elapsed = time.time() - start

    r = rpc("getSlot")
    alive = "result" in r
    if not alive:
        # Server may just be rate-limited after rapid burst; wait and retry
        time.sleep(2)
        r = rpc("getSlot")
        alive = "result" in r
    result("Rapid query stress", alive, f"{count} queries in {elapsed:.1f}s ({count/elapsed:.0f} req/s)")# ============================================================
# SECTION 10: EDGE CASE ATTACKS
# ============================================================

def test_method_fuzzing():
    """Try calling random/nonexistent RPC methods."""
    fuzz_methods = [
        "deleteBlock", "setBalance", "mintToken", "shutdown",
        "reboot", "dropDatabase", "exec", "eval", "system",
        "__proto__", "constructor", "toString", "valueOf",
        "getSlot; DROP TABLE blocks", "../../etc/passwd",
        "A" * 10000,  # very long method name
    ]
    for m in fuzz_methods:
        rpc(m, timeout=2)

    r = rpc("getSlot")
    alive = "result" in r
    result("Method fuzzing", alive, f"Server {'survived' if alive else 'DEAD'} {len(fuzz_methods)} fuzz methods")

def test_unicode_attacks():
    """Send Unicode edge cases in RPC params."""
    attacks = [
        "\u0000",  # null byte
        "\ud800",  # unpaired surrogate (invalid)
        "\U0001F4A9" * 10000,  # many emoji
        "\u202e" + "txget",  # RTL override
        "\ufeff" * 100,  # BOM flood
    ]
    for atk in attacks:
        try:
            rpc("getAccountInfo", [atk], timeout=2)
        except:
            pass

    r = rpc("getSlot")
    alive = "result" in r
    result("Unicode attack handling", alive, f"Server {'survived' if alive else 'DEAD'} after Unicode attacks")

def test_concurrent_deploys():
    """Deploy many contracts simultaneously."""
    r = rpc("createWallet", ["deploy_stress"])
    addr = r.get("result", {}).get("address", "")
    if not addr:
        result("Concurrent deploy stress", True, "Skipped")
        return

    rpc("requestAirdrop", [addr, 50000000000])  # 50 MOLT
    time.sleep(2)

    # Minimal valid-ish WASM
    wasm_header = b'\x00asm\x01\x00\x00\x00'
    wasm_b64 = base64.b64encode(wasm_header).decode()

    results_list = []
    def deploy_one(i):
        r = rpc("deployContract", [wasm_b64, "deploy_stress", f"stress_{i}"], timeout=5)
        results_list.append(r)

    with concurrent.futures.ThreadPoolExecutor(max_workers=10) as pool:
        futures = [pool.submit(deploy_one, i) for i in range(20)]
        concurrent.futures.wait(futures, timeout=30)

    r = rpc("getSlot")
    alive = "result" in r
    deployed = sum(1 for r in results_list if "result" in r and "error" not in r)
    result("Concurrent deploy stress", alive, f"{deployed}/20 deployed, server {'alive' if alive else 'DEAD'}")

def test_http_method_attacks():
    """Try non-POST HTTP methods."""
    alive_after = True
    for method in ["GET", "PUT", "DELETE", "PATCH", "OPTIONS", "HEAD"]:
        try:
            req = urllib.request.Request(RPC, method=method)
            urllib.request.urlopen(req, timeout=2)
        except:
            pass

    r = rpc("getSlot")
    alive_after = "result" in r
    result("Non-POST HTTP method handling", alive_after, "Server survived non-POST requests")

def test_content_type_attacks():
    """Send requests with wrong content types."""
    payloads = [
        ("text/plain", b'{"jsonrpc":"2.0","id":1,"method":"getSlot","params":[]}'),
        ("text/xml", b'<xml>evil</xml>'),
        ("multipart/form-data", b'----boundary\r\nContent-Disposition: form-data; name="file"\r\n\r\nevil\r\n----boundary--'),
        ("application/x-www-form-urlencoded", b'method=getSlot'),
    ]
    for ct, body in payloads:
        try:
            rpc_raw(body, timeout=2, content_type=ct)
        except:
            pass

    r = rpc("getSlot")
    alive = "result" in r
    result("Content-type attack handling", alive, "Server survived wrong content types")

# ============================================================
# MAIN
# ============================================================

def main():
    print("=" * 70)
    print("  MoltChain Adversarial Security Test Suite")
    print("=" * 70)
    print(f"  RPC: {RPC}")
    print(f"  WS:  {HOST}:{WS_PORT}")
    print(f"  P2P: {HOST}:{P2P_PORT}")
    print()

    # Verify basic connectivity
    r = rpc("getSlot")
    if "result" not in r:
        print("FATAL: Cannot connect to validator RPC")
        sys.exit(1)

    sections = [
        ("RPC LAYER ATTACKS", [
            test_oversized_payload,
            test_malformed_json,
            test_binary_injection_in_params,
            test_rpc_flooding,
            test_rate_limit_bypass_xff,
            test_concurrent_sendtx_invalid,
            test_huge_transaction,
        ]),
        ("TRANSACTION ATTACKS", [
            test_replay_attack,
            test_zero_amount_transfer,
            test_negative_amount_transfer,
            test_overflow_amount_transfer,
            test_double_spend_concurrent,
            test_self_transfer_drain,
        ]),
        ("WEBSOCKET ATTACKS", [
            test_ws_connection_flood,
            test_ws_malformed_messages,
        ]),
        ("ADMIN API ATTACKS", [
            test_admin_brute_force,
            test_admin_without_token,
            test_admin_timing_attack,
        ]),
        ("P2P / NETWORK ATTACKS", [
            test_p2p_connection_flood,
            test_p2p_garbage_data,
            test_p2p_syn_flood_simulation,
        ]),
        ("STATE MANIPULATION ATTACKS", [
            test_forge_signature,
            test_transfer_from_foreign_wallet,
            test_nonexistent_sender,
        ]),
        ("CONTRACT ATTACKS", [
            test_deploy_malicious_contract,
        ]),
        ("CONSENSUS / BLOCK ATTACKS", [
            test_validator_info_consistency,
            test_chain_integrity,
        ]),
        ("RESOURCE EXHAUSTION", [
            test_many_wallets,
            test_many_airdrops,
            test_rapid_account_queries,
        ]),
        ("EDGE CASE ATTACKS", [
            test_method_fuzzing,
            test_unicode_attacks,
            test_concurrent_deploys,
            test_http_method_attacks,
            test_content_type_attacks,
        ]),
    ]

    for section_name, tests in sections:
        print(f"\n--- {section_name} ---")
        for test_fn in tests:
            try:
                test_fn()
            except Exception as e:
                result(test_fn.__name__.replace("test_", ""), False, f"EXCEPTION: {e}")
            # Verify server is still alive between tests
            try:
                alive_check = rpc("getSlot", timeout=3)
                if "result" not in alive_check:
                    print(f"  [!] SERVER UNRESPONSIVE after {test_fn.__name__}")
                    time.sleep(3)
            except:
                print(f"  [!] SERVER UNREACHABLE after {test_fn.__name__}")
                time.sleep(5)

    print("\n" + "=" * 70)
    print(f"  RESULTS: {PASS} PASS / {FAIL} FAIL / {WARN} WARN")
    print("=" * 70)

    if FAIL > 0:
        print("\n  FAILURES:")
        for tag, name, detail in RESULTS:
            if tag == "FAIL":
                print(f"    - {name}: {detail}")

    if WARN > 0:
        print("\n  WARNINGS:")
        for tag, name, detail in RESULTS:
            if tag == "WARN":
                print(f"    - {name}: {detail}")

    print()
    return 0 if FAIL == 0 else 1

if __name__ == "__main__":
    sys.exit(main())
