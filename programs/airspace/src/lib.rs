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

// Allow this until fixed upstream
#![allow(clippy::result_large_err)]

use anchor_lang::prelude::*;

declare_id!("AmAJeyNxxjNHfhBoCpsNMgWxhukdv3DSu3XpLfJspace");

mod instructions;
use instructions::*;

pub use instructions::{
    AirspaceAuthorityFinalize, AirspaceCreate, AirspacePermitCreate, AirspacePermitIssuerCreate,
    AirspacePermitIssuerRevoke, AirspacePermitRevoke, CreateGovernorId, GovernorFinalizePropose,
    GovernorPropose,
};

pub mod events;
pub mod state;

pub mod seeds {
    use super::constant;

    #[constant]
    pub const GOVERNOR_ID: &[u8] = b"governor-id";

    #[constant]
    pub const AIRSPACE: &[u8] = b"airspace";

    #[constant]
    pub const AIRSPACE_PERMIT_ISSUER: &[u8] = b"airspace-permit-issuer";

    #[constant]
    pub const AIRSPACE_PERMIT: &[u8] = b"airspace-permit";
}

#[program]
pub mod airspace {
    use super::*;

    /// Create the governor identity account
    ///
    /// If this is a testing environment, the signer on the first transaction executing this
    /// instruction becomes the first governor. For mainnet environment the first governor
    /// is set from a hardcoded default.
    pub fn create_governor_id(ctx: Context<CreateGovernorId>) -> Result<()> {
        instructions::create_governor_id_handler(ctx)
    }

    /// Propose a new protocol governor address. Must be signed by the current governor address.
    ///
    /// # Parameters
    ///
    /// * `proposed_governor` - The new address with governor authority
    pub fn governor_propose(
        ctx: Context<GovernorPropose>,
        proposed_governor: Pubkey,
    ) -> Result<()> {
        instructions::governor_propose(ctx, proposed_governor)
    }

    /// Finalize the protocol governor change, where the new authority has to be the signer
    pub fn governor_finalize_propose(ctx: Context<GovernorFinalizePropose>) -> Result<()> {
        instructions::governor_finalize_propose(ctx)
    }

    /// Create a new airspace, which serves as an isolation boundary for resources in the protocol
    ///
    /// # Parameters
    ///
    /// * `seed` - An arbitrary string of bytes used to generate the airspace address.
    /// * `is_restricted` - If true, then user access to create an account within the airspace is
    ///                     restricted, and must be approved by some regulating authority.
    /// * `authority` - The utimate authority with permission to modify things about an airspace.
    pub fn airspace_create(
        ctx: Context<AirspaceCreate>,
        seed: String,
        is_restricted: bool,
        authority: Pubkey,
    ) -> Result<()> {
        instructions::airspace_create_handler(ctx, seed, is_restricted, authority)
    }

    /// Propose a new authority for an airspace
    ///
    /// # Parameters
    ///
    /// * `proposed_authority` - The address that the authority is being changed to.
    pub fn airspace_propose_authority(
        ctx: Context<AirspaceAuthorityPropose>,
        proposed_authority: Pubkey,
    ) -> Result<()> {
        instructions::airspace_transfer_authority_handler(ctx, proposed_authority)
    }

    /// Cancel a pending authority change for an airspace
    pub fn airspace_cancel_proposed_authority(
        ctx: Context<AirspaceAuthorityCancelProposal>,
    ) -> Result<()> {
        instructions::airspace_authority_cancel_proposal(ctx)
    }

    /// Finalize the authority change for an airspace, where the new authority has to be the signer
    pub fn airspace_finalize_authority(ctx: Context<AirspaceAuthorityFinalize>) -> Result<()> {
        instructions::airspace_authority_finalize(ctx)
    }

    /// Create a new license for an address to serve as an airspace regulator.
    ///
    /// Addresses with regulator licenses in an airspace are allowed to issue new permits
    /// for other addresses to utilize the airspace.
    ///
    /// # Parameters
    ///
    /// * `issuer` - The address that is being granted the permission to issue permits.
    pub fn airspace_permit_issuer_create(
        ctx: Context<AirspacePermitIssuerCreate>,
        issuer: Pubkey,
    ) -> Result<()> {
        instructions::airspace_permit_issuer_create_handler(ctx, issuer)
    }

    /// Revoke a previously authorized permit issuer, preventing the permit issuer from issuing any
    /// new permits for the airspace.
    pub fn airspace_permit_issuer_revoke(ctx: Context<AirspacePermitIssuerRevoke>) -> Result<()> {
        instructions::airspace_permit_issuer_revoke_handler(ctx)
    }

    /// Create a new permit, allowing an address access to resources in an airspace
    ///
    /// # Parameters
    ///
    /// * `owner` - The owner for the new permit, which is the address being allowed to use
    ///             the airspace.
    pub fn airspace_permit_create(ctx: Context<AirspacePermitCreate>, owner: Pubkey) -> Result<()> {
        instructions::airspace_permit_create_handler(ctx, owner)
    }

    /// Revoke a previously created permit
    pub fn airspace_permit_revoke(ctx: Context<AirspacePermitRevoke>) -> Result<()> {
        instructions::airspace_permit_revoke_handler(ctx)
    }
}

#[error_code]
pub enum AirspaceErrorCode {
    /// 707000 - No permissions to do an action
    #[msg("The signer does not have the required permissions to do this")]
    PermissionDenied = 701_000,
}
