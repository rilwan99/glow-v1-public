// SPDX-License-Identifier: AGPL-3.0-or-later
//
// Copyright (C) 2024 A1 XYZ, INC.
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use std::collections::HashSet;

use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use anchor_spl::associated_token::ID as ASSOCIATED_TOKEN_ID;
use glow_margin::seeds::MARGIN_ACCOUNT_CONSTRAINT_SEED;
use glow_margin::AccountFeatureFlags;
use solana_sdk::instruction::Instruction;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::system_program::ID as SYSTEM_PROGRAM_ID;
use solana_sdk::sysvar::{rent::Rent, SysvarId};

use anchor_lang::prelude::{AccountMeta, Id, System, ToAccountMetas};
use anchor_lang::{system_program, InstructionData};

use glow_margin::instruction as ix_data;
use glow_margin::program::Margin;
use glow_margin::seeds::{ADAPTER_CONFIG_SEED, PERMIT_SEED, TOKEN_CONFIG_SEED};
use glow_margin::{accounts as ix_account, MarginAccount};
use glow_program_common::ADDRESS_LOOKUP_REGISTRY_ID;

pub use glow_margin::ID as MARGIN_PROGRAM;
pub use glow_margin::{TokenAdmin, TokenConfigUpdate, TokenKind};

use crate::airspace::derive_permit;
use crate::airspace::AirspaceDetails;
use crate::airspace::AirspaceIxBuilder;
use crate::MintInfo;

/// Utility for creating instructions to interact with the margin
/// program for a specific account.
#[derive(Clone)]
pub struct MarginIxBuilder {
    /// Owner of the margin account.
    pub owner: Pubkey,

    /// Seed used to generate margin account PDA.
    pub seed: u16,

    /// The address of the margin account.
    pub address: Pubkey,

    /// Details of the airspace the margin account belongs to
    pub airspace_details: AirspaceDetails,

    /// The account paying for any rent.
    /// - Defaults to authority, which defaults to owner.
    payer: Option<Pubkey>,

    /// Key that will sign to authorize changes to the margin account.
    /// - Defaults to owner.
    authority: Option<Pubkey>,
}

impl MarginIxBuilder {
    /// Create a new [MarginIxBuilder] which uses the margin account as the authority.
    /// Ordinary margin users should use this function to create a builder.
    pub fn new(airspace: Pubkey, owner: Pubkey, seed: u16) -> Self {
        let address = derive_margin_account(&airspace, &owner, seed);
        Self {
            owner,
            seed,
            payer: None,
            address,
            airspace_details: AirspaceDetails::from_address(airspace),
            authority: None,
        }
    }

    pub fn new_for_address(airspace: Pubkey, address: Pubkey, payer: Pubkey) -> Self {
        Self {
            owner: payer,
            seed: 0,
            payer: Some(payer),
            address,
            airspace_details: AirspaceDetails::from_address(airspace),
            authority: None,
        }
    }

    /// Use if an administrator is managing the account instead of the owner.
    pub fn with_authority(mut self, authority: Pubkey) -> Self {
        self.authority = Some(authority);
        self
    }

    /// Use if a different wallet should pay or receive rent instead of the authority.
    pub fn with_payer(mut self, payer: Pubkey) -> Self {
        self.payer = Some(payer);
        self
    }

    pub fn authority(&self) -> Pubkey {
        self.authority.unwrap_or(self.owner)
    }

    pub fn payer(&self) -> Pubkey {
        self.payer.unwrap_or_else(|| self.authority())
    }

    /// the instruction is expected to be signed by the margin account
    pub fn needs_signature(&self, inner: &Instruction) -> bool {
        inner
            .accounts
            .iter()
            .any(|a| a.is_signer && self.address == a.pubkey)
    }

    /// Get instruction to create the account
    pub fn create_permit(&self) -> Instruction {
        let builder = AirspaceIxBuilder::new_from_address(
            self.airspace_details.address,
            self.owner,
            self.owner,
        );
        builder.permit_create(self.owner)
    }

    /// Get instruction to create the account
    pub fn create_account(&self, features: AccountFeatureFlags) -> Instruction {
        let accounts = ix_account::CreateAccount {
            owner: self.owner,
            permit: derive_permit(&self.airspace_details.address, &self.owner),
            payer: self.payer(),
            margin_account: self.address,
            system_program: SYSTEM_PROGRAM_ID,
        };
        Instruction {
            program_id: Margin::id(),
            data: ix_data::CreateAccount {
                seed: self.seed,
                features: features.bits(),
            }
            .data(),
            accounts: accounts.to_account_metas(None),
        }
    }

