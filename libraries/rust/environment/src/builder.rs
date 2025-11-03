use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt::Debug,
    str::FromStr,
    sync::Arc,
};

use glow_margin::TokenConfig;
use glow_program_common::oracle::TokenPriceOracle;
use squads_multisig::{
    client::{ProposalCreateAccounts, ProposalCreateArgs, VaultTransactionCreateAccounts},
    pda::get_vault_pda,
    squads_multisig_program::{self},
    state::TransactionMessage,
    vault_transaction::VaultTransactionMessageExt,
};
use thiserror::Error;

use solana_sdk::{instruction::Instruction, pubkey::Pubkey, signer::Signer, system_program};

use crate::{
    builder::margin::upgrade_token_config,
    config::{EnvironmentConfig, TokenDescription},
};
use glow_instructions::{
    airspace::AirspaceDetails,
    margin::MarginConfigIxBuilder,
    test_service::{derive_token_mint, if_not_initialized},
};
use glow_solana_client::{
    network::NetworkKind,
    rpc::{ClientError, SolanaRpc, SolanaRpcExtra},
    transaction::TransactionBuilder,
};

pub(crate) mod global;
pub(crate) mod margin;
pub(crate) mod margin_pool;

pub use global::{configure_environment, configure_tokens, create_test_tokens, token_context};

/// Descriptions for errors while building the configuration instructions
#[derive(Error, Debug)]
pub enum BuilderError {
    #[error("error using network interface: {0:?}")]
    ClientError(#[from] ClientError),

    #[error("missing mint field for token {0}")]
    MissingMint(String),

    #[error("missing decimals field for token {0}")]
    MissingDecimals(String),

    #[error("no definition for token {0}")]
    UnknownToken(String),

    #[error(
        "connected to the wrong network for the given config: {actual:?} (expected {expected:?})"
    )]
    WrongNetwork {
        expected: NetworkKind,
        actual: NetworkKind,
    },

    #[error("Inconsistent config: {0}")]
    InconsistentConfig(String),
}

/// How will the proposed instructions be executed?
pub enum ProposalExecution {
    /// by creating a governance proposal. The actual instructions will be
    /// executed later.
    Governance(ProposalContext),

    /// by directly submitting a transaction that contains the instructions.
    Direct {
        /// The account that invoked programs may expect to sign the proposed
        /// instructions. You are expected to own this keypair, so you can
        /// directly sign the transactions with it.
        authority: Pubkey,
    },
}

#[derive(Debug)]
pub struct ProposalContext {
    pub payer: Pubkey,
    pub authority: Pubkey,
    pub program: Pubkey,
    pub multisig: Pubkey,
    // pub proposal: Pubkey,
    // pub batch: Pubkey,
    // pub batch_index: u64,
    // pub batch_size: u32,
    pub transaction_index: u64,
    pub vault_index: u8,
}

#[derive(Debug)]
pub struct TokenContext {
    airspace: Pubkey,
    mint: Pubkey,
    price_oracle: TokenPriceOracle,
    desc: TokenDescription,
    token_program: Pubkey,
}

pub struct PlanInstructions {
    pub setup: Vec<Vec<TransactionBuilder>>,
    pub lookup_setup: HashMap<LookupScope, HashSet<Pubkey>>,
    pub propose: Vec<TransactionBuilder>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub enum SetupPhase {
    TokenMints,
    TokenAccounts,
}

pub struct Builder {
    pub(crate) network: NetworkKind,
    pub(crate) interface: Arc<dyn SolanaRpc>,
    pub(crate) signer: Arc<dyn Signer>,
    pub(crate) proposal_execution: ProposalExecution,
    setup_tx: BTreeMap<SetupPhase, Vec<TransactionBuilder>>,
    propose_tx: Vec<TransactionBuilder>,
    addr_lookup_scopes: HashMap<LookupScope, HashSet<Pubkey>>,
}

impl Builder {
    pub async fn new(
        interface: Arc<dyn SolanaRpc>,
        signer: Arc<dyn Signer>,
        proposal_execution: ProposalExecution,
    ) -> Result<Self, BuilderError> {
        Ok(Self {
            network: NetworkKind::from_interface(interface.as_ref()).await?,
            interface,
            proposal_execution,
            setup_tx: BTreeMap::new(),
            propose_tx: vec![],
            addr_lookup_scopes: HashMap::new(),
            signer,
        })
    }

