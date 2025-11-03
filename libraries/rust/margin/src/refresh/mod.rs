/// refresh direct deposit positions in margin
pub mod deposit;
/// refresh pool positions
pub mod pool;
/// generically represent the idea of refreshing margin account positions
pub mod position_refresher;

use std::sync::Arc;

use glow_simulation::solana_rpc_api::SolanaRpcClient;

use self::{deposit::DepositRefresher, pool::PoolRefresher, position_refresher::SmartRefresher};

/// PositionRefresher that refreshes all known positions within margin.
pub fn canonical_position_refresher(rpc: Arc<dyn SolanaRpcClient>) -> SmartRefresher<()> {
    SmartRefresher {
        refreshers: vec![
            Arc::new(DepositRefresher { rpc: rpc.clone() }),
            Arc::new(PoolRefresher { rpc: rpc.clone() }),
        ],
        rpc,
        margin_account: (),
    }
}
