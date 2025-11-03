use std::sync::Arc;

use anchor_lang::AccountDeserialize;
use anyhow::{Context, Result};
use glow_instructions::{get_metadata_address, margin::derive_token_config};
use glow_margin::{MarginAccount, TokenConfig};
use glow_metadata::{PositionTokenMetadata, TokenMetadata};
use glow_simulation::solana_rpc_api::SolanaRpcClient;
use solana_sdk::pubkey::Pubkey;

use super::get_anchor_account;

pub(crate) async fn get_position_metadata(
    rpc: &Arc<dyn SolanaRpcClient>,
    airspace: &Pubkey,
    position_token_mint: &Pubkey,
) -> Result<PositionTokenMetadata> {
    let md_address = get_metadata_address(airspace, position_token_mint);

    get_anchor_account(rpc, &md_address)
        .await
        .with_context(|| format!("metadata for position token {position_token_mint} {md_address}"))
}

pub(crate) async fn get_position_config(
    rpc: &Arc<dyn SolanaRpcClient>,
    airspace: &Pubkey,
    token_mint: &Pubkey,
) -> Result<Option<(Pubkey, TokenConfig)>> {
    let cfg_address = derive_token_config(airspace, token_mint);
    let account_data = rpc.get_account(&cfg_address).await?;

    match account_data {
        None => Ok(None),
        Some(account) => Ok(Some((
            cfg_address,
            TokenConfig::try_deserialize(&mut &account.data[..])?,
        ))),
    }
}

pub(crate) async fn get_token_metadata(
    rpc: &Arc<dyn SolanaRpcClient>,
    airspace: &Pubkey,
    token_mint: &Pubkey,
) -> Result<TokenMetadata> {
    let md_address = get_metadata_address(airspace, token_mint);

    get_anchor_account(rpc, &md_address)
        .await
        .with_context(|| format!("metadata for token_mint {token_mint}"))
}

/// Get the latest [MarginAccount] state
pub async fn get_margin_account(
    rpc: &Arc<dyn SolanaRpcClient>,
    address: &Pubkey,
) -> Result<MarginAccount> {
    get_anchor_account(rpc, address)
        .await
        .context("margin account")
}
