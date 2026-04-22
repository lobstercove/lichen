use lichen_core::{AbiFunction, AbiType};

/// Convert snake_case to camelCase
pub(super) fn to_camel_case(s: &str) -> String {
    let mut result = String::new();
    let mut capitalize_next = false;
    for (index, ch) in s.chars().enumerate() {
        if ch == '_' {
            capitalize_next = true;
        } else if capitalize_next {
            result.push(ch.to_ascii_uppercase());
            capitalize_next = false;
        } else if index == 0 {
            result.push(ch.to_ascii_lowercase());
        } else {
            result.push(ch);
        }
    }
    result
}

/// Convert snake_case to PascalCase
pub(super) fn to_pascal_case(s: &str) -> String {
    s.split('_')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(ch) => ch.to_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// Strip _ptr suffix from parameter names
pub(super) fn clean_param_name(name: &str) -> String {
    name.strip_suffix("_ptr").unwrap_or(name).to_string()
}

/// Map ABI type to TypeScript type
pub(super) fn ts_type(abi_type: &AbiType) -> &'static str {
    match abi_type {
        AbiType::Pubkey => "string",
        AbiType::U64 | AbiType::I64 => "bigint",
        AbiType::U32 | AbiType::I32 | AbiType::U16 | AbiType::I16 | AbiType::U8 => "number",
        AbiType::F32 | AbiType::F64 => "number",
        AbiType::Bool => "boolean",
        AbiType::String => "string",
        AbiType::Bytes | AbiType::BytesWithLen => "Uint8Array",
    }
}

/// Map ABI type to Python type hint
pub(super) fn py_type(abi_type: &AbiType) -> &'static str {
    match abi_type {
        AbiType::Pubkey => "str",
        AbiType::U64
        | AbiType::I64
        | AbiType::U32
        | AbiType::I32
        | AbiType::U16
        | AbiType::I16
        | AbiType::U8 => "int",
        AbiType::F32 | AbiType::F64 => "float",
        AbiType::Bool => "bool",
        AbiType::String => "str",
        AbiType::Bytes | AbiType::BytesWithLen => "bytes",
    }
}

/// Detect if a function is read-only based on ABI metadata or heuristic
pub(super) fn is_readonly(function: &AbiFunction) -> bool {
    if function.readonly {
        return true;
    }
    let name = function.name.as_str();
    name.starts_with("get_")
        || name == "balance_of"
        || name == "total_supply"
        || name == "allowance"
        || name == "owner_of"
        || name == "token_uri"
        || name == "name"
        || name == "symbol"
        || name == "decimals"
        || name == "supply"
}
