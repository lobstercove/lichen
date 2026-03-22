//! # MoltChain Rust SDK
//!
//! Official Rust SDK for interacting with MoltChain blockchain.
//!
//! ## Features
//!
//! - **Type-safe RPC client** - Interact with validators via JSON-RPC
//! - **Transaction building** - Create and sign transactions
//! - **Keypair management** - Ed25519 keypair generation and management
//! - **Async/await** - Built on Tokio for async operations
//! - **Solana-compatible** - Compatible with Solana wallet formats
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use moltchain_client_sdk::{Client, Keypair};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Connect to validator
//!     let client = Client::new("http://localhost:8899");
//!     
//!     // Generate keypair
//!     let keypair = Keypair::new();
//!     
//!     // Get balance
//!     let balance = client.get_balance(&keypair.pubkey()).await?;
//!     println!("Balance: {} MOLT", balance.molt());
//!     
//!     Ok(())
//! }
//! ```

pub mod client;
pub mod error;
pub mod keypair;
pub mod transaction;
pub mod types;

// Re-exports for convenience
pub use client::{Client, ClientBuilder};
pub use error::{Error, Result};
pub use keypair::{Keypair, Pubkey};
pub use transaction::TransactionBuilder;
pub use types::{Balance, Block, NetworkInfo, Transaction};

// Re-export core types
pub use moltchain_core::{Account, Hash, Instruction, Message, BASE_FEE, SYSTEM_PROGRAM_ID};

/// SDK version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