    /// Get instruction to close account
    pub fn close_account(&self) -> Instruction {
        let accounts = ix_account::CloseAccount {
            owner: self.owner,
            receiver: self.payer(),
            margin_account: self.address,
        };

        Instruction {
            program_id: Margin::id(),
            data: ix_data::CloseAccount.data(),
            accounts: accounts.to_account_metas(None),
        }
    }

    /// Get instruction to create address lookup registry account
    pub fn init_lookup_registry(&self) -> Instruction {
        let registry_account = self.lookup_table_registry_address();
        let accounts = ix_account::InitLookupRegistry {
            authority: self.authority(),
            payer: self.payer(),
            margin_account: self.address,
            registry_account,
            registry_program: ADDRESS_LOOKUP_REGISTRY_ID,
            system_program: SYSTEM_PROGRAM_ID,
        }
        .to_account_metas(None);

        Instruction {
            program_id: Margin::id(),
            data: ix_data::InitLookupRegistry.data(),
            accounts,
        }
    }

    /// Get instruction to create a new lookup table in a lookup registry account
    pub fn create_lookup_table(&self, slot: u64) -> (Instruction, Pubkey) {
        let (lookup_table, _) =
            solana_address_lookup_table_program::instruction::derive_lookup_table_address(
                &self.address,
                slot,
            );
        let accounts = ix_account::CreateLookupTable {
            authority: self.authority(),
            payer: self.payer(),
            margin_account: self.address,
            registry_account: self.lookup_table_registry_address(),
            registry_program: ADDRESS_LOOKUP_REGISTRY_ID,
            system_program: SYSTEM_PROGRAM_ID,
            lookup_table,
            address_lookup_table_program: solana_address_lookup_table_program::id(),
        }
        .to_account_metas(None);

        (
            Instruction {
                program_id: Margin::id(),
                data: ix_data::CreateLookupTable {
                    recent_slot: slot,
                    discriminator: 10,
                }
                .data(),
                accounts,
            },
            lookup_table,
        )
    }

    /// Get instruction to append accounts to a lookup table
    pub fn append_to_lookup_table(
        &self,
        lookup_table: Pubkey,
        addresses: &[Pubkey],
    ) -> Instruction {
        let accounts = ix_account::AppendToLookup {
            authority: self.authority(),
            payer: self.payer(),
            margin_account: self.address,
            registry_account: self.lookup_table_registry_address(),
            registry_program: ADDRESS_LOOKUP_REGISTRY_ID,
            system_program: SYSTEM_PROGRAM_ID,
            lookup_table,
            address_lookup_table_program: solana_address_lookup_table_program::id(),
        }
        .to_account_metas(None);

        Instruction {
            program_id: Margin::id(),
            data: ix_data::AppendToLookup {
                addresses: addresses
                    .iter()
                    .cloned()
                    .collect::<HashSet<_>>()
                    .into_iter()
                    .collect(),
            }
            .data(),
            accounts,
        }
    }

    /// Get instruction to update the accounting for assets in
    /// the custody of the margin account.
    ///
    /// # Params
    ///
    /// `account` - The account address that has had a balance change
    pub fn update_position_balance(&self, account: Pubkey) -> Instruction {
        update_position_balance(self.address, account)
    }

    /// Get instruction to register new position
    ///
    /// # Params
    ///
    /// `position_token_mint` - The mint for the relevant token for the position
    /// `token_oracle` - The oracle account with price information on the token
    ///
    /// # Returns
    ///
    /// Returns the instruction, and the address of the token account to be
    /// created for the position.
    pub fn register_position(&self, position_token_mint: MintInfo) -> Instruction {
        let token_account =
            derive_position_token_account(&self.address, &position_token_mint.address);

        let config = MarginConfigIxBuilder::new(self.airspace_details.clone(), self.payer())
            .derive_token_config(&position_token_mint.address);

        let accounts = ix_account::RegisterPosition {
            authority: self.authority(),
            payer: self.payer(),
            margin_account: self.address,
            position_token_mint: position_token_mint.address,
            config,
            token_account,
            token_program: position_token_mint.token_program(),
            system_program: System::id(),
            rent: Rent::id(),
        };

        Instruction {
            program_id: Margin::id(),
            data: ix_data::RegisterPosition {}.data(),
            accounts: accounts.to_account_metas(None),
        }
    }

