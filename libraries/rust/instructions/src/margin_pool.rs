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

use anchor_lang::prelude::{Id, System, ToAccountMetas};
use anchor_lang::InstructionData;
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use anchor_spl::token_2022::ID as TOKEN_2022_ID;
use glow_program_common::oracle::TokenPriceOracle;
use glow_program_common::token_change::TokenChange;
use solana_sdk::instruction::Instruction;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::sysvar::{rent::Rent, SysvarId};

use glow_margin_pool::accounts as ix_accounts;
use glow_margin_pool::{instruction as ix_data, MarginPoolConfig, TokenMetadataParams};

pub use glow_margin_pool::ID as MARGIN_POOL_PROGRAM;

use crate::airspace::AirspaceDetails;
use crate::margin::MarginConfigIxBuilder;
use crate::{get_metadata_address, MintInfo};

/// Utility for creating instructions to interact with the margin
/// pools program for a specific pool.
#[derive(Clone, Debug)]
pub struct MarginPoolIxBuilder {
    /// The address of the mint for tokens stored in the pool
    pub token_mint: MintInfo,

    /// The address of the margin pool
    pub address: Pubkey,

    /// The address of the airspace
    pub airspace: Pubkey,

    /// The address of the account holding the tokens in the pool
    pub vault: Pubkey,

    /// The address of the mint for deposit notes, which represent user
    /// deposit in the pool
    pub deposit_note_mint: Pubkey,

    /// The address of the mint for loan notes, which represent user borrows
    /// from the pool
    pub loan_note_mint: Pubkey,
}

impl MarginPoolIxBuilder {
    /// Create a new builder for an SPL token mint by deriving pool addresses
    ///
    /// # Params
    ///
    /// `airspace` - The airspace that the pool is being registered under
    /// `token_mint` - The token mint which whose tokens the pool stores
    pub fn new(airspace: Pubkey, token_mint: MintInfo) -> Self {
        let address = derive_margin_pool(&airspace, &token_mint.address);
        let (vault, _) = Pubkey::find_program_address(
            &[address.as_ref(), b"vault".as_ref()],
            &glow_margin_pool::ID,
        );
        let (deposit_note_mint, _) = Pubkey::find_program_address(
            &[address.as_ref(), b"deposit-notes".as_ref()],
            &glow_margin_pool::ID,
        );
        let (loan_note_mint, _) = Pubkey::find_program_address(
            &[address.as_ref(), b"loan-notes".as_ref()],
            &glow_margin_pool::ID,
        );

        Self {
            airspace,
            token_mint,
            address,
            vault,
            deposit_note_mint,
            loan_note_mint,
        }
    }

    /// Get the token program of the pool deposits
    pub fn pool_deposit_mint_info(&self) -> MintInfo {
        MintInfo::with_token_2022(self.deposit_note_mint)
    }

    /// Get the token program of the pool loans
    pub fn pool_loan_mint_info(&self) -> MintInfo {
        MintInfo::with_token_2022(self.loan_note_mint)
    }

    /// Instruction to create the pool with given parameters
    ///
    /// # Params
    ///
    /// `payer` - The address paying for the rent
    /// `payer` - The address that will own the fees, defaults to the authority
    pub fn create(
        &self,
        authority: Pubkey,
        payer: Pubkey,
        fee_owner: Option<Pubkey>,
    ) -> Instruction {
        let fee_owner = fee_owner.unwrap_or(authority);
        let accounts = ix_accounts::CreatePool {
            authority,
            airspace: self.airspace,
            fee_owner,
            payer,
            margin_pool: self.address,
            vault: self.vault,
            deposit_note_mint: self.deposit_note_mint,
            loan_note_mint: self.loan_note_mint,
            token_mint: self.token_mint.address,
            fee_destination: derive_margin_pool_fee_destination(&fee_owner, &self.address),
            token_metadata: get_metadata_address(&self.airspace, &self.token_mint.address),
            deposit_note_metadata: get_metadata_address(&self.airspace, &self.deposit_note_mint),
            loan_note_metadata: get_metadata_address(&self.airspace, &self.loan_note_mint),
            mint_token_program: self.token_mint.token_program(),
            pool_token_program: self.pool_loan_mint_info().token_program(),
            metadata_program: glow_metadata::ID,
            system_program: System::id(),
        }
        .to_account_metas(None);

        Instruction {
            program_id: glow_margin_pool::ID,
            data: ix_data::CreatePool {}.data(),
            accounts,
        }
    }

