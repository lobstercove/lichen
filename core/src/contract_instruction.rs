// Lichen Smart Contract Instructions
// Deploy and invoke WASM contracts

use serde::{Deserialize, Serialize};

/// Smart contract instruction types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ContractInstruction {
    /// Deploy new contract
    /// Accounts: [deployer (signer, writable), contract (writable)]
    /// Data: WASM bytecode
    Deploy {
        /// WASM bytecode
        code: Vec<u8>,
        /// Initial storage data (optional)
        init_data: Vec<u8>,
    },

    /// Call contract function
    /// Accounts: [caller (signer, writable), contract (writable), ...]
    /// Data: function name + arguments
    Call {
        /// Function name to call
        function: String,
        /// Function arguments (serialized)
        args: Vec<u8>,
        /// Value to transfer (in spores)
        value: u64,
    },

    /// Upgrade contract (only owner can upgrade)
    /// Accounts: [owner (signer), contract (writable)]
    /// Data: new WASM bytecode
    Upgrade {
        /// New WASM bytecode
        code: Vec<u8>,
    },

    /// Close contract and withdraw remaining balance
    /// Accounts: [owner (signer), contract (writable), destination (writable)]
    Close,

    /// Set or update the upgrade timelock for a contract.
    /// Once set, upgrades are staged for N epochs before execution.
    /// Setting to 0 removes the timelock (instant upgrades again).
    /// Accounts: [owner (signer), contract (writable)]
    SetUpgradeTimelock {
        /// Number of epochs to delay between submission and execution.
        /// 0 removes the timelock.
        epochs: u32,
    },

    /// Execute a previously staged upgrade after the timelock has expired.
    /// Accounts: [owner (signer), contract (writable)]
    ExecuteUpgrade,

    /// Veto (cancel) a pending upgrade. Only the governance authority can veto.
    /// Accounts: [governance_authority (signer), contract (writable)]
    VetoUpgrade,
}

impl ContractInstruction {
    /// Serialize instruction to bytes
    pub fn serialize(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(self).map_err(|e| e.to_string())
    }

    /// Deserialize instruction from bytes
    pub fn deserialize(data: &[u8]) -> Result<Self, String> {
        serde_json::from_slice(data).map_err(|e| e.to_string())
    }

    /// Create deploy instruction
    pub fn deploy(code: Vec<u8>, init_data: Vec<u8>) -> Self {
        ContractInstruction::Deploy { code, init_data }
    }

    /// Create call instruction
    pub fn call(function: String, args: Vec<u8>, value: u64) -> Self {
        ContractInstruction::Call {
            function,
            args,
            value,
        }
    }

    /// Create upgrade instruction
    pub fn upgrade(code: Vec<u8>) -> Self {
        ContractInstruction::Upgrade { code }
    }

    /// Create close instruction
    pub fn close() -> Self {
        ContractInstruction::Close
    }

    /// Create set upgrade timelock instruction
    pub fn set_upgrade_timelock(epochs: u32) -> Self {
        ContractInstruction::SetUpgradeTimelock { epochs }
    }

    /// Create execute upgrade instruction
    pub fn execute_upgrade() -> Self {
        ContractInstruction::ExecuteUpgrade
    }

    /// Create veto upgrade instruction
    pub fn veto_upgrade() -> Self {
        ContractInstruction::VetoUpgrade
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deploy_instruction() {
        let code = vec![0x00, 0x61, 0x73, 0x6d]; // WASM magic
        let init_data = vec![1, 2, 3];

        let instr = ContractInstruction::deploy(code.clone(), init_data.clone());

        match instr {
            ContractInstruction::Deploy {
                code: c,
                init_data: d,
            } => {
                assert_eq!(c, code);
                assert_eq!(d, init_data);
            }
            _ => panic!("Wrong instruction type"),
        }
    }

    #[test]
    fn test_call_instruction() {
        let instr = ContractInstruction::call("transfer".to_string(), vec![1, 2, 3, 4], 1000);

        match instr {
            ContractInstruction::Call { function, .. } => {
                assert_eq!(function, "transfer");
            }
            _ => panic!("Wrong instruction type"),
        }
    }

    #[test]
    fn test_serialization() {
        let instr = ContractInstruction::call("test".to_string(), vec![1, 2, 3], 0);

        let serialized = instr.serialize().unwrap();
        let deserialized = ContractInstruction::deserialize(&serialized).unwrap();

        assert_eq!(instr, deserialized);
    }
}
