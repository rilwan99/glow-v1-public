//! Instructions and helpers to create stake pools for testing
use std::sync::Arc;

use anchor_spl::{
    associated_token::{self, get_associated_token_address, spl_associated_token_account},
    token::spl_token,
};
use glow_instructions::MintInfo;
use glow_simulation::solana_rpc_api::SolanaRpcClient;
use glow_solana_client::util::Key;
use solana_sdk::native_token::LAMPORTS_PER_SOL;
#[allow(deprecated)]
use solana_sdk::{
    borsh0_10::{get_instance_packed_len, get_packed_len},
    program_pack::Pack,
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    stake, system_instruction,
};
use spl_stake_pool::{
    find_deposit_authority_program_address, find_withdraw_authority_program_address,
    state::{Fee, StakePool, ValidatorList},
    ID as STAKE_POOL_ID, MINIMUM_RESERVE_LAMPORTS,
};

use crate::{context::MarginTestContext, send_and_confirm};

pub struct TestStakePool {
    pub pool: Pubkey,
    pub validator_list: Pubkey,
    pub pool_mint: Pubkey,
    pub reserve_stake: Pubkey,
    pub pool_manager: Pubkey,
    pub pool_mint_fee_address: Pubkey,
}

#[allow(deprecated)]
impl TestStakePool {
    pub async fn new(
        ctx: &Arc<MarginTestContext>,
        authority: &Keypair,
    ) -> Result<Self, anyhow::Error> {
        let pool_mint = ctx.generate_key();

        Self::new_with_pool_mint(ctx, authority, &pool_mint).await
    }

    /// Create a new test stake pool with a predetermined pool mint.
    /// This is useful when integrating with Solayer as we have to create the Solayer pool
    /// before we are able to create the stake pool.
    pub async fn new_with_pool_mint(
        ctx: &Arc<MarginTestContext>,
        authority: &Keypair,
        pool_mint: &Keypair,
    ) -> anyhow::Result<Self> {
        let rpc = ctx.rpc();
        // Keypairs
        let reserve_stake = ctx.generate_key();
        let pool_mint_fee = ctx.generate_key();
        let stake_pool_keypair = ctx.generate_key();
        let validator_list_keypair = ctx.generate_key();
        let independent_staker = ctx.generate_key();

        // Pubkeys
        let pool_authority = authority.pubkey();
        let pool_mint_address = pool_mint.pubkey();
        let pool_mint_fee_address = pool_mint_fee.pubkey();
        let validator_list_address = validator_list_keypair.pubkey();
        let stake_pool_address = stake_pool_keypair.pubkey();
        let reserve_stake_address = reserve_stake.pubkey();
        // let pool_mint_fee_dest = get_associated_token_address(&pool_authority, &pool_mint_address);

        // Rent calculations
        let empty_validator_list = ValidatorList::new(8);
        let validator_list_size = get_instance_packed_len(&empty_validator_list)?;
        let validator_list_lamports = rpc
            .get_minimum_balance_for_rent_exemption(validator_list_size)
            .await?;

        let stake_pool_lamports = rpc
            .get_minimum_balance_for_rent_exemption(get_packed_len::<StakePool>())
            .await?;

        let withdraw_authority =
            find_withdraw_authority_program_address(&STAKE_POOL_ID, &stake_pool_address).0;

        // Create:
        // - pool mint
        // - stake reserve
        // - pool mint fee collection
        let instructions = vec![
            system_instruction::create_account(
                &pool_authority,
                &reserve_stake.pubkey(),
                rpc.get_minimum_balance_for_rent_exemption(200).await? + MINIMUM_RESERVE_LAMPORTS,
                200, // the CLI uses 200 as the constant
                &stake::program::ID,
            ),
            stake::instruction::initialize(
                &reserve_stake.pubkey(),
                &stake::state::Authorized {
                    staker: withdraw_authority,
                    withdrawer: withdraw_authority,
                },
                &stake::state::Lockup::default(),
            ),
            // independent staker
            system_instruction::create_account(
                &authority.pubkey(),
                &independent_staker.pubkey(),
                rpc.get_minimum_balance_for_rent_exemption(200).await? + MINIMUM_RESERVE_LAMPORTS,
                200, // the CLI uses 200 as the constant
                &stake::program::ID,
            ),
            stake::instruction::initialize(
                &independent_staker.pubkey(),
                &stake::state::Authorized {
                    staker: authority.pubkey(),
                    withdrawer: authority.pubkey(),
                },
                &stake::state::Lockup::default(),
            ),
            system_instruction::create_account(
                &pool_authority,
                &pool_mint.pubkey(),
                rpc.get_minimum_balance_for_rent_exemption(spl_token::state::Mint::LEN)
                    .await?,
                spl_token::state::Mint::LEN as u64,
                &spl_token::ID,
            ),
            spl_token::instruction::initialize_mint(
                &spl_token::ID,
                &pool_mint_address,
                &withdraw_authority,
                None,
                9,
            )?,
            system_instruction::create_account(
                &pool_authority,
                &pool_mint_fee_address,
                rpc.get_minimum_balance_for_rent_exemption(spl_token::state::Account::LEN)
                    .await?,
                spl_token::state::Account::LEN as u64,
                &spl_token::ID,
            ),
            spl_token::instruction::initialize_account(
                &spl_token::ID,
                &pool_mint_fee_address,
                &pool_mint_address,
                &pool_authority,
            )?,
        ];

        let init_instructions = vec![
            // validator list
            system_instruction::create_account(
                &pool_authority,
                &validator_list_address,
                validator_list_lamports,
                validator_list_size as u64,
                &spl_stake_pool::ID,
            ),
            // stake pool
            system_instruction::create_account(
                &pool_authority,
                &stake_pool_address,
                stake_pool_lamports,
                get_packed_len::<StakePool>() as u64,
                &spl_stake_pool::ID,
            ),
            // staker
            spl_stake_pool::instruction::initialize(
                &spl_stake_pool::id(),
                &stake_pool_address,
                &pool_authority,              // manager
                &independent_staker.pubkey(), // staker
                &withdraw_authority,          // stake pool withdraw authority
                &validator_list_address,      // validator list
                &reserve_stake_address,       // reserve_stake
                &pool_mint_address,           // pool_mint
                &pool_mint_fee_address,       // manager_pool_account,
                &spl_token::ID,               // token_program_id,
                None,                         // deposit_authority
                Fee {
                    denominator: 10000,
                    numerator: 5,
                }, // fee
                Fee {
                    denominator: 10000,
                    numerator: 2,
                }, // withdrawal_fee
                Fee {
                    denominator: 10000,
                    numerator: 0,
                }, // deposit_fee
                0,                            // referral_fee
                8,                            // max_validators
            ),
        ];

        send_and_confirm(
            &rpc,
            &instructions,
            &[
                authority,
                &reserve_stake,
                &independent_staker,
                pool_mint,
                &pool_mint_fee,
            ],
        )
        .await?;
        send_and_confirm(
            &rpc,
            &init_instructions,
            &[authority, &stake_pool_keypair, &validator_list_keypair],
        )
        .await?;

        Ok(Self {
            pool: stake_pool_address,
            validator_list: validator_list_address,
            pool_mint: pool_mint_address,
            reserve_stake: reserve_stake_address,
            pool_manager: pool_authority,
            pool_mint_fee_address,
        })
    }