    /// Instruction to configure the pool with given parameters
    pub fn configure(
        &self,
        authority: Pubkey,
        payer: Pubkey,
        config: &MarginPoolConfiguration,
    ) -> Instruction {
        let accounts = ix_accounts::Configure {
            authority,
            airspace: self.airspace,
            payer,
            margin_pool: self.address,
            token_mint: self.token_mint.address,
            token_metadata: get_metadata_address(&self.airspace, &self.token_mint.address),
            metadata_program: glow_metadata::ID,
            system_program: System::id(),
            deposit_metadata: get_metadata_address(&self.airspace, &self.deposit_note_mint),
            loan_metadata: get_metadata_address(&self.airspace, &self.loan_note_mint),
            deposit_note_mint: self.deposit_note_mint,
            loan_note_mint: self.loan_note_mint,
        }
        .to_account_metas(None);

        Instruction {
            program_id: glow_margin_pool::ID,
            data: ix_data::Configure {
                metadata: config.metadata.clone(),
                config: config.parameters,
                oracle: config.token_oracle,
            }
            .data(),
            accounts,
        }
    }

    /// Instruction to deposit tokens into the pool in exchange for deposit notes
    ///
    /// # Params
    ///
    /// `depositor` - The authority for the source tokens
    /// `margin_account` - The margin account to use for the deposit, if any
    /// `source` - The token account that has the tokens to be deposited
    /// `destination` - The token account to send notes representing the deposit
    /// `change` - The type of token change being made. See [TokenChange].
    pub fn deposit(
        &self,
        depositor: Pubkey,
        margin_account: Option<Pubkey>,
        source: Pubkey,
        destination: Pubkey,
        change: TokenChange,
    ) -> Instruction {
        let accounts = ix_accounts::Deposit {
            margin_pool: self.address,
            optional_margin_account: margin_account,
            vault: self.vault,
            deposit_note_mint: self.deposit_note_mint,
            depositor,
            source,
            source_mint: self.token_mint.address,
            destination,
            deposit_metadata: get_metadata_address(&self.airspace, &self.deposit_note_mint),
            mint_token_program: self.token_mint.token_program(),
            pool_token_program: self.pool_loan_mint_info().token_program(),
        }
        .to_account_metas(None);

        let TokenChange { kind, tokens } = change;
        Instruction {
            program_id: glow_margin_pool::ID,
            data: ix_data::Deposit {
                change_kind: kind,
                amount: tokens,
            }
            .data(),
            accounts,
        }
    }

    /// Instruction to withdraw tokens from the pool in exchange for deposit notes
    ///
    /// # Params
    ///
    /// `depositor` - The authority for the deposit notes
    /// `source` - The token account that has the deposit notes to be exchanged
    /// `destination` - The token account to send the withdrawn deposit
    /// `change` - The amount of the deposit
    pub fn withdraw(
        &self,
        depositor: Pubkey,
        source: Pubkey,
        destination: Pubkey,
        change: TokenChange,
    ) -> Instruction {
        let destination_ata = get_associated_token_address_with_program_id(
            &depositor,
            &self.token_mint.address,
            &self.token_mint.token_program(),
        );
        let accounts = ix_accounts::Withdraw {
            margin_pool: self.address,
            vault: self.vault,
            deposit_note_mint: self.deposit_note_mint,
            depositor,
            source,
            destination,
            destination_ata,
            token_mint: self.token_mint.address,
            mint_token_program: self.token_mint.token_program(),
            pool_token_program: self.pool_deposit_mint_info().token_program(),
        }
        .to_account_metas(None);

        let TokenChange { kind, tokens } = change;
        Instruction {
            program_id: glow_margin_pool::ID,
            data: ix_data::Withdraw {
                change_kind: kind,
                amount: tokens,
            }
            .data(),
            accounts,
        }
    }

    /// Instruction to borrow tokens using a margin account
    ///
    /// # Params
    ///
    /// `margin_account` - The account being borrowed against
    /// `deposit_account` - The account to receive the notes for the borrowed tokens
    /// `amount` - The amount of tokens to be borrowed
    pub fn margin_borrow(
        &self,
        margin_account: Pubkey,
        deposit_account: Pubkey,
        change: TokenChange,
    ) -> Instruction {
        let accounts = ix_accounts::MarginBorrow {
            margin_account,
            margin_pool: self.address,
            loan_note_mint: self.loan_note_mint,
            deposit_note_mint: self.deposit_note_mint,
            loan_account: derive_loan_account(&margin_account, &self.loan_note_mint),
            deposit_account,
            pool_token_program: self.pool_loan_mint_info().token_program(),
        }
        .to_account_metas(None);

        let TokenChange { kind, tokens } = change;
        Instruction {
            program_id: glow_margin_pool::ID,
            data: ix_data::MarginBorrow {
                change_kind: kind,
                amount: tokens,
            }
            .data(),
            accounts,
        }
    }

