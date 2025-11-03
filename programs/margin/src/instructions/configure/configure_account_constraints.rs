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

use anchor_lang::{prelude::*, AccountsClose};

use bitflags::Flags;
use glow_airspace::state::Airspace;
use glow_program_common::serialization::StorageSpace;

use crate::{seeds::*, AccountConstraintTicket, AccountConstraints, AdapterConfig, MarginAccount};

#[derive(Accounts)]
pub struct ConfigureAccountConstraints<'info> {
    /// A PDA of the adapter that has signed to prove that the adapter is calling this instruction
    #[account(
        owner = adapter_program.key()
    )]
    pub adapter_signer: Signer<'info>,

    /// The payer for any rent costs, if required
    #[account(mut)]
    pub payer: Signer<'info>,

    /// The airspace being modified
    pub airspace: Account<'info, Airspace>,

    #[account(
        mut,
        has_one = airspace
    )]
    pub margin_account: AccountLoader<'info, MarginAccount>,

    /// The adapter for which the constraint is configured
    pub adapter_program: AccountInfo<'info>,

    /// The config account to be modified
    #[account(
        seeds = [
            ADAPTER_CONFIG_SEED,
            airspace.key().as_ref(),
            adapter_program.key().as_ref()
        ],
        bump,
    )]
    pub adapter_config: Account<'info, AdapterConfig>,

    #[account(
        init_if_needed,
        seeds = [
            MARGIN_ACCOUNT_CONSTRAINT_SEED,
            margin_account.key().as_ref(),
        ],
        bump,
        payer = payer,
        space = AccountConstraintTicket::SPACE
    )]
    pub account_constraint_ticket: Account<'info, AccountConstraintTicket>,

    pub system_program: Program<'info, System>,
}

/// Configure margin account constraints.
///
/// Constraints are placed on margin accounts by an adapter if it needs to limit the
/// actions of a margin account.
/// For example, glow-vault allows a margin account to be a vault operator, and transfers
/// vault funds to the margin account. To prevent the margin account owner from withdrawing
/// funds, the vault adapter sets a restriction on the margin account preventing withdrawals.
///
/// It was not practical to require the airspace permit holder to sign the transaction, as
/// the owner of the margin account should have autonomy in requiring constraints to be added
/// or lifted from their margin account. We thus chose to enforce that any adapter sign this
/// instruction to prove that it is being called via CPI, and that the owner is not lifting
/// constraints without the adapter's awareness.
pub fn configure_account_constraints_handler(
    ctx: Context<ConfigureAccountConstraints>,
    account_constraints: AccountConstraints,
) -> Result<()> {
    // Validate the constraints
    require!(
        !account_constraints.contains_unknown_bits(),
        crate::ErrorCode::UnknownFeatureFlags
    );

    let margin_account = &mut ctx.accounts.margin_account.load_mut()?;
    let ticket = &mut ctx.accounts.account_constraint_ticket;

    // Check that the margin account has no positions
    let open_positions = margin_account
        .positions()
        .any(|p| p.address != Pubkey::default());
    require!(
        !open_positions,
        crate::ErrorCode::UnknownTokenProgram, // TODO: use the correct code
    );

    // If there are no constraints, we should remove the ticket
    if account_constraints.is_empty() {
        // The constraint ticket should already exist
        require!(
            ticket.adapter == ctx.accounts.adapter_program.key(),
            crate::ErrorCode::UnknownTokenProgram, // TODO: use the correct code
        );
        require!(
            ticket.margin_account == ctx.accounts.margin_account.key(),
            crate::ErrorCode::UnknownTokenProgram, // TODO: use the correct code
        );

        // Close the ticket
        ticket.close(ctx.accounts.payer.to_account_info())?;
    } else {
        // Ensure that the margin account has no constraints, and has no open positions (i.e. it's newly created)
        require!(
            margin_account.constraints.is_empty(),
            crate::ErrorCode::UnknownFeatureFlags
        ); // TODO error code

        ticket.set_inner(AccountConstraintTicket {
            adapter: ctx.accounts.adapter_program.key(),
            margin_account: ctx.accounts.margin_account.key(),
            constraints: account_constraints,
        });
    }

    // Update the constraint
    margin_account.constraints = account_constraints;

    Ok(())
}