    /// Get instruction to close a position
    ///
    /// # Params
    ///
    /// `position_token_mint` - The address of the token mint for the position, this is the
    ///   pool token mint, not the SPL mint.
    /// `token_account` - The address of the token account for the position being closed.
    pub fn close_position(
        &self,
        position_token_mint: MintInfo,
        token_account: Pubkey,
    ) -> Instruction {
        let accounts = ix_account::ClosePosition {
            authority: self.authority(),
            receiver: self.payer(),
            margin_account: self.address,
            position_token_mint: position_token_mint.address,
            token_account,
            token_program: position_token_mint.token_program(),
        };

        Instruction {
            program_id: Margin::id(),
            data: ix_data::ClosePosition.data(),
            accounts: accounts.to_account_metas(None),
        }
    }

    /// Get instruction to refresh the config for a position
    ///
    /// # Params
    ///
    /// `position_token_mint` - The mint for the position to be refreshed
    pub fn refresh_position_config(&self, position_token_mint: &Pubkey) -> Instruction {
        let config = MarginConfigIxBuilder::new(self.airspace_details.clone(), self.payer())
            .derive_token_config(position_token_mint);

        let accounts = ix_account::RefreshPositionConfig {
            config,
            margin_account: self.address,
            permit: derive_margin_permit(&self.airspace_details.address, &self.authority()),
            refresher: self.authority(),
        };

        Instruction {
            program_id: Margin::id(),
            data: ix_data::RefreshPositionConfig.data(),
            accounts: accounts.to_account_metas(None),
        }
    }

    /// Get instruction to refresh the price value for a deposit account
    ///
    /// # Params
    ///
    /// `token_config` - The token config for the position to be refreshed
    /// `price_oracle` - The price oracle for the token, stored in the token config
    pub fn refresh_deposit_position(
        &self,
        mint: MintInfo,
        price_oracle: &Pubkey,
        redemption_oracle: Option<Pubkey>,
        refresh_balance: bool,
    ) -> Instruction {
        refresh_deposit_position(
            &self.airspace_details.address,
            self.address,
            mint,
            *price_oracle,
            redemption_oracle,
            refresh_balance,
        )
    }

    /// Get instruction to invoke through an adapter
    ///
    /// # Params
    ///
    /// `adapter_ix` - The instruction to be invoked
    pub fn adapter_invoke(&self, adapter_ix: Instruction) -> Instruction {
        invoke_single!(
            self.airspace_details.address,
            self.address,
            adapter_ix,
            AdapterInvoke { owner: self.owner }
        )
    }

    pub fn adapter_invoke_many(&self, adapter_ixs: &[Instruction]) -> Instruction {
        let adapter_ixs = adapter_ixs.to_vec();
        invoke_many!(
            self.airspace_details.address,
            self.address,
            adapter_ixs,
            AdapterInvoke { owner: self.owner }
        )
    }

    /// Get instruction to invoke through an adapter for permissionless accounting instructions
    ///
    /// # Params
    ///
    /// `adapter_ix` - The instruction to be invoked
    pub fn accounting_invoke(&self, adapter_ix: Instruction) -> Instruction {
        accounting_invoke(self.airspace_details.address, self.address, adapter_ix)
    }

    /// Begin liquidating a margin account
    ///
    /// # Params
    ///
    /// `liquidator` - The address of the liquidator
    pub fn liquidate_begin(&self) -> Instruction {
        liquidate_begin(
            self.airspace_details.address,
            self.address,
            self.authority(),
            self.payer(),
        )
    }

    /// Invoke action as liquidator
    pub fn liquidator_invoke(&self, adapter_ix: Instruction, fee_mint: MintInfo) -> Instruction {
        liquidator_invoke(
            self.airspace_details.address,
            self.authority(),
            self.address,
            fee_mint,
            adapter_ix,
        )
    }

    /// Invoke actions as liquidator
    pub fn liquidator_invoke_many(
        &self,
        adapter_ixs: &[Instruction],
        fee_mint: MintInfo,
    ) -> Instruction {
        liquidator_invoke_many(
            self.airspace_details.address,
            self.authority(),
            self.address,
            fee_mint,
            adapter_ixs.to_vec(),
        )
    }