    pub async fn deposit_sol(
        &self,
        authority: &Keypair,
        pool_tokens_to: Option<Pubkey>,
        rpc: Arc<dyn SolanaRpcClient + 'static>,
        amount: u64,
    ) -> Result<(), anyhow::Error> {
        let withdraw_authority =
            find_withdraw_authority_program_address(&STAKE_POOL_ID, &self.pool).0;
        let mut ix = vec![];
        // Create an ATA for pool tokens if not provided
        let pool_tokens_to = match pool_tokens_to {
            Some(p) => p,
            None => {
                let mint = MintInfo::with_token_program(self.pool_mint, spl_token::ID);
                ix.push(mint.create_associated_token_account_idempotent(
                    &authority.pubkey(),
                    &authority.pubkey(),
                ));
                mint.associated_token_address(&authority.pubkey())
            }
        };
        let deposit_sol_instr = spl_stake_pool::instruction::deposit_sol(
            &spl_stake_pool::id(),
            &self.pool,
            &withdraw_authority,
            &self.reserve_stake,
            &authority.pubkey(),
            &pool_tokens_to,
            &self.pool_mint_fee_address,
            &Pubkey::default(),
            &self.pool_mint,
            &spl_token::ID,
            // &spl_token::ID,
            amount,
        );
        ix.push(deposit_sol_instr);
        send_and_confirm(&rpc, &ix, &[authority]).await?;
        Ok(())
    }

    pub fn withdrawal_authority(&self) -> Pubkey {
        find_withdraw_authority_program_address(&STAKE_POOL_ID, &self.pool).0
    }
}
