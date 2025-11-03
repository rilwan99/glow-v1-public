//! Set up whirlpool and set liquidity

use anchor_lang::{prelude::*, system_program, InstructionData};
use anchor_spl::associated_token::get_associated_token_address;
use anchor_spl::{
    associated_token::ID as ASSOCIATED_TOKEN_ID, token::ID as TOKEN_ID,
    token_2022::ID as TOKEN_2022_ID,
};
use glow_instructions::MintInfo;
use num_traits::{Pow, ToPrimitive};
use rust_decimal::{Decimal, MathematicalOps};
use solana_sdk::program_pack::Pack;
use solana_sdk::system_instruction;
use solana_sdk::{
    instruction::Instruction, pubkey::Pubkey, signature::Keypair, signer::Signer, sysvar::SysvarId,
};
use solana_test_framework::ClientExtensions;
use stable_swap_client::fees::Fees;
use stable_swap_client::state::SwapInfo;

use glow_simulation::Keygen;
use glow_simulation::{runtime::TestRuntimeRpcClient, DeterministicKeygen};

use crate::tokens::TokenManager;

pub struct TestSaberPool {
    pub address: Pubkey,
    pub authority: Pubkey,
    pub admin: Pubkey,
    pub output_lp: Pubkey,
    pub pool_mint: MintInfo,

    pub reserve_a: Pubkey,
    pub reserve_b: Pubkey,
    pub mint_a: MintInfo,
    pub mint_b: MintInfo,
    pub admin_fees_a: Pubkey,
    pub admin_fees_b: Pubkey,
}

impl TestSaberPool {
    pub async fn create(
        context: &TestRuntimeRpcClient,
        mint_a: MintInfo,
        mint_b: MintInfo,
    ) -> anyhow::Result<TestSaberPool> {
        let admin = context.payer().pubkey();
        // Create input accounts
        // - swap
        let swap = context.keygen.generate_key();
        // - swap authority
        let (swap_authority, swap_authority_nonce) =
            Pubkey::find_program_address(&[swap.pubkey().as_ref()], &stable_swap_client::ID);
        // - token a
        let token_a = mint_a.associated_token_address(&swap_authority);
        // - token b
        let token_b = mint_b.associated_token_address(&swap_authority);

        let swap_info_exemption_amount;

        let (pool_mint, pool_mint_destination, admin_fees_a, admin_fees_b) = {
            let mut c = context.context.write().await;
            c.banks_client
                .create_associated_token_account(
                    &swap_authority,
                    &mint_a.address,
                    context.payer(),
                    &mint_a.token_program(),
                )
                .await
                .unwrap();
            c.banks_client
                .create_associated_token_account(
                    &swap_authority,
                    &mint_b.address,
                    context.payer(),
                    &mint_b.token_program(),
                )
                .await
                .unwrap();

            swap_info_exemption_amount = c
                .banks_client
                .get_rent()
                .await?
                .minimum_balance(std::mem::size_of::<SwapInfo>());

            c.banks_client
                .create_account(
                    context.payer(),
                    &swap,
                    swap_info_exemption_amount,
                    SwapInfo::LEN as u64,
                    stable_swap_client::ID,
                )
                .await
                .unwrap();

            drop(c);

            let token_manager = TokenManager::new(context.clone());
            token_manager
                .mint(mint_a, &swap_authority, &token_a, 100_000_000_000)
                .await?;
            token_manager
                .mint(mint_b, &swap_authority, &token_b, 100_000_000_000)
                .await?;

            let pool_mint = token_manager
                .create_token(6, Some(&swap_authority), None, false)
                .await?;
            let pool_mint_destination = token_manager.create_account(pool_mint, &admin).await?;
            let admin_fees_a = token_manager.create_account(mint_a, &admin).await?;
            let admin_fees_b = token_manager.create_account(mint_b, &admin).await?;

            (pool_mint, pool_mint_destination, admin_fees_a, admin_fees_b)
        };

        // Initialize config
        let mut ixs = vec![];

        let swap_info_ix = stable_swap_client::instruction::initialize(
            &TOKEN_ID,
            &swap.pubkey(),
            &swap_authority,
            &admin,
            &admin_fees_a,
            &admin_fees_b,
            &mint_a.address,
            &token_a,
            &mint_b.address,
            &token_b,
            &pool_mint.address,
            &pool_mint_destination,
            swap_authority_nonce,
            100,
            Fees {
                admin_trade_fee_numerator: 0,
                admin_trade_fee_denominator: 1,
                admin_withdraw_fee_numerator: 0,
                admin_withdraw_fee_denominator: 1,
                trade_fee_numerator: 0,
                trade_fee_denominator: 1,
                withdraw_fee_numerator: 0,
                withdraw_fee_denominator: 1,
            },
        )?;

        ixs.push(swap_info_ix);

        let tx = context
            .create_transaction(&ixs, context.payer(), vec![context.payer(), &swap])
            .await?;
        context.send_and_confirm(tx).await?;

        Ok(TestSaberPool {
            address: swap.pubkey(),
            authority: swap_authority,
            admin,
            output_lp: pool_mint_destination,
            pool_mint,
            reserve_a: token_a,
            reserve_b: token_b,
            mint_a,
            mint_b,
            admin_fees_a,
            admin_fees_b,
        })
    }
}