    pub fn collect_liquidation_fee(
        &self,
        token_mint: MintInfo,
        token_config: Pubkey,
        price_oracle: Pubkey,
        redemption_quote_oracle: Option<Pubkey>,
    ) -> Instruction {
        collect_liquidation_fee(
            self.address,
            self.authority(),
            token_mint,
            token_config,
            price_oracle,
            redemption_quote_oracle,
        )
    }

    /// End liquidating a margin account
    ///
    /// # Params
    ///
    /// `liquidator` - The address of the liquidator
    /// `original_liquidator` - The liquidator that started the liquidation process
    pub fn liquidate_end(&self, original_liquidator: Option<Pubkey>) -> Instruction {
        let authority = self.authority();
        liquidate_end(
            self.address,
            original_liquidator.unwrap_or(authority),
            authority,
        )
    }

    /// Create a new token account registered as a position
    ///
    /// Can be used to deposit tokens into the custody of the margin account, without
    /// the use of any adapters to manage it.
    ///
    /// # Params
    ///
    /// `token_mint` - The mint for the token to be deposited
    pub fn create_deposit_position(&self, token_mint: MintInfo) -> Instruction {
        create_deposit_position(
            self.address,
            self.airspace_details.address,
            self.authority(),
            self.payer(),
            token_mint,
        )
    }

    /// Transfer tokens into or out of a deposit account associated with the margin account
    pub fn transfer_deposit(
        &self,
        source_owner: Pubkey,
        source: Pubkey,
        destination: Pubkey,
        mint: MintInfo,
        amount: u64,
    ) -> Instruction {
        let accounts = ix_account::TransferDeposit {
            owner: self.owner,
            margin_account: self.address,
            source_owner,
            source,
            destination,
            mint: mint.address,
            token_program: mint.token_program(),
        };

        Instruction {
            program_id: glow_margin::ID,
            data: ix_data::TransferDeposit { amount }.data(),
            accounts: accounts.to_account_metas(None),
        }
    }

    /// Verify that an account is healthy
    ///
    pub fn verify_healthy(&self) -> Instruction {
        let accounts = ix_account::VerifyHealthy {
            margin_account: self.address,
        };

        Instruction {
            program_id: Margin::id(),
            accounts: accounts.to_account_metas(None),
            data: ix_data::VerifyHealthy.data(),
        }
    }

    /// Verify that an account is unhealthy
    ///
    pub fn verify_unhealthy(&self) -> Instruction {
        let accounts = ix_account::VerifyUnhealthy {
            margin_account: self.address,
        };

        Instruction {
            program_id: Margin::id(),
            accounts: accounts.to_account_metas(None),
            data: ix_data::VerifyUnhealthy.data(),
        }
    }

    /// Peform an administrative transfer for a position
    pub fn admin_transfer_position_to(
        &self,
        target: &Pubkey,
        position_token_mint: MintInfo,
        amount: u64,
    ) -> Instruction {
        let accounts = ix_account::AdminTransferPosition {
            authority: glow_program_common::PROTOCOL_GOVERNOR_ID,
            source_account: self.address,
            target_account: *target,
            source_token_account: self.get_token_account_address(&position_token_mint.address),
            target_token_account: derive_position_token_account(
                target,
                &position_token_mint.address,
            ),
            token_mint: position_token_mint.address,
            token_program: position_token_mint.token_program(),
        }
        .to_account_metas(None);

        Instruction {
            program_id: glow_margin::ID,
            data: ix_data::AdminTransferPosition { amount }.data(),
            accounts,
        }
    }

    /// Helper function to get token account address for a position mint
    #[inline]
    pub fn get_token_account_address(&self, position_token_mint: &Pubkey) -> Pubkey {
        derive_position_token_account(&self.address, position_token_mint)
    }

    /// Address of the lookup table registry for this account
    pub fn lookup_table_registry_address(&self) -> Pubkey {
        Pubkey::find_program_address(&[self.address.as_ref()], &ADDRESS_LOOKUP_REGISTRY_ID).0
    }
}

/// Get instruction to update the accounting for assets in
/// the custody of the margin account.
///
/// # Params
///
/// `account` - The account address that has had a balance change
pub fn update_position_balance(margin_account: Pubkey, token_account: Pubkey) -> Instruction {
    let accounts = ix_account::UpdatePositionBalance {
        margin_account,
        token_account,
    };
    Instruction {
        program_id: Margin::id(),
        data: ix_data::UpdatePositionBalance.data(),
        accounts: accounts.to_account_metas(None),
    }
}

