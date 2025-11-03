#![allow(unused_imports)]

#[macro_use]
pub mod scenario_setup;

use anchor_spl::token::Mint;
use glow_simulation::solana_rpc_api::SolanaRpcClient;
use solana_sdk::{account::Account, pubkey::Pubkey, signature::Keypair, signer::Signer};

pub mod actions;
pub mod adapters;
pub mod context;
pub mod environment;
pub mod load;
pub mod margin;
pub mod pricing;
pub mod program_test;
pub mod runtime;
pub mod setup_helper;
pub mod slippy;
pub mod test_positions;
pub mod test_user;
pub mod tokens;
pub mod util;

pub fn test_default<T: TestDefault>() -> T {
    TestDefault::test_default()
}

/// Sane defaults that can be used for fields you don't care about.
pub trait TestDefault {
    fn test_default() -> Self;
}

pub async fn send_and_confirm(
    rpc: &std::sync::Arc<dyn SolanaRpcClient>,
    instructions: &[solana_sdk::instruction::Instruction],
    signers: &[&solana_sdk::signature::Keypair],
) -> Result<solana_sdk::signature::Signature, anyhow::Error> {
    let blockhash = rpc.get_latest_blockhash().await?;
    let mut signing_keypairs = vec![rpc.payer()];
    signing_keypairs.extend(signers.iter().map(|k| &**k));

    let tx = solana_sdk::transaction::Transaction::new_signed_with_payer(
        instructions,
        Some(&rpc.payer().pubkey()),
        &signing_keypairs,
        blockhash,
    )
    .into();

    rpc.send_and_confirm_transaction(tx).await
}

/// An initialiser that has the accounts to load, mints and token accounts to create
pub struct Initializer {
    pub accounts: Vec<(Pubkey, Account)>,
    // pub mints: Vec<(Keypair, )
}

// pub struct MintInitializer {
//     pub address: Keypair,
//     pub decimals: u8,
//     pub mint: Mint
// }