    /// Instruction to borrow tokens using a margin account
    ///
    /// # Params
    ///
    /// `margin_account` - The account being borrowed against
    /// `destination` - The account to receive the borrowed tokens
    /// `destination_mint` - The mint of the account to receive the borrowed tokens
    /// `amount` - The amount of tokens to be borrowed
    pub fn margin_borrow_v2(
        &self,
        margin_account: Pubkey,
        destination: Pubkey,
        amount: u64,
    ) -> Instruction {
        let accounts = ix_accounts::MarginBorrowV2 {
            margin_account,
            margin_pool: self.address,
            loan_note_mint: self.loan_note_mint,
            vault: self.vault,
            token_mint: self.token_mint.address,
            loan_account: derive_loan_account(&margin_account, &self.loan_note_mint),
            destination,
            mint_token_program: self.token_mint.token_program(),
            pool_token_program: TOKEN_2022_ID,
        }
        .to_account_metas(None);

        Instruction {
            program_id: glow_margin_pool::ID,
            data: ix_data::MarginBorrowV2 { amount }.data(),
            accounts,
        }
    }

    /// Instruction to repay tokens owed by a margin account
    ///
    /// # Params
    ///
    /// `margin_account` - The account with the loan to be repaid
    /// `deposit_account` - The account with notes to repay the loan
    /// `amount` - The amount to be repaid
    pub fn margin_repay(
        &self,
        margin_account: Pubkey,
        deposit_account: Pubkey,
        change: TokenChange,
    ) -> Instruction {
        let accounts = ix_accounts::MarginRepay {
            margin_account,
            margin_pool: self.address,
            loan_note_mint: self.loan_note_mint,
            deposit_note_mint: self.deposit_note_mint,
            loan_account: derive_loan_account(&margin_account, &self.loan_note_mint),
            deposit_account,
            pool_token_program: self.pool_deposit_mint_info().token_program(),
        }
        .to_account_metas(None);

        let TokenChange { kind, tokens } = change;
        Instruction {
            program_id: glow_margin_pool::ID,
            data: ix_data::MarginRepay {
                change_kind: kind,
                amount: tokens,
            }
            .data(),
            accounts,
        }
    }

    /// Instruction to repay tokens owed by a margin account using a token account
    ///
    /// # Params
    ///
    /// `margin_account` - The account with the loan to be repaid
    /// `repayment_source_authority` - The authority for the repayment source tokens
    /// `repayment_source_account` - The token account to use for repayment
    /// `loan_account` - The account with the loan debt to be reduced
    /// `amount` - The amount to be repaid
    pub fn repay(
        &self,
        repayment_source_authority: Pubkey,
        repayment_source_account: Pubkey,
        loan_account: Pubkey,
        change: TokenChange,
    ) -> Instruction {
        let accounts = ix_accounts::Repay {
            margin_pool: self.address,
            loan_note_mint: self.loan_note_mint,
            vault: self.vault,
            token_mint: self.token_mint.address,
            loan_account,
            repayment_token_account: repayment_source_account,
            repayment_account_authority: repayment_source_authority,
            mint_token_program: self.token_mint.token_program(),
            pool_token_program: self.pool_loan_mint_info().token_program(),
        }
        .to_account_metas(None);

        let TokenChange { kind, tokens } = change;
        Instruction {
            program_id: glow_margin_pool::ID,
            data: ix_data::Repay {
                change_kind: kind,
                amount: tokens,
            }
            .data(),
            accounts,
        }
    }

    /// Instruction to refresh the position on a margin account
    ///
    /// # Params
    ///
    /// `margin_account` - The margin account with the deposit to be withdrawn
    /// `oracle` - The oracle account for this pool
    pub fn margin_refresh_position(
        &self,
        margin_account: Pubkey,
        oracle: Pubkey,
        redemption_oracle: Option<Pubkey>,
    ) -> Instruction {
        let accounts = ix_accounts::MarginRefreshPosition {
            margin_account,
            margin_pool: self.address,
            price_oracle: oracle,
            redemption_quote_oracle: redemption_oracle,
        }
        .to_account_metas(None);

        Instruction {
            program_id: glow_margin_pool::ID,
            data: ix_data::MarginRefreshPosition {}.data(),
            accounts,
        }
    }