pub fn create_deposit_account_and_position(
    margin_account: Pubkey,
    airspace: Pubkey,
    authority: Pubkey,
    payer: Pubkey,
    mint: MintInfo,
) -> Vec<Instruction> {
    vec![
        mint.create_associated_token_account_idempotent(&margin_account, &payer),
        create_deposit_position(margin_account, airspace, authority, payer, mint),
    ]
}

/// Construct a CreateDepositPosition instruction
pub fn create_deposit_position(
    margin_account: Pubkey,
    airspace: Pubkey,
    authority: Pubkey,
    payer: Pubkey,
    mint: MintInfo,
) -> Instruction {
    let config_ix = MarginConfigIxBuilder::new(AirspaceDetails::from_address(airspace), payer);
    let token_account = mint.associated_token_address(&margin_account);
    let accounts = ix_account::CreateDepositPosition {
        margin_account,
        authority,
        payer,
        mint: mint.address,
        config: config_ix.derive_token_config(&mint.address),
        token_account,
        associated_token_program: ASSOCIATED_TOKEN_ID,
        mint_token_program: mint.token_program(),
        system_program: system_program::ID,
        rent: Rent::id(),
    };
    Instruction {
        program_id: glow_margin::ID,
        accounts: accounts.to_account_metas(None),
        data: ix_data::CreateDepositPosition.data(),
    }
}

pub fn liquidate_begin(
    airspace: Pubkey,
    margin_account: Pubkey,
    liquidator: Pubkey,
    payer: Pubkey,
) -> Instruction {
    let permit = derive_margin_permit(&airspace, &liquidator);
    let liquidation = derive_liquidation(margin_account, liquidator);
    let accounts = glow_margin::accounts::LiquidateBegin {
        margin_account,
        payer,
        liquidator,
        permit,
        liquidation,
        system_program: system_program::ID,
    };
    Instruction {
        program_id: Margin::id(),
        accounts: accounts.to_account_metas(None),
        data: glow_margin::instruction::LiquidateBegin {}.data(),
    }
}

pub fn collect_liquidation_fee(
    margin_account: Pubkey,
    liquidator: Pubkey,
    token_mint: MintInfo,
    token_config: Pubkey,
    price_oracle: Pubkey,
    redemption_quote_oracle: Option<Pubkey>,
) -> Instruction {
    let accounts = glow_margin::accounts::CollectLiquidationFee {
        liquidator,
        liquidation_fee_mint: token_mint.address,
        liquidation: derive_liquidation(margin_account, liquidator),
        liquidator_fee_token: token_mint.associated_token_address(&liquidator),
        liquidator_fee_token_program: token_mint.token_program(),
        margin_account,
        token_config,
        margin_account_fee_source: token_mint.associated_token_address(&margin_account),
        price_oracle,
        redemption_quote_oracle,
    };
    Instruction {
        program_id: Margin::id(),
        accounts: accounts.to_account_metas(None),
        data: glow_margin::instruction::CollectLiquidationFee {}.data(),
    }
}

pub fn liquidate_end(
    margin_account: Pubkey,
    original_liquidator: Pubkey,
    authority: Pubkey,
) -> Instruction {
    let liquidation = derive_liquidation(margin_account, original_liquidator);
    let accounts = ix_account::LiquidateEnd {
        margin_account,
        authority,
        liquidation,
    };
    Instruction {
        program_id: Margin::id(),
        accounts: accounts.to_account_metas(None),
        data: ix_data::LiquidateEnd.data(),
    }
}

/// Get instruction to refresh the price and balance value for a deposit account
///
/// # Params
///
/// `token_config` - The token config for the position to be refreshed
/// `price_oracle` - The price oracle for the token, stored in the token config
pub fn refresh_deposit_position(
    airspace: &Pubkey,
    margin_account: Pubkey,
    mint: MintInfo,
    price_oracle: Pubkey,
    redemption_oracle: Option<Pubkey>,
    refresh_balance: bool,
) -> Instruction {
    let mut accounts = ix_account::RefreshDepositPosition {
        config: derive_token_config(airspace, &mint.address),
        price_oracle,
        redemption_quote_oracle: redemption_oracle,
        margin_account,
    }
    .to_account_metas(None);
    if refresh_balance {
        accounts.push(AccountMeta {
            pubkey: get_associated_token_address_with_program_id(
                &margin_account,
                &mint.address,
                &mint.token_program(),
            ),
            is_signer: false,
            is_writable: false,
        });
    }

    Instruction {
        program_id: Margin::id(),
        data: ix_data::RefreshDepositPosition.data(),
        accounts,
    }
}