    /// Variant of the normal constructor, with three differences:
    /// ✓ never need to be awaited
    /// ✓ never returns an error
    /// 🗴 NetworkKind must be known in advance
    pub fn new_infallible(
        interface: Arc<dyn SolanaRpc>,
        signer: Arc<dyn Signer>,
        proposal_execution: ProposalExecution,
        network: NetworkKind,
    ) -> Self {
        Self {
            network,
            interface,
            proposal_execution,
            setup_tx: BTreeMap::new(),
            propose_tx: vec![],
            addr_lookup_scopes: HashMap::new(),
            signer,
        }
    }

    pub fn build(self) -> PlanInstructions {
        PlanInstructions {
            setup: self.setup_tx.into_values().collect::<Vec<_>>(),
            propose: self.propose_tx,
            lookup_setup: self.addr_lookup_scopes,
        }
    }

    pub fn payer(&self) -> Pubkey {
        self.signer.pubkey()
    }

    pub fn proposal_payer(&self) -> Pubkey {
        match &self.proposal_execution {
            ProposalExecution::Direct { .. } => self.payer(),
            ProposalExecution::Governance(ctx) => ctx.authority,
        }
    }

    /// Account that invoked programs may expect to sign the proposed instructions.
    pub fn proposal_authority(&self) -> Pubkey {
        match &self.proposal_execution {
            ProposalExecution::Direct { authority } => *authority,
            ProposalExecution::Governance(ctx) => ctx.authority,
        }
    }

    pub async fn account_exists(&self, address: &Pubkey) -> Result<bool, BuilderError> {
        Ok(self.interface.account_exists(address).await?)
    }

    pub async fn upgrade_margin_token_configs(
        &mut self,
        airspace: &Pubkey,
        tokens: &[Pubkey],
    ) -> Result<Vec<Option<TokenConfig>>, BuilderError> {
        // let addresses = tokens
        //     .iter()
        //     .map(|addr| derive_token_config(airspace, addr))
        //     .collect::<Vec<_>>();

        // Loop through each token and migrate if needed
        let mut result = Vec::with_capacity(tokens.len());
        for address in tokens {
            let config = upgrade_token_config(self, airspace, address).await?;
            result.push(config);
        }

        Ok(result)
    }

    // pub async fn get_margin_token_configs(
    //     &self,
    //     airspace: &Pubkey,
    //     tokens: &[Pubkey],
    // ) -> Result<Vec<Option<TokenConfig>>, BuilderError> {
    //     let addresses = tokens
    //         .iter()
    //         .map(|addr| derive_token_config(airspace, addr))
    //         .collect::<Vec<_>>();

    //     Ok(self.interface.try_get_anchor_accounts(&addresses).await?)
    // }

    pub fn setup<T: Into<TransactionBuilder>>(
        &mut self,
        phase: SetupPhase,
        txns: impl IntoIterator<Item = T>,
    ) {
        let setup_tx = match self.setup_tx.get_mut(&phase) {
            Some(tx) => tx,
            None => self.setup_tx.entry(phase).or_default(),
        };

        setup_tx.extend(txns.into_iter().map(|t| t.into()))
    }

