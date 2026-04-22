use super::*;

/// Build a Lichen contract Call instruction for the "burn" function.
/// Used during withdrawal flow — treasury burns wrapped tokens on behalf of user.
fn _build_contract_burn_instruction(
    contract_pubkey: &Pubkey,
    caller: &Pubkey,
    amount: u64,
) -> Instruction {
    let mut args: Vec<u8> = Vec::with_capacity(40);
    args.extend_from_slice(caller.as_ref());
    args.extend_from_slice(&amount.to_le_bytes());

    let payload = serde_json::json!({
        "Call": {
            "function": "burn",
            "args": args.iter().map(|byte| *byte as u64).collect::<Vec<u64>>(),
            "value": 0
        }
    });
    let data = serde_json::to_vec(&payload).expect("json encode");

    Instruction {
        program_id: Pubkey::new(LICN_CONTRACT_PROGRAM),
        accounts: vec![*caller, *contract_pubkey],
        data,
    }
}

fn _build_system_transfer(from: &Pubkey, to: &Pubkey, amount: u64) -> Instruction {
    let mut data = Vec::with_capacity(9);
    data.push(0u8);
    data.extend_from_slice(&amount.to_le_bytes());
    Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![*from, *to],
        data,
    }
}