    /// Instruction to register a loan position with a margin pool.
    pub fn register_loan(&self, margin_account: Pubkey, payer: Pubkey) -> Instruction {
        let loan_note_account = derive_loan_account(&margin_account, &self.loan_note_mint);
        let loan_token_config =
            MarginConfigIxBuilder::new(AirspaceDetails::from_address(self.airspace), payer)
                .derive_token_config(&self.loan_note_mint);

        let accounts = ix_accounts::RegisterLoan {
            margin_account,
            loan_token_config,
            margin_pool: self.address,
            loan_note_account,
            loan_note_mint: self.loan_note_mint,
            payer,
            token_program: self.pool_loan_mint_info().token_program(),
            system_program: System::id(),
            rent: Rent::id(),
        };

        Instruction {
            program_id: glow_margin_pool::ID,
            data: ix_data::RegisterLoan {}.data(),
            accounts: accounts.to_account_metas(None),
        }
    }

    /// Instruction to close a loan account in a margin pool
    pub fn close_loan(&self, margin_account: Pubkey, payer: Pubkey) -> Instruction {
        let loan_note_account = derive_loan_account(&margin_account, &self.loan_note_mint);

        let accounts = ix_accounts::CloseLoan {
            margin_account,
            margin_pool: self.address,
            loan_note_account,
            loan_note_mint: self.loan_note_mint,
            beneficiary: payer,
            token_program: self.pool_loan_mint_info().token_program(),
        };

        Instruction {
            program_id: glow_margin_pool::ID,
            data: ix_data::CloseLoan {}.data(),
            accounts: accounts.to_account_metas(None),
        }
    }

    /// Instruction to collect interest and fees
    pub fn collect(&self, fee_destination: Pubkey) -> Instruction {
        let accounts = ix_accounts::Collect {
            margin_pool: self.address,
            vault: self.vault,
            fee_destination,
            deposit_note_mint: self.deposit_note_mint,
            token_program: self.pool_deposit_mint_info().token_program(),
        }
        .to_account_metas(None);

        Instruction {
            program_id: glow_margin_pool::ID,
            data: ix_data::Collect.data(),
            accounts,
        }
    }

    /// Instruction to withdraw collected fees
    pub fn withdraw_fees(
        &self,
        fee_owner: Pubkey,
        fee_withdrawal_destination: Pubkey,
    ) -> Instruction {
        let accounts = ix_accounts::WithdrawFees {
            margin_pool: self.address,
            vault: self.vault,
            fee_owner,
            fee_destination: derive_margin_pool_fee_destination(&fee_owner, &self.address),
            fee_withdrawal_destination,
            token_mint: self.token_mint.address,
            deposit_note_mint: self.deposit_note_mint,
            mint_token_program: self.token_mint.token_program(),
            pool_token_program: self.pool_deposit_mint_info().token_program(),
        }
        .to_account_metas(None);

        Instruction {
            program_id: glow_margin_pool::ID,
            data: ix_data::WithdrawFees {}.data(),
            accounts,
        }
    }

    /// Instruction to transfer a loan between margin accounts
    pub fn admin_transfer_loan(
        &self,
        source_margin_account: &Pubkey,
        target_margin_account: &Pubkey,
        amount: u64,
    ) -> Instruction {
        let accounts = ix_accounts::AdminTransferLoan {
            authority: glow_program_common::PROTOCOL_GOVERNOR_ID,
            margin_pool: self.address,
            source_loan_account: derive_loan_account(source_margin_account, &self.loan_note_mint),
            target_loan_account: derive_loan_account(target_margin_account, &self.loan_note_mint),
            loan_note_mint: self.loan_note_mint,
            pool_token_program: self.pool_loan_mint_info().token_program(),
        }
        .to_account_metas(None);

        Instruction {
            program_id: glow_margin_pool::ID,
            data: ix_data::AdminTransferLoan { amount }.data(),
            accounts,
        }
    }
}

/// Parameters used to configure a margin pool
#[derive(Clone, Default)]
pub struct MarginPoolConfiguration {
    /// Optional configuration of the pool
    pub parameters: Option<MarginPoolConfig>,
    /// Optional metadata of the pool, includes collateral weight and risk multiplier
    pub metadata: Option<TokenMetadataParams>,
    /// Pool/token oracle
    pub token_oracle: Option<TokenPriceOracle>,
}

/// Find a loan token account for a margin account and margin pool's loan note mint
pub fn derive_loan_account(margin_account: &Pubkey, loan_note_mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[margin_account.as_ref(), loan_note_mint.as_ref()],
        &glow_margin_pool::id(),
    )
    .0
}

/// Derive the address for a margin pool
pub fn derive_margin_pool(airspace: &Pubkey, token_mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[airspace.as_ref(), token_mint.as_ref()],
        &glow_margin_pool::ID,
    )
    .0
}

pub fn derive_margin_pool_fee_destination(fee_owner: &Pubkey, pool: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            pool.as_ref(),
            fee_owner.as_ref(),
            glow_margin_pool::seeds::FEE_DESTINATION,
        ],
        &glow_margin_pool::ID,
    )
    .0
}