    pub fn propose(
        &mut self,
        instructions: impl IntoIterator<Item = Instruction>,
        memo: Option<String>,
    ) {
        let instructions = match &mut self.proposal_execution {
            ProposalExecution::Direct { .. } => instructions
                .into_iter()
                .map(TransactionBuilder::from)
                .collect::<Vec<_>>(),
            ProposalExecution::Governance(ctx) => {
                // use squads_multisig::anchor_lang::{InstructionData, ToAccountMetas};

                // Batch instructions into 3s and create a proposal for each batch
                let ixs = instructions.into_iter().collect::<Vec<_>>();
                if ixs.is_empty() {
                    log::warn!("No actual instructions to propose for {:?}", memo);
                    return;
                }

                let mut batch_instructions = vec![];

                let vault_key =
                    get_vault_pda(&ctx.multisig, 0, Some(&squads_multisig_program::ID)).0;

                let proposal = squads_multisig::pda::get_proposal_pda(
                    &ctx.multisig,
                    ctx.transaction_index + 1,
                    Some(&squads_multisig_program::ID),
                )
                .0;
                log::info!(
                    "Creating proposal {proposal} with memo {:?} at index {}",
                    memo,
                    ctx.transaction_index + 1
                );
                let proposal_ix = squads_multisig::client::proposal_create(
                    ProposalCreateAccounts {
                        multisig: ctx.multisig,
                        proposal,
                        creator: ctx.payer,
                        rent_payer: ctx.payer,
                        system_program: system_program::ID,
                    },
                    ProposalCreateArgs {
                        transaction_index: ctx.transaction_index + 1,
                        draft: false,
                    },
                    Some(squads_multisig_program::ID),
                );
                let message = TransactionMessage::try_compile(&vault_key, &ixs, &[]).unwrap();
                let ix = squads_multisig::client::vault_transaction_create(
                    VaultTransactionCreateAccounts {
                        multisig: ctx.multisig,
                        transaction: squads_multisig::pda::get_transaction_pda(
                            &ctx.multisig,
                            ctx.transaction_index + 1,
                            Some(&squads_multisig_program::ID),
                        )
                        .0,
                        creator: ctx.payer,
                        rent_payer: ctx.payer,
                        system_program: system_program::ID,
                    },
                    ctx.vault_index,
                    0,
                    &message,
                    memo,
                    Some(squads_multisig_program::ID),
                );
                batch_instructions.push(TransactionBuilder::from(vec![ix, proposal_ix]));
                ctx.transaction_index += 1;

                // for instruction in instructions {
                //     ctx.batch_size += 1;
                //     let tx = Transaction::new_with_payer(&[instruction], Some(&ctx.payer));
                //     let batch_tx_pda = Pubkey::find_program_address(
                //         &[
                //             SEED_PREFIX,
                //             ctx.multisig.as_ref(),
                //             SEED_TRANSACTION,
                //             &ctx.batch_index.to_le_bytes(),
                //             SEED_BATCH_TRANSACTION,
                //             &ctx.batch_size.to_le_bytes(),
                //         ],
                //         &squads_multisig_program::ID,
                //     );
                //     let batch_add_tx_ix = Instruction {
                //         program_id: squads_multisig_program::id(),
                //         accounts: squads_multisig_program::accounts::BatchAddTransaction {
                //             multisig: ctx.multisig,
                //             proposal: ctx.proposal,
                //             batch: ctx.batch,
                //             transaction: batch_tx_pda.0,
                //             member: ctx.payer,
                //             rent_payer: ctx.payer,
                //             system_program: system_program::ID,
                //         }
                //         .to_account_metas(None),
                //         data: squads_multisig_program::instruction::BatchAddTransaction {
                //             args: BatchAddTransactionArgs {
                //                 ephemeral_signers: 0, // We don't use ephemeral signers so far
                //                 transaction_message: tx.message_data(),
                //             },
                //         }
                //         .data(),
                //     };
                //     batch_instructions.push(TransactionBuilder::from(batch_add_tx_ix));
                // }
                batch_instructions
            }
        };

        self.propose_tx.extend(instructions);
    }

    pub fn register_lookups(
        &mut self,
        scope: LookupScope,
        address: impl IntoIterator<Item = Pubkey>,
    ) {
        match self.addr_lookup_scopes.get_mut(&scope) {
            Some(addresses) => addresses.extend(address),
            None => {
                _ = self
                    .addr_lookup_scopes
                    .insert(scope, address.into_iter().collect())
            }
        }
    }

    pub(crate) fn margin_config_ix(&self, airspace: &Pubkey) -> MarginConfigIxBuilder {
        MarginConfigIxBuilder::new(
            AirspaceDetails {
                name: "".to_string(),
                address: *airspace,
                authority: self.proposal_authority(),
            },
            self.proposal_payer(),
        )
    }
}

pub(crate) async fn filter_initializers(
    builder: &Builder,
    ixns: impl IntoIterator<Item = (Pubkey, Instruction)>,
) -> Result<Vec<Instruction>, BuilderError> {
    let (accounts, ixns): (Vec<_>, Vec<_>) = ixns.into_iter().unzip();
    let exists = builder.interface.accounts_exist(&accounts).await?;

    Ok(ixns
        .into_iter()
        .enumerate()
        .filter_map(|(idx, ix)| {
            let ix = match builder.network {
                NetworkKind::Localnet => if_not_initialized(accounts[idx], ix),
                _ => ix,
            };

            (!exists[idx]).then_some(ix)
        })
        .collect())
}

pub(crate) fn resolve_token_mint(
    env: &EnvironmentConfig,
    name: &str,
) -> Result<Pubkey, BuilderError> {
    if let Ok(address) = Pubkey::from_str(name) {
        return Ok(address);
    }

    for airspace in &env.airspaces {
        for token in &airspace.tokens {
            if token.name != name {
                continue;
            }

            match token.mint {
                Some(mint) => return Ok(mint),
                None => return Ok(derive_token_mint(name)),
            }
        }
    }

    Err(BuilderError::UnknownToken(name.to_owned()))
}

/// Specifies the scopes that lookup tables are organized into
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LookupScope {
    Airspace,
    Pools,
}
