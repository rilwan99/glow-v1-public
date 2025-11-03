//! Instructions and helpers to create stake pools for testing
use std::{str::FromStr, sync::Arc, u64};

use anchor_lang::{
    pubkey, AccountDeserialize, AnchorSerialize, Discriminator, InstructionData, ToAccountMetas,
};
use anchor_spl::{
    associated_token::{self, get_associated_token_address, spl_associated_token_account},
    token::spl_token::{
        self,
        state::{Account as TokenAccount, AccountState as TokenAccountState, Mint},
    },
};
use anyhow::Result;
use glow_idls::solayer::accounts::RestakingPool;
use glow_margin::program::Margin;
use glow_margin_sdk::{get_state::get_anchor_account, solana::transaction};
use glow_simulation::{solana_rpc_api::SolanaRpcClient, DeterministicKeygen, Keygen};
use glow_solana_client::util::Key;
use solana_sdk::{
    account::Account, instruction::Instruction, program_option::COption, program_pack::Pack,
    pubkey::Pubkey, signature::Keypair, signer::Signer, stake, system_instruction,
};
use solana_sdk::{message::Message, system_program::ID as SYSTEM_PROGRAM};
use squads_multisig_program::{
    state::ProgramConfig, Member, MultisigCreateArgsV2, Permissions, ProgramConfigInitArgs,
    ProposalCreateArgs, VaultTransactionCreateArgs, ID as SQUADS_ID, SEED_PREFIX,
    SEED_PROGRAM_CONFIG,
};

use crate::{context::MarginTestContext, margin_test_context, send_and_confirm, Initializer};

use super::stake_pool::TestStakePool;

pub struct TestSolayer {
    pub pool: Pubkey,
    pub rst_mint: Pubkey,
    pub lst_mint: Pubkey,
    pub lst_vault: Pubkey,
}

impl TestSolayer {
    /// To initialise a Solayer pool, we need the `solayer_admin` to sign for the transaction.
    /// This account is hardcoded in the program, so we can't initialise a pool.
    /// This poses a problem for testing, so to sidestep this, we manually create the Solayer pool.
    /// The pool stores the lst_mint, rst_mint, and its seed, so it's trivial to create.
    ///
    /// This pattern of injecting accounts that we can't create is common when testing, so to simplify
    /// when testing the integration of various programs, this function returns the accounts to add to the runtime,
    /// and the token accounts to create after starting the runtime. We could also add the token accounts before starting
    /// the runtime, but creating them after is more convenient.
    pub fn initializer() -> Result<(Initializer, Keypair, TestSolayer)> {
        // tests/keypairs/tstgPC1UAaCaWHtRYQNBzX9vUFGHpF68c98VJzbQRn3.json
        let lst_mint = Keypair::from_bytes(&[
            196, 58, 122, 122, 48, 253, 179, 92, 163, 18, 100, 210, 220, 56, 187, 115, 56, 248,
            123, 191, 169, 201, 158, 76, 140, 220, 108, 251, 127, 212, 101, 32, 13, 74, 41, 246,
            163, 167, 223, 17, 177, 46, 203, 106, 137, 65, 169, 131, 137, 56, 110, 37, 75, 123, 38,
            65, 116, 169, 254, 150, 101, 133, 103, 44,
        ])?;
        let lst_mint_address = lst_mint.pubkey();
        let (pool, bump) = Pubkey::find_program_address(
            &["pool".as_bytes(), lst_mint_address.as_ref()],
            &glow_idls::solayer::ID,
        );
        let rst_mint = pubkey!("sSo14endRuUbvQaJS3dq36Q829a3A6BEfoeeRGJywEh");

        let restaking_pool = RestakingPool {
            // The LST mint is from the stake pool
            lst_mint: lst_mint_address,
            // The RST mint is created by Solayer's program and 'owned' by the pool
            rst_mint,
            bump,
        };
        let mut data = RestakingPool::discriminator().to_vec();
        restaking_pool.serialize(&mut data)?;
        let pool_account = Account {
            lamports: 1_000_000_000,
            data,
            owner: pubkey!("sSo1iU21jBrU9VaJ8PJib1MtorefUV4fzC9GURa2KNn"),
            executable: false,
            rent_epoch: u64::MAX,
        };
        let mint = Mint {
            mint_authority: COption::Some(pool),
            supply: 0,
            decimals: 9,
            is_initialized: true,
            freeze_authority: COption::Some(pool),
        };
        let mut mint_data = vec![0; Mint::LEN];
        mint.pack_into_slice(&mut mint_data);
        let rst_mint_account = Account {
            lamports: 1_000_000_000,
            data: mint_data,
            owner: anchor_spl::token::ID,
            executable: false,
            rent_epoch: u64::MAX,
        };

        let (lst_vault, lst_vault_account) = Self::get_account_for_lst_mint(lst_mint_address, pool);

        Ok((
            // Initialiser
            Initializer {
                accounts: vec![
                    // solayer pool
                    (pool, pool_account),
                    // rst mint
                    (rst_mint, rst_mint_account),
                    // lst vault
                    (lst_vault, lst_vault_account),
                ],
            },
            // LST mint keypair
            lst_mint,
            // Solayer instance
            TestSolayer {
                pool,
                rst_mint,
                lst_mint: lst_mint_address,
                lst_vault,
            },
        ))
    }

