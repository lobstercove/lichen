use anyhow::Result;

use crate::cli_args::TokenCommands;
use crate::client::RpcClient;
use crate::keypair_manager::KeypairManager;
use crate::token_create_support::{handle_token_create, TokenCreateRequest};
use crate::token_read_support::{handle_token_balance, handle_token_info, handle_token_list};
use crate::token_write_support::{handle_token_mint, handle_token_send};

pub(super) async fn handle_token_command(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    token_cmd: TokenCommands,
) -> Result<()> {
    match token_cmd {
        TokenCommands::Create {
            name,
            symbol,
            wasm,
            decimals,
            initial_supply,
            website,
            logo_url,
            description,
            twitter,
            telegram,
            discord,
            keypair,
        } => {
            handle_token_create(
                client,
                keypair_mgr,
                TokenCreateRequest {
                    name,
                    symbol,
                    wasm,
                    decimals,
                    initial_supply,
                    website,
                    logo_url,
                    description,
                    twitter,
                    telegram,
                    discord,
                    keypair,
                },
            )
            .await?
        }
        TokenCommands::Info { token } => handle_token_info(client, token).await?,
        TokenCommands::Mint {
            token,
            amount,
            to,
            keypair,
        } => handle_token_mint(client, keypair_mgr, token, amount, to, keypair).await?,
        TokenCommands::Send {
            token,
            to,
            amount,
            keypair,
        } => handle_token_send(client, keypair_mgr, token, to, amount, keypair).await?,
        TokenCommands::Balance {
            token,
            address,
            keypair,
        } => handle_token_balance(client, keypair_mgr, token, address, keypair).await?,
        TokenCommands::List => handle_token_list(client).await?,
    }

    Ok(())
}