/// Get instruction to invoke through an adapter
///
/// # Params
///
/// `adapter_ix` - The instruction to be invoked
pub fn adapter_invoke(
    airspace: Pubkey,
    owner: Pubkey,
    margin_account: Pubkey,
    adapter_ix: Instruction,
) -> Instruction {
    invoke_single!(
        airspace,
        margin_account,
        adapter_ix,
        AdapterInvoke { owner }
    )
}

pub fn adapter_invoke_many(
    airspace: Pubkey,
    owner: Pubkey,
    margin_account: Pubkey,
    adapter_ixs: Vec<Instruction>,
) -> Instruction {
    invoke_many!(
        airspace,
        margin_account,
        adapter_ixs,
        AdapterInvoke { owner }
    )
}

/// Invoke action as liquidator
pub fn liquidator_invoke(
    airspace: Pubkey,
    liquidator: Pubkey,
    margin_account: Pubkey,
    fee_mint: MintInfo,
    adapter_ix: Instruction,
) -> Instruction {
    let liquidation = derive_liquidation(margin_account, liquidator);

    let liquidator_fee_mint = fee_mint.address;
    let liquidator_fee_token_program = fee_mint.token_program();

    invoke_single!(
        airspace,
        margin_account,
        adapter_ix,
        LiquidatorInvoke {
            liquidator,
            liquidation,
            liquidator_fee_mint,
            liquidator_fee_token_program,
        }
    )
}

/// Invoke action as liquidator
pub fn liquidator_invoke_many(
    airspace: Pubkey,
    liquidator: Pubkey,
    margin_account: Pubkey,
    fee_mint: MintInfo,
    adapter_ixs: Vec<Instruction>,
) -> Instruction {
    let liquidation = derive_liquidation(margin_account, liquidator);

    let liquidator_fee_mint = fee_mint.address;
    let liquidator_fee_token_program = fee_mint.token_program();

    invoke_many!(
        airspace,
        margin_account,
        adapter_ixs,
        LiquidatorInvoke {
            liquidator,
            liquidation,
            liquidator_fee_mint,
            liquidator_fee_token_program,
        }
    )
}

/// Get instruction to invoke through an adapter for permissionless accounting instructions
///
/// # Params
///
/// `adapter_ix` - The instruction to be invoked
pub fn accounting_invoke(
    airspace: Pubkey,
    margin_account: Pubkey,
    adapter_ix: Instruction,
) -> Instruction {
    invoke_single!(airspace, margin_account, adapter_ix, AccountingInvoke)
}

pub fn accounting_invoke_many(
    airspace: Pubkey,
    margin_account: Pubkey,
    adapter_ixs: Vec<Instruction>,
) -> Instruction {
    invoke_many!(airspace, margin_account, adapter_ixs, AccountingInvoke)
}

/// Utility for creating instructions that modify configuration for the margin program within
/// an airspace
#[derive(Eq, PartialEq, Clone)]
pub struct MarginConfigIxBuilder {
    airspace_details: AirspaceDetails,
    payer: Pubkey,
}

impl MarginConfigIxBuilder {
    /// Create a new [MarginConfigIxBuilder] for a given airspace, assuming the
    /// payer is the authority if not provided.
    pub fn new(airspace_details: AirspaceDetails, payer: Pubkey) -> Self {
        Self {
            airspace_details,
            payer,
        }
    }

    /// Set the configuration for a token that can be used as a position within a margin account
    pub fn configure_token(
        &self,
        token_mint: Pubkey,
        new_config: TokenConfigUpdate,
    ) -> Instruction {
        let accounts = ix_account::ConfigureToken {
            authority: self.airspace_details.authority,
            airspace: self.airspace_details.address,
            payer: self.payer,
            mint: token_mint,
            token_config: self.derive_token_config(&token_mint),
            system_program: system_program::ID,
        };

        Instruction {
            program_id: glow_margin::ID,
            data: ix_data::ConfigureToken { update: new_config }.data(),
            accounts: accounts.to_account_metas(None),
        }
    }

