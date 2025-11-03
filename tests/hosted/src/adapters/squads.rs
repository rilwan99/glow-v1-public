//! Instructions and helpers to create stake pools for testing
use std::sync::Arc;

use anchor_lang::{pubkey, AnchorSerialize, Discriminator, InstructionData, ToAccountMetas};
use anyhow::Result;
use glow_margin_sdk::get_state::get_anchor_account;
use glow_simulation::solana_rpc_api::SolanaRpcClient;
use solana_sdk::system_program;
use solana_sdk::{
    account::Account, instruction::Instruction, pubkey::Pubkey, signature::Keypair, signer::Signer,
};
use solana_sdk::{message::Message, system_program::ID as SYSTEM_PROGRAM};
use squads_multisig::client::VaultTransactionCreateAccounts;
use squads_multisig::vault_transaction::VaultTransactionMessageExt;
use squads_multisig_program::{
    state::ProgramConfig, Member, MultisigCreateArgsV2, Permissions, ProposalCreateArgs,
    VaultTransactionCreateArgs, ID as SQUADS_ID,
};
use squads_multisig_program::{ProposalVoteArgs, TransactionMessage};

use crate::send_and_confirm;
use crate::{context::MarginTestContext, Initializer};

pub struct TestSquad {
    pub program_config: Pubkey,
    pub treasury: Pubkey,
    pub authority: Pubkey,
    pub program_id: Pubkey,
}

impl TestSquad {
    /// Create an initialiser for the squad, to be used when initialising other programs
    pub fn initializer() -> Result<(Initializer, Self)> {
        // Add multisig config
        let squads_authority = pubkey!("HM5y4mz3Bt9JY9mr1hkyhnvqxSH4H2u2451j7Hc2dtvK");
        let program_config = ProgramConfig {
            authority: squads_authority,
            multisig_creation_fee: 100,
            treasury: squads_authority,
            _reserved: [0; 64],
        };
        let mut program_config_data = ProgramConfig::discriminator().to_vec();
        program_config.serialize(&mut program_config_data)?;
        let program_config_address =
            squads_multisig::pda::get_program_config_pda(Some(&SQUADS_ID)).0;
        let accounts = vec![
            // Squads wallet
            (
                squads_authority,
                Account {
                    lamports: 1_000_000_000,
                    data: vec![],
                    owner: SYSTEM_PROGRAM,
                    executable: false,
                    rent_epoch: u64::MAX,
                },
            ),
            // Squads program config
            (
                program_config_address,
                Account {
                    lamports: 1_000_000_000,
                    data: program_config_data,
                    owner: SQUADS_ID,
                    executable: false,
                    rent_epoch: u64::MAX,
                },
            ),
        ];

        Ok((
            // Initialiser
            Initializer { accounts },
            Self {
                program_config: program_config_address,
                treasury: squads_authority,
                authority: squads_authority,
                program_id: SQUADS_ID,
            },
        ))
    }

    pub async fn create_multisig(
        &self,
        ctx: &Arc<MarginTestContext>,
        creator: &Keypair,
        members: &[Pubkey],
    ) -> Result<TestMultisig> {
        let create_key = ctx.generate_key();
        let create_key_address = create_key.pubkey();
        let creator_address = creator.pubkey();
        let multisig =
            squads_multisig::pda::get_multisig_pda(&create_key_address, Some(&SQUADS_ID)).0;

        let test_multisig = TestMultisig {
            address: multisig,
            creator: creator_address,
            transaction_index: 0,
        };

        let ix = Instruction {
            program_id: SQUADS_ID,
            accounts: squads_multisig_program::accounts::MultisigCreateV2 {
                program_config: self.program_config,
                treasury: self.treasury,
                multisig,
                create_key: create_key_address,
                creator: creator_address,
                system_program: SYSTEM_PROGRAM,
            }
            .to_account_metas(None),
            data: squads_multisig_program::instruction::MultisigCreateV2 {
                args: MultisigCreateArgsV2 {
                    config_authority: Some(creator_address),
                    threshold: 1,
                    members: members
                        .iter()
                        .map(|m| Member {
                            key: *m,
                            permissions: Permissions { mask: 0b00000111 },
                        })
                        .collect(),
                    time_lock: 0,
                    rent_collector: Some(creator_address),
                    memo: None,
                },
            }
            .data(),
        };

        // Send SOL to the vault to fund it
        let transfer_ix = solana_sdk::system_instruction::transfer(
            &creator_address,
            &test_multisig.vault(0),
            1_000_000_000,
        );

        send_and_confirm(&ctx.rpc(), &[ix, transfer_ix], &[creator, &create_key]).await?;

        Ok(test_multisig)
    }
}

pub struct TestMultisig {
    pub address: Pubkey,
    pub creator: Pubkey,
    pub transaction_index: u64,
}

