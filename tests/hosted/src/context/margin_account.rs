use anchor_lang::prelude::Pubkey;
use anchor_spl::associated_token::spl_associated_token_account::instruction::create_associated_token_account_idempotent;
use anchor_spl::{
    associated_token::ID as ASSOCIATED_TOKEN_ID, token::ID as TOKEN_ID,
    token_2022::ID as TOKEN_2022_ID,
};
use glow_instructions::margin::{refresh_deposit_position, MarginIxBuilder};
use glow_instructions::MintInfo;
use glow_solana_client::{
    signature::Authorization,
    transaction::{TransactionBuilder, WithSigner},
};
use solana_sdk::signer::Signer;

use super::MarginTestContext;

impl MarginTestContext {
    /// Create and register the token account and position if missing.
    pub fn register_deposit_position(
        &self,
        mint: MintInfo,
        margin_account: Authorization,
    ) -> Vec<TransactionBuilder> {
        register_deposit_position(
            mint,
            margin_account,
            self.airspace_details.address,
            self.payer().pubkey(),
        )
    }

    // pub fn refresh_deposit(&self, mint: MintInfo, margin_account: Pubkey) -> TransactionBuilder {
    //     refresh_deposit(mint, margin_account, &self.airspace_details.address)
    // }
}

pub(super) fn register_deposit_position(
    mint: MintInfo,
    margin_account: Authorization,
    airspace: Pubkey,
    payer: Pubkey,
) -> Vec<TransactionBuilder> {
    let create_ata = create_associated_token_account_idempotent(
        &payer,
        &margin_account.address,
        &mint.address,
        &mint.token_program(),
    );
    let builder = MarginIxBuilder::new_for_address(airspace, margin_account.address, payer)
        .with_authority(margin_account.authority.pubkey());
    let register = builder
        .create_deposit_position(mint)
        .with_signer(&margin_account.authority);

    vec![create_ata.into(), register]
}