    /// Set the configuration for an adapter program
    pub fn configure_adapter(&self, program_id: Pubkey, is_adapter: bool) -> Instruction {
        let accounts = ix_account::ConfigureAdapter {
            authority: self.airspace_details.authority,
            airspace: self.airspace_details.address,
            payer: self.payer,
            adapter_program: program_id,
            adapter_config: self.derive_adapter_config(&program_id),
            system_program: system_program::ID,
        };

        Instruction {
            program_id: glow_margin::ID,
            data: ix_data::ConfigureAdapter { is_adapter }.data(),
            accounts: accounts.to_account_metas(None),
        }
    }

    /// Set the configuration for a liquidator
    pub fn configure_liquidator(&self, liquidator: Pubkey, is_liquidator: bool) -> Instruction {
        Instruction {
            program_id: glow_margin::ID,
            data: ix_data::ConfigureLiquidator { is_liquidator }.data(),
            accounts: self.configure_permit(liquidator).to_account_metas(None),
        }
    }

    /// Set the configuration for a vault operator
    pub fn configure_vault_operator(&self, operator: Pubkey, may_operate: bool) -> Instruction {
        Instruction {
            program_id: glow_margin::ID,
            data: ix_data::ConfigureVaultOperator { may_operate }.data(),
            accounts: self.configure_permit(operator).to_account_metas(None),
        }
    }

    /// Enable or disable permission to refresh position metadata
    pub fn configure_position_config_refresher(
        &self,
        refresher: Pubkey,
        may_refresh: bool,
    ) -> Instruction {
        Instruction {
            program_id: glow_margin::ID,
            data: ix_data::ConfigurePositionConfigRefresher { may_refresh }.data(),
            accounts: self.configure_permit(refresher).to_account_metas(None),
        }
    }

    /// get the accounts to configure a permit
    fn configure_permit(&self, owner: Pubkey) -> ix_account::ConfigurePermit {
        ix_account::ConfigurePermit {
            authority: self.airspace_details.authority,
            airspace: self.airspace_details.address,
            payer: self.payer,
            owner,
            permit: self.derive_margin_permit(&owner),
            system_program: system_program::ID,
        }
    }

    /// Derive address for the config account for a given token
    pub fn derive_token_config(&self, token_mint: &Pubkey) -> Pubkey {
        derive_token_config(&self.airspace_details.address, token_mint)
    }

    /// Derive address for the config account for a given adapter
    pub fn derive_adapter_config(&self, adapter_program_id: &Pubkey) -> Pubkey {
        derive_adapter_config(&self.airspace_details.address, adapter_program_id)
    }

    /// Derive address for the config account for a given liquidator
    #[deprecated(note = "use derive_margin_permit")]
    pub fn derive_liquidator_config(&self, liquidator: &Pubkey) -> Pubkey {
        derive_margin_permit(&self.airspace_details.address, liquidator)
    }

    /// Derive address for a user's permit account in an airspace
    pub fn derive_margin_permit(&self, liquidator: &Pubkey) -> Pubkey {
        derive_margin_permit(&self.airspace_details.address, liquidator)
    }
}

/// The token account that holds position tokens when the position is custodied
/// by the margin account
pub fn derive_position_token_account(
    margin_account: &Pubkey,
    position_token_mint: &Pubkey,
) -> Pubkey {
    Pubkey::find_program_address(
        &[margin_account.as_ref(), position_token_mint.as_ref()],
        &Margin::id(),
    )
    .0
}

/// Derive the address for a user's margin account
pub fn derive_margin_account(airspace: &Pubkey, owner: &Pubkey, seed: u16) -> Pubkey {
    Pubkey::find_program_address(
        &[
            owner.as_ref(),
            airspace.as_ref(),
            seed.to_le_bytes().as_ref(),
        ],
        &glow_margin::ID,
    )
    .0
}

pub fn derive_account_constraint_ticket(margin_account: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[MARGIN_ACCOUNT_CONSTRAINT_SEED, margin_account.as_ref()],
        &glow_margin::ID,
    )
    .0
}

/// Derive the address for a user's margin account from the data in that account
pub fn derive_margin_account_from_state(state: &MarginAccount) -> Pubkey {
    derive_margin_account(
        &state.airspace,
        &state.owner,
        u16::from_le_bytes(state.user_seed),
    )
}

