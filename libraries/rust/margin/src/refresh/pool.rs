//! Refresh margin deposits and pool positions.

use anyhow::Result;

use glow_instructions::{
    derive_pyth_price_feed_account, margin::accounting_invoke, margin_pool::MarginPoolIxBuilder,
    MintInfo,
};
use glow_margin::MarginAccount;
use glow_program_common::oracle::TokenPriceOracle;
use glow_simulation::solana_rpc_api::SolanaRpcClient;
use glow_solana_client::{network::NetworkKind, transaction::TransactionBuilder};
use solana_sdk::pubkey::Pubkey;
use std::{collections::HashMap, sync::Arc};

use crate::{
    get_state::{get_position_metadata, get_token_metadata},
    margin_account_ext::MarginAccountExt,
};

use super::position_refresher::define_refresher;

define_refresher!(PoolRefresher, refresh_all_pool_positions);

/// Identify all pool positions, find metadata, and refresh them.
pub async fn refresh_all_pool_positions(
    rpc: &Arc<dyn SolanaRpcClient>,
    state: &MarginAccount,
) -> Result<Vec<(TransactionBuilder, TokenPriceOracle)>> {
    Ok(refresh_all_pool_positions_underlying_to_tx(rpc, state)
        .await?
        .into_values()
        .collect())
}

/// Identify all pool positions, find metadata, and refresh them.   
/// Map keyed by underlying token mint.
pub async fn refresh_all_pool_positions_underlying_to_tx(
    rpc: &Arc<dyn SolanaRpcClient>,
    state: &MarginAccount,
) -> Result<HashMap<Pubkey, (TransactionBuilder, TokenPriceOracle)>> {
    let network_kind = NetworkKind::from_genesis_hash(&rpc.get_genesis_hash().await.unwrap());
    let pyth_program = network_kind.pyth_oracle();
    let mut txns = HashMap::new();
    let address = state.address();
    for position in state.positions() {
        if position.adapter != glow_margin_pool::ID {
            continue;
        }
        let p_metadata = get_position_metadata(rpc, &state.airspace, &position.token).await?;
        if txns.contains_key(&p_metadata.underlying_token_mint) {
            continue;
        }
        let t_metadata =
            get_token_metadata(rpc, &state.airspace, &p_metadata.underlying_token_mint).await?;
        let ix_builder = MarginPoolIxBuilder::new(
            state.airspace,
            MintInfo::with_token_program(
                p_metadata.underlying_token_mint,
                p_metadata.token_program,
            ),
        );
        let price_oracle = derive_pyth_price_feed_account(
            t_metadata.token_price_oracle.pyth_feed_id().unwrap(),
            None,
            pyth_program,
        );
        let redemption_price_oracle = t_metadata
            .token_price_oracle
            .pyth_redemption_feed_id()
            .map(|feed_id| derive_pyth_price_feed_account(feed_id, None, pyth_program));
        let inner =
            ix_builder.margin_refresh_position(address, price_oracle, redemption_price_oracle);
        let ix = accounting_invoke(state.airspace, address, inner);

        txns.insert(
            p_metadata.underlying_token_mint,
            (ix.into(), t_metadata.token_price_oracle),
        );
    }

    Ok(txns)
}
