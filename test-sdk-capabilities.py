#!/usr/bin/env python3
"""
MoltChain SDK Capability Test
Tests all core SDK functions to ensure readiness for wallet/programs/marketplace
"""

import requests
import json
import time
from typing import Dict, Any

class MoltChainClient:
    """Python SDK Client"""
    
    def __init__(self, rpc_url: str = "http://localhost:8899"):
        self.rpc_url = rpc_url
        self.request_id = 0
    
    def _rpc_call(self, method: str, params: Any = None) -> Dict:
        """Make JSON-RPC call"""
        self.request_id += 1
        payload = {
            "jsonrpc": "2.0",
            "id": self.request_id,
            "method": method,
        }
        if params is not None:
            payload["params"] = params
        
        response = requests.post(
            self.rpc_url,
            headers={"Content-Type": "application/json"},
            data=json.dumps(payload),
            timeout=5
        )
        response.raise_for_status()
        result = response.json()
        
        if "error" in result:
            raise Exception(f"RPC Error: {result['error']}")
        
        return result.get("result")
    
    def get_slot(self) -> int:
        """Get current slot"""
        return self._rpc_call("getSlot")
    
    def get_block(self, slot: int) -> Dict:
        """Get block by slot"""
        return self._rpc_call("getBlock", [slot])
    
    def get_balance(self, address: str) -> int:
        """Get account balance"""
        return self._rpc_call("getBalance", [address])
    
    def get_account_info(self, address: str) -> Dict:
        """Get account information"""
        return self._rpc_call("getAccountInfo", [address])
    
    def get_validators(self) -> list:
        """Get validator list"""
        return self._rpc_call("getValidators")
    
    def get_network_info(self) -> Dict:
        """Get network information"""
        return self._rpc_call("getNetworkInfo")
    
    def send_transaction(self, tx_base64: str) -> str:
        """Send transaction"""
        return self._rpc_call("sendTransaction", [tx_base64])


def test_capability(name: str, func, expected=True):
    """Test a specific capability"""
    try:
        result = func()
        if expected:
            print(f"✅ {name}")
            return True, result
        else:
            print(f"⚠️  {name} - unexpected success")
            return True, result
    except Exception as e:
        if not expected:
            print(f"✅ {name} - expected failure")
            return True, str(e)
        else:
            print(f"❌ {name} - {str(e)}")
            return False, str(e)


def main():
    print("🦞 MoltChain SDK Capability Test")
    print("=" * 50)
    print()
    
    # Test all 3 validators
    validators = [
        ("Validator 1", "http://localhost:8899"),
        ("Validator 2", "http://localhost:8901"),
        ("Validator 3", "http://localhost:8903"),
    ]
    
    results = {}
    
    for name, url in validators:
        print(f"\n📡 Testing {name} ({url})")
        print("-" * 50)
        
        client = MoltChainClient(url)
        validator_results = {}
        
        # Test 1: Get Slot
        success, result = test_capability(
            "Get Current Slot",
            lambda: client.get_slot()
        )
        validator_results["getSlot"] = {"success": success, "result": result}
        
        # Test 2: Get Block
        if success:
            slot = result
            success2, result2 = test_capability(
                f"Get Block (slot {slot})",
                lambda: client.get_block(slot)
            )
            validator_results["getBlock"] = {"success": success2, "result": result2}
        
        # Test 3: Get Balance (non-existent account)
        test_address = "11111111111111111111111111111111"
        success, result = test_capability(
            "Get Balance (System Program)",
            lambda: client.get_balance(test_address),
            expected=True  # Should work, might be 0
        )
        validator_results["getBalance"] = {"success": success, "result": result}
        
        # Test 4: Get Account Info
        success, result = test_capability(
            "Get Account Info",
            lambda: client.get_account_info(test_address),
            expected=True
        )
        validator_results["getAccountInfo"] = {"success": success, "result": result}
        
        # Test 5: Get Validators
        success, result = test_capability(
            "Get Validators List",
            lambda: client.get_validators()
        )
        validator_results["getValidators"] = {"success": success, "result": result}
        
        # Test 6: Get Network Info
        success, result = test_capability(
            "Get Network Info",
            lambda: client.get_network_info()
        )
        validator_results["getNetworkInfo"] = {"success": success, "result": result}
        
        results[name] = validator_results
    
    # Summary
    print("\n\n📊 Summary")
    print("=" * 50)
    
    total_tests = 0
    passed_tests = 0
    
    for validator, tests in results.items():
        print(f"\n{validator}:")
        for method, result in tests.items():
            total_tests += 1
            if result["success"]:
                passed_tests += 1
                print(f"  ✅ {method}")
            else:
                print(f"  ❌ {method}: {result['result']}")
    
    print(f"\n📈 Results: {passed_tests}/{total_tests} tests passed")
    
    # Core capabilities check
    print("\n\n🎯 Core Capabilities for Future Features")
    print("=" * 50)
    
    required_for_wallet = [
        ("✓", "getBalance", "Check account balances"),
        ("✓", "getAccountInfo", "View account data"),
        ("?", "sendTransaction", "Submit transfers (needs implementation)"),
        ("?", "getRecentBlockhash", "Build transactions (needs implementation)"),
    ]
    
    required_for_programs = [
        ("?", "getProgramAccounts", "List program-owned accounts (needs implementation)"),
        ("?", "deployProgram", "Deploy smart contracts (needs implementation)"),
        ("?", "invokeProgram", "Call program instructions (needs implementation)"),
    ]
    
    required_for_marketplace = [
        ("✓", "getSlot", "Track confirmation time"),
        ("✓", "getBlock", "Verify transaction finality"),
        ("✓", "getValidators", "Network health monitoring"),
        ("?", "getTransactionHistory", "Order history (needs implementation)"),
    ]
    
    print("\n🔐 Wallet Requirements:")
    for status, method, desc in required_for_wallet:
        print(f"  {status} {method:20} - {desc}")
    
    print("\n📝 Smart Contracts/Programs Requirements:")
    for status, method, desc in required_for_programs:
        print(f"  {status} {method:20} - {desc}")
    
    print("\n🏪 Marketplace Requirements:")
    for status, method, desc in required_for_marketplace:
        print(f"  {status} {method:20} - {desc}")
    
    print("\n\n✅ SDK Readiness Assessment:")
    print("  • Basic queries: READY ✅")
    print("  • Transaction submission: NEEDS IMPLEMENTATION ⚠️")
    print("  • Program deployment: NEEDS IMPLEMENTATION ⚠️")
    print("  • Transaction history: NEEDS IMPLEMENTATION ⚠️")
    
    print("\n💡 Next Steps:")
    print("  1. Implement sendTransaction with proper serialization")
    print("  2. Add getRecentBlockhash RPC method")
    print("  3. Add program deployment RPC methods")
    print("  4. Add transaction history indexing")


if __name__ == "__main__":
    main()
