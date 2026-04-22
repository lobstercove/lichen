pub(super) use crate::codegen_python_support::generate_python;
pub(super) use crate::codegen_typescript_support::generate_typescript;

#[cfg(test)]
mod tests {
    use super::{generate_python, generate_typescript};
    use crate::codegen_shared_support::{clean_param_name, to_camel_case, to_pascal_case};
    use lichen_core::ContractAbi;

    #[test]
    fn test_to_camel_case() {
        assert_eq!(to_camel_case("balance_of"), "balanceOf");
        assert_eq!(to_camel_case("total_supply"), "totalSupply");
        assert_eq!(to_camel_case("transfer"), "transfer");
        assert_eq!(to_camel_case("get_owner_of"), "getOwnerOf");
    }

    #[test]
    fn test_to_pascal_case() {
        assert_eq!(to_pascal_case("lusd_token"), "LusdToken");
        assert_eq!(to_pascal_case("dex_core"), "DexCore");
        assert_eq!(to_pascal_case("moss_storage"), "MossStorage");
    }

    #[test]
    fn test_clean_param_name() {
        assert_eq!(clean_param_name("owner_ptr"), "owner");
        assert_eq!(clean_param_name("amount"), "amount");
        assert_eq!(clean_param_name("to_ptr"), "to");
    }

    #[test]
    fn test_generate_typescript_lusd_token() {
        let abi_json = include_str!("../../contracts/lusd_token/abi.json");
        let abi: ContractAbi = serde_json::from_str(abi_json).unwrap();
        let code = generate_typescript(&abi);
        assert!(code.contains("class LusdTokenClient"));
        assert!(code.contains("async balanceOf("));
        assert!(code.contains("async transfer(signer: Keypair"));
        assert!(code.contains("async totalSupply("));
        assert!(code.contains("queryContract"));
        assert!(code.contains("callContract"));
        assert!(code.contains("@lichen/sdk"));
    }

    #[test]
    fn test_generate_python_lusd_token() {
        let abi_json = include_str!("../../contracts/lusd_token/abi.json");
        let abi: ContractAbi = serde_json::from_str(abi_json).unwrap();
        let code = generate_python(&abi);
        assert!(code.contains("class LusdTokenClient:"));
        assert!(code.contains("def balance_of(self"));
        assert!(code.contains("def transfer(self, signer: Keypair"));
        assert!(code.contains("def total_supply(self"));
        assert!(code.contains("query_contract"));
        assert!(code.contains("call_contract"));
        assert!(code.contains("lichen_sdk"));
    }
}
