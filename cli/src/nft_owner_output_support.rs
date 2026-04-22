use crate::nft_owner_list_output_support::print_owned_nfts;
use crate::output_support::print_json;

pub(super) fn print_nft_owner_result(
    address: &str,
    nfts: &serde_json::Value,
    json_output: bool,
) {
    if json_output {
        print_json(nfts);
    } else {
        print_owned_nfts(address, nfts);
    }
}

pub(super) fn print_nft_owner_error(error: &anyhow::Error, json_output: bool) {
    if json_output {
        print_json(&serde_json::json!({"error": error.to_string()}));
    } else {
        println!("Could not fetch NFTs: {}", error);
    }
}