    pub fn get_account_for_lst_mint(lst_mint_address: Pubkey, pool: Pubkey) -> (Pubkey, Account) {
        // LST vault ATA
        let lst_vault = associated_token::get_associated_token_address(&pool, &lst_mint_address);
        let lst_vault_account = TokenAccount {
            mint: lst_mint_address,
            owner: pool,
            amount: 0,
            delegate: COption::None,
            state: TokenAccountState::Initialized,
            is_native: COption::None,
            delegated_amount: 0,
            close_authority: COption::None,
        };
        let mut lst_vault_data = vec![0; TokenAccount::LEN];
        lst_vault_account.pack_into_slice(&mut lst_vault_data);
        let lst_vault_account = Account {
            lamports: 1_000_000_000,
            data: lst_vault_data,
            owner: anchor_spl::token::ID,
            executable: false,
            rent_epoch: u64::MAX,
        };
        (lst_vault, lst_vault_account)
    }

    pub async fn restake(
        &self,
        rpc: &Arc<dyn SolanaRpcClient>,
        signer: &Keypair,
        amount: u64,
    ) -> anyhow::Result<()> {
        let signer_address = signer.pubkey();
        let signer_lst_ata = get_associated_token_address(&signer_address, &self.lst_mint);
        let signer_rst_ata = get_associated_token_address(&signer_address, &self.rst_mint);

        let lst_ata_ix = associated_token::spl_associated_token_account::instruction::create_associated_token_account_idempotent(
            &signer_address,
            &signer_address,
            &self.lst_mint, &anchor_spl::token::ID);
        let rst_ata_ix = associated_token::spl_associated_token_account::instruction::create_associated_token_account_idempotent(
            &signer_address,
            &signer_address,
            &self.rst_mint, &anchor_spl::token::ID);
        let restake_ix = Instruction {
            program_id: glow_idls::solayer::ID,
            accounts: glow_idls::solayer::client::accounts::Restake {
                signer: signer.pubkey(),
                lst_mint: self.lst_mint,
                lst_ata: signer_lst_ata,
                rst_ata: signer_rst_ata,
                rst_mint: self.rst_mint,
                vault: self.lst_vault,
                pool: self.pool,
                associated_token_program: associated_token::ID,
                token_program: anchor_spl::token::ID,
                system_program: SYSTEM_PROGRAM,
            }
            .to_account_metas(None),
            data: glow_idls::solayer::client::args::Restake { amount }.data(),
        };
        let tx = rpc
            .create_transaction(&[signer], &[lst_ata_ix, rst_ata_ix, restake_ix])
            .await?;
        rpc.send_and_confirm_transaction(tx).await?;

        Ok(())
    }
    pub async fn unrestake(
        &self,
        rpc: &Arc<dyn SolanaRpcClient>,
        signer: &Keypair,
        amount: u64,
    ) -> anyhow::Result<()> {
        let signer_address = signer.pubkey();
        let signer_lst_ata = get_associated_token_address(&signer_address, &self.lst_mint);
        let signer_rst_ata = get_associated_token_address(&signer_address, &self.rst_mint);

        let lst_ata_ix = associated_token::spl_associated_token_account::instruction::create_associated_token_account_idempotent(
            &signer_address,
            &signer_address,
            &self.lst_mint, &anchor_spl::token::ID);
        let rst_ata_ix = associated_token::spl_associated_token_account::instruction::create_associated_token_account_idempotent(
            &signer_address,
            &signer_address,
            &self.rst_mint, &anchor_spl::token::ID);
        let restake_ix = Instruction {
            program_id: glow_idls::solayer::ID,
            accounts: glow_idls::solayer::client::accounts::Unrestake {
                signer: signer.pubkey(),
                lst_mint: self.lst_mint,
                lst_ata: signer_lst_ata,
                rst_ata: signer_rst_ata,
                rst_mint: self.rst_mint,
                vault: self.lst_vault,
                pool: self.pool,
                associated_token_program: associated_token::ID,
                token_program: anchor_spl::token::ID,
                system_program: SYSTEM_PROGRAM,
            }
            .to_account_metas(None),
            data: glow_idls::solayer::client::args::Unrestake { amount }.data(),
        };
        let tx = rpc
            .create_transaction(&[signer], &[lst_ata_ix, rst_ata_ix, restake_ix])
            .await?;
        rpc.send_and_confirm_transaction(tx).await?;

        Ok(())
    }
}
