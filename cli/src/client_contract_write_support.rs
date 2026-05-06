use anyhow::Result;
use lichen_core::{ContractInstruction, Hash, Instruction, Keypair, Pubkey};

use crate::client::RpcClient;
use crate::client_tx_support::{serialize_contract_instruction, submit_signed_instruction};

impl RpcClient {
    /// Deploy a smart contract
    pub async fn deploy_contract(
        &self,
        deployer: &Keypair,
        wasm_code: Vec<u8>,
        contract_address: &Pubkey,
        init_data: Vec<u8>,
    ) -> Result<String> {
        let code_hash = Hash::hash(&wasm_code);
        if self.is_code_hash_deploy_blocked(&code_hash).await? {
            anyhow::bail!(
                "Deployment blocked: code hash {} has an active DeployBlocked restriction",
                code_hash.to_hex()
            );
        }

        let contract_ix = ContractInstruction::Deploy {
            code: wasm_code,
            init_data,
        };

        let instruction = Instruction {
            program_id: Pubkey::new([0xFFu8; 32]),
            accounts: vec![deployer.pubkey(), *contract_address],
            data: serialize_contract_instruction(contract_ix)?,
        };

        submit_signed_instruction(self, deployer, instruction).await
    }

    /// Upgrade a deployed smart contract (owner only)
    pub async fn upgrade_contract(
        &self,
        owner: &Keypair,
        wasm_code: Vec<u8>,
        contract_address: &Pubkey,
    ) -> Result<String> {
        let code_hash = Hash::hash(&wasm_code);
        if self.is_code_hash_deploy_blocked(&code_hash).await? {
            anyhow::bail!(
                "Contract upgrade blocked: code hash {} has an active DeployBlocked restriction",
                code_hash.to_hex()
            );
        }

        let contract_ix = ContractInstruction::Upgrade { code: wasm_code };

        let instruction = Instruction {
            program_id: Pubkey::new([0xFFu8; 32]),
            accounts: vec![owner.pubkey(), *contract_address],
            data: serialize_contract_instruction(contract_ix)?,
        };

        submit_signed_instruction(self, owner, instruction).await
    }

    /// Call a smart contract function
    pub async fn call_contract(
        &self,
        caller: &Keypair,
        contract_address: &Pubkey,
        function: String,
        args: Vec<u8>,
        value: u64,
    ) -> Result<String> {
        let contract_ix = ContractInstruction::Call {
            function,
            args,
            value,
        };

        let instruction = Instruction {
            program_id: Pubkey::new([0xFFu8; 32]),
            accounts: vec![caller.pubkey(), *contract_address],
            data: serialize_contract_instruction(contract_ix)?,
        };

        submit_signed_instruction(self, caller, instruction).await
    }
}
