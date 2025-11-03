//! Refresh margin deposits and pool positions.

use anyhow::Result;
use glow_instructions::{
    derive_pyth_price_feed_account, margin::refresh_deposit_position, MintInfo,
};
use glow_margin::MarginAccount;
use glow_program_common::oracle::TokenPriceOracle;
use glow_simulation::solana_rpc_api::SolanaRpcClient;
use glow_solana_client::{network::NetworkKind, transaction::TransactionBuilder};
use std::sync::Arc;

use crate::{get_state::get_position_config, margin_account_ext::MarginAccountExt};

use super::position_refresher::define_refresher;

define_refresher!(DepositRefresher, refresh_deposit_positions);

/// Refresh direct ATA deposit positions managed by the margin program
pub async fn refresh_deposit_positions(
    rpc: &Arc<dyn SolanaRpcClient>,
    state: &MarginAccount,
) -> Result<Vec<(TransactionBuilder, TokenPriceOracle)>> {
    let pyth_oracle =
        NetworkKind::from_genesis_hash(&rpc.get_genesis_hash().await.unwrap()).pyth_oracle();
    let mut instructions = vec![];
    let address = state.address();
    for position in state.positions() {
        let (_, p_config) = match get_position_config(rpc, &state.airspace, &position.token).await?
        {
            None => continue,
            Some(r) => r,
        };

        if position.token != p_config.underlying_mint {
            continue;
        }

        let oracle = p_config.oracle().unwrap();
        let feed_id = oracle.pyth_feed_id().copied().unwrap();
        let redemption_feed_id = oracle.pyth_redemption_feed_id().copied();

        // TODO(nev)s: From a design perspective, the expectation is that the user will inject
        // an oracle update instruction before calling this.
        // However, that requires integrating with a Hermes service, which is out of the scope
        // of this lib at this point. We can add this integration separately and later.
        let pyth_price_update = derive_pyth_price_feed_account(&feed_id, None, pyth_oracle);
        let pyth_redemption_price_update = redemption_feed_id
            .map(|feed_id| derive_pyth_price_feed_account(&feed_id, None, pyth_oracle));

        let refresh = refresh_deposit_position(
            &state.airspace,
            address,
            if position.is_token_2022 == 1 {
                MintInfo::with_token_2022(position.token)
            } else if position.is_token_2022 == 0 {
                MintInfo::with_legacy(position.token)
            } else {
                panic!("Invalid token program");
            },
            pyth_price_update,
            pyth_redemption_price_update,
            true,
        );
        instructions.push((
            refresh.into(),
            p_config.oracle().expect("Expected an oracle"),
        ));
    }

    Ok(instructions)
}