/// Derive address for the config account for a given token
pub fn derive_token_config(airspace: &Pubkey, token_mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[TOKEN_CONFIG_SEED, airspace.as_ref(), token_mint.as_ref()],
        &glow_margin::ID,
    )
    .0
}

/// Derive address for the config account for a given adapter
pub fn derive_adapter_config(airspace: &Pubkey, adapter_program_id: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            ADAPTER_CONFIG_SEED,
            airspace.as_ref(),
            adapter_program_id.as_ref(),
        ],
        &glow_margin::ID,
    )
    .0
}

/// Derive address for the config account for a given liquidator
#[deprecated(note = "use derive_margin_permit")]
pub fn derive_liquidator_config(airspace: &Pubkey, liquidator: &Pubkey) -> Pubkey {
    derive_margin_permit(airspace, liquidator)
}

/// Derive address for a user's permit account in an airspace
pub fn derive_margin_permit(airspace: &Pubkey, owner: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[PERMIT_SEED, airspace.as_ref(), owner.as_ref()],
        &glow_margin::ID,
    )
    .0
}

pub fn derive_liquidation(margin_account: Pubkey, liquidator: Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[b"liquidation", margin_account.as_ref(), liquidator.as_ref()],
        &glow_margin::id(),
    )
    .0
}

/// Generic invocation logic that can be applied to any margin account invoke
/// instruction, such as adapter_invoke, liquidate_invoke, and accounting_invoke
macro_rules! invoke_single {
    (
        $airspace:expr,
        $margin_account:expr,
        $adapter_ix:ident,
        $Instruction:ident $({
            $($additional_field:ident$(: $value:expr)?),* $(,)?
        })?
    ) => {{
        let adapter_config = derive_adapter_config(&$airspace, &$adapter_ix.program_id);

        let mut accounts = ix_account::$Instruction {
            margin_account: $margin_account,
            $($($additional_field$(: $value)?),*)?
        }
        .to_account_metas(None);

        accounts.push(anchor_lang::prelude::AccountMeta {
            pubkey: $adapter_ix.program_id,
            is_signer: false,
            is_writable: false,
        });
        accounts.push(anchor_lang::prelude::AccountMeta {
            pubkey: adapter_config,
            is_signer: false,
            is_writable: false,
        });

        let num_accounts = $adapter_ix.accounts.len() as u8;
        for acc in $adapter_ix.accounts {
            if acc.pubkey == $margin_account {
                accounts.push(anchor_lang::prelude::AccountMeta {
                    is_signer: false,
                    ..acc
                })
            } else {
                accounts.push(acc)
            }
        }

        Instruction {
            program_id: Margin::id(),
            data: ix_data::$Instruction {
                instructions: vec![glow_margin::IxData {
                    num_accounts,
                    data: $adapter_ix.data
                }],
            }
            .data(),
            accounts,
        }
    }};
}

macro_rules! invoke_many {
    (
        $airspace:expr,
        $margin_account:expr,
        $adapter_ixs:ident,
        $Instruction:ident $({
            $($additional_field:ident$(: $value:expr)?),* $(,)?
        })?
    ) => {{
        let mut accounts = ix_account::$Instruction {
            margin_account: $margin_account,
            $($($additional_field$(: $value)?),*)?
        }
        .to_account_metas(None);
        let mut instructions = Vec::with_capacity($adapter_ixs.len());
        for adapter_ix in &$adapter_ixs {
            let adapter_config = derive_adapter_config(&$airspace, &adapter_ix.program_id);
            accounts.push(anchor_lang::prelude::AccountMeta {
                pubkey: adapter_ix.program_id,
                is_signer: false,
                is_writable: false,
            });
            accounts.push(anchor_lang::prelude::AccountMeta {
                pubkey: adapter_config,
                is_signer: false,
                is_writable: false,
            });
            let num_accounts = adapter_ix.accounts.len() as u8;
            for acc in &adapter_ix.accounts {
                if acc.pubkey == $margin_account {
                    accounts.push(anchor_lang::prelude::AccountMeta {
                        is_signer: false,
                        ..acc.clone()
                    })
                } else {
                    accounts.push(acc.clone())
                }
            }
            instructions.push(glow_margin::IxData {
                num_accounts,
                data: adapter_ix.data.clone()
            })
        }


        Instruction {
            program_id: Margin::id(),
            data: ix_data::$Instruction {
                instructions
            }
            .data(),
            accounts,
        }
    }};
}

use invoke_many;
use invoke_single;
