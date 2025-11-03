use std::{rc::Rc, sync::Arc};

use glow_instructions::MintInfo;
use margin::MarginClient;
use solana_sdk::pubkey::Pubkey;

use glow_solana_client::rpc::SolanaRpc;

use client::ClientState;
use config::GlowAppConfig;
use state::{tokens::TokenAccount, AccountStates};

mod client;
pub mod config;
pub mod margin;
pub mod margin_pool;
pub mod state;
pub mod test_service;
mod wallet;

pub use client::{ClientError, ClientResult};
pub use glow_solana_client::network::NetworkKind;
use test_service::TestServiceClient;
pub use wallet::Wallet;

/// Central client object for interacting with the protocol
#[derive(Clone)]
pub struct GlowClient {
    client: Arc<ClientState>,
}

impl GlowClient {
    /// Create the client state
    pub fn new(
        interface: Arc<dyn SolanaRpc>,
        wallet: Rc<dyn Wallet>,
        config: GlowAppConfig,
        airspace: &str,
        network_kind: NetworkKind,
    ) -> ClientResult<Self> {
        Ok(Self {
            client: Arc::new(ClientState::new(
                interface,
                wallet,
                config,
                airspace.to_owned(),
                network_kind,
            )?),
        })
    }

    /// The airspace this client is associated with
    pub fn airspace(&self) -> Pubkey {
        self.client.airspace()
    }

    /// Get the state management object for this client
    pub fn state(&self) -> &AccountStates {
        self.client.state()
    }

    /// Get the balance of a token owned by the user's wallet
    pub fn wallet_balance(&self, token: MintInfo) -> u64 {
        let address = token.associated_token_address(&self.client.signer());

        self.client
            .state()
            .get::<TokenAccount>(&address)
            .map(|account| account.amount)
            .unwrap_or_default()
    }

    /// Get the client for the test service program
    pub fn test_service(&self) -> TestServiceClient {
        TestServiceClient::new(self.client.clone())
    }

    /// Get the client for the margin program
    pub fn margin(&self) -> MarginClient {
        MarginClient::new(self.client.clone())
    }
}

// macro_rules! bail {
//     ($($fmt_args:tt)*) => {
//         return Err($crate::client::ClientError::Unexpected(format!($($fmt_args)*)))
//     };
// }

// pub(crate) use bail;