impl TestMultisig {
    /// Create a multisig transaction and return its address and the proposal address.
    pub async fn create_transaction(
        &mut self,
        rpc: &Arc<dyn SolanaRpcClient>,
        signer: &Keypair,
        instructions: Vec<Instruction>,
        vault_index: u8,
    ) -> Result<(Pubkey, Pubkey)> {
        let transaction_index = self.transaction_index + 1;
        let transaction = squads_multisig::pda::get_transaction_pda(
            &self.address,
            transaction_index,
            Some(&SQUADS_ID),
        )
        .0;
        let vault_pda =
            squads_multisig::pda::get_vault_pda(&self.address, vault_index, Some(&SQUADS_ID)).0;
        let transaction_message = TransactionMessage::try_compile(&vault_pda, &instructions, &[])?;
        let ix = squads_multisig::client::vault_transaction_create(
            VaultTransactionCreateAccounts {
                multisig: self.address,
                transaction,
                creator: self.creator,
                rent_payer: self.creator,
                system_program: SYSTEM_PROGRAM,
            },
            vault_index,
            0,
            &transaction_message,
            None,
            Some(SQUADS_ID),
        );
        // TODO(nev): deliberately left this here, I'd like to figure out what Squads does differently to the below,
        // because we could use that logic for creating bulk transactions.
        // let ix = Instruction {
        //     program_id: SQUADS_ID,
        //     accounts: squads_multisig_program::accounts::VaultTransactionCreate {
        //         multisig: self.address,
        //         creator: self.creator,
        //         rent_payer: self.creator,
        //         system_program: SYSTEM_PROGRAM,
        //         transaction,
        //     }
        //     .to_account_metas(None),
        //     data: squads_multisig_program::instruction::VaultTransactionCreate {
        //         args: VaultTransactionCreateArgs {
        //             vault_index: 0,
        //             ephemeral_signers: 0,
        //             transaction_message: transaction_message.serialize().to_vec(),
        //             memo: None,
        //         },
        //     }
        //     .data(),
        // };
        let (proposal_ix, proposal) = self.create_proposal(transaction_index, false);

        let tx = rpc
            .create_transaction(&[signer], &[ix, proposal_ix])
            .await?;
        rpc.send_and_confirm_transaction(tx).await?;

        self.transaction_index += 1;

        Ok((transaction, proposal))
    }

    pub fn vault(&self, vault_index: u8) -> Pubkey {
        squads_multisig::pda::get_vault_pda(&self.address, vault_index, Some(&SQUADS_ID)).0
    }

    pub async fn approve_and_execute_proposal(
        &self,
        rpc: &Arc<dyn SolanaRpcClient>,
        signer: &Keypair,
        transaction: Pubkey,
        proposal: Pubkey,
        extra_signers: &[&Keypair],
    ) -> Result<()> {
        // Get the transaction to extract its accounts
        let transaction_state = get_anchor_account::<
            squads_multisig_program::state::VaultTransaction,
        >(rpc, &transaction)
        .await?;
        let approval_ix = Instruction {
            program_id: SQUADS_ID,
            accounts: squads_multisig_program::accounts::ProposalVote {
                multisig: self.address,
                proposal,
                member: signer.pubkey(),
            }
            .to_account_metas(None),
            data: squads_multisig_program::instruction::ProposalApprove {
                args: ProposalVoteArgs { memo: None },
            }
            .data(),
        };

        let message: &TransactionMessage =
            unsafe { std::mem::transmute(&transaction_state.message) };
        let mut accounts = squads_multisig_program::accounts::VaultTransactionExecute {
            multisig: self.address,
            proposal,
            transaction,
            member: signer.pubkey(),
        }
        .to_account_metas(None);
        let remaining_accounts =
            message.get_accounts_for_execute(&self.vault(0), &transaction, &[], 0, &SQUADS_ID)?;
        accounts.extend(remaining_accounts);
        let execute_ix = Instruction {
            program_id: SQUADS_ID,
            accounts,
            data: squads_multisig_program::instruction::VaultTransactionExecute {}.data(),
        };

        if extra_signers.is_empty() {
            send_and_confirm(rpc, &[approval_ix, execute_ix], &[signer]).await?;
        } else {
            let mut signers = vec![signer];
            signers.extend(extra_signers.iter().cloned());
            send_and_confirm(rpc, &[approval_ix, execute_ix], &signers).await?;
        }

        Ok(())
    }

    fn create_proposal(&self, transaction_index: u64, draft: bool) -> (Instruction, Pubkey) {
        let proposal = squads_multisig::pda::get_proposal_pda(
            &self.address,
            transaction_index,
            Some(&SQUADS_ID),
        )
        .0;
        let ix = Instruction {
            program_id: SQUADS_ID,
            accounts: squads_multisig_program::accounts::ProposalCreate {
                multisig: self.address,
                proposal,
                creator: self.creator,
                rent_payer: self.creator,
                system_program: SYSTEM_PROGRAM,
            }
            .to_account_metas(None),
            data: squads_multisig_program::instruction::ProposalCreate {
                args: ProposalCreateArgs {
                    transaction_index,
                    draft,
                },
            }
            .data(),
        };

        (ix, proposal)
    }
}
