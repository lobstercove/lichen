# Lichen Exchange Address Validation Vectors

**Status:** Source-derived and tested
**Created:** 2026-06-29
**Integration guide:** [EXCHANGE_INTEGRATION.md](EXCHANGE_INTEGRATION.md)
**Metadata sheet:** [EXCHANGE_CHAIN_METADATA.md](EXCHANGE_CHAIN_METADATA.md)
**Regression test:** `core/src/account.rs::account::tests::test_exchange_address_validation_vectors`

Native LICN deposit addresses are Base58 strings that decode to exactly 32 bytes.
The regex below is only a fast prefilter. Exchanges must still Base58-decode the
address and require exactly 32 decoded bytes.

## Validation Rule

Recommended prefilter:

```text
^[1-9A-HJ-NP-Za-km-z]{32,44}$
```

Required validation:

```text
valid_native_address(address):
    if not matches(address, "^[1-9A-HJ-NP-Za-km-z]{32,44}$"):
        return false
    decoded = base58_decode(address)
    return len(decoded) == 32
```

Reject EVM `0x...` strings for native LICN deposits. EVM-format addresses are
display/mapping values, not native deposit addresses.

## Valid Vectors

| Name | Native Base58 address | Decoded hex | EVM mapping |
| --- | --- | --- | --- |
| `all_zero` | `11111111111111111111111111111111` | `0000000000000000000000000000000000000000000000000000000000000000` | `0x88386fc84ba6bc95484008f6362f93160ef3e563` |
| `minimal_nonzero` | `11111111111111111111111111111112` | `0000000000000000000000000000000000000000000000000000000000000001` | `0x717e6a320cf44b4afac2b0732d9fcbe2b7fa0cf6` |
| `all_ones` | `4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi` | `0101010101010101010101010101010101010101010101010101010101010101` | `0xb312bec018884c2d66667c67a90508214bd8bafc` |
| `all_ff` | `JEKNVnkbo3jma5nREBBJCDoXFVeKkD56V3xKrvRmWxFG` | `ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff` | `0xab758a3376d22aedc6a55823d1b3ecbee81b8fb9` |
| `ml_dsa_seed_07` | `88aSEgG26m1Tnoco64rGqoVGjYaGgUbxja6GdRub7A8` | `01d3a1e51ecf491b79ca7691bd269271f8d8e8d94313a6abcc6c8ae8bc34b5f9` | `0x279e75e415dc4eb29935e1381e65fa92cfc114e1` |

`all_zero` and `minimal_nonzero` are structural validation vectors, not exchange
deposit addresses to assign to users. Exchanges should generate real deposit
wallets from managed keys and must not assign reserved/system-style addresses.

## Invalid Vectors

| Name | Input | Expected result |
| --- | --- | --- |
| `empty` | `` | Reject; decoded length is not 32 bytes |
| `non_base58_chars` | `0OIl` | Reject; Base58 alphabet excludes `0`, `O`, `I`, and `l` |
| `too_short` | `1` | Reject; decoded length is 1 byte |
| `too_long` | `111111111111111111111111111111111` | Reject; decoded length is 33 bytes |
| `evm_shaped` | `0x88386fc84ba6bc95484008f6362f93160ef3e563` | Reject for native LICN deposit address |

## Source Evidence

These vectors are locked by:

```bash
cargo test -p lobstercove-lichen-core account::tests::test_exchange_address_validation_vectors
```

The passing test verifies:

- Hex bytes to native Base58 encoding.
- Native Base58 back to `Pubkey`.
- EVM mapping from the same native pubkey bytes.
- Invalid input rejection through `Pubkey::from_base58`.
