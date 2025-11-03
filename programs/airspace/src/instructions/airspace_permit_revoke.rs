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

use anchor_lang::prelude::*;

use crate::{
    events::AirspacePermitRevoked,
    seeds::AIRSPACE_PERMIT_ISSUER,
    state::{Airspace, AirspacePermit},
    AirspaceErrorCode,
};

#[derive(Accounts)]
pub struct AirspacePermitRevoke<'info> {
    #[account(mut)]
    receiver: Signer<'info>,

    /// The authority allowed to revoke an airspace permit
    ///
    /// The addresses allowed to revoke are:
    ///     * the airspace authority, always
    ///     * the regulator that issued the permit, always
    ///     * any address, if the airspace is restricted and the regulator license
    ///       has been revoked
    /// The only addresses that can revoke a permit are either the regulator that
    /// created the permit, or the airspace authority.
    authority: Signer<'info>,

    /// The airspace the permit is to be revoked from
    airspace: Account<'info, Airspace>,

    /// The identity account for the regulator that issued the permit
    #[account(seeds = [
                AIRSPACE_PERMIT_ISSUER,
                airspace.key().as_ref(),
                permit.issuer.as_ref()
              ],
              bump
    )]
    issuer_id: AccountInfo<'info>,

    /// The airspace account to be created
    #[account(mut,
              close = receiver,
              has_one = airspace
    )]
    permit: Account<'info, AirspacePermit>,
}

pub fn airspace_permit_revoke_handler(ctx: Context<AirspacePermitRevoke>) -> Result<()> {
    let airspace = &mut ctx.accounts.airspace;
    let permit = &ctx.accounts.permit;
    let authority = ctx.accounts.authority.key();

    if !airspace_revocation_allowed(
        airspace.is_restricted,
        authority,
        airspace.authority,
        permit.issuer,
        ctx.accounts.issuer_id.data_is_empty(),
    ) {
        return err!(AirspaceErrorCode::PermissionDenied);
    }

    emit!(AirspacePermitRevoked {
        airspace: airspace.key(),
        permit: permit.key(),
    });

    Ok(())
}

pub fn airspace_revocation_allowed(
    restricted: bool,
    authority: Pubkey,
    airspace_authority: Pubkey,
    issuer: Pubkey,
    issuer_data_empty: bool,
) -> bool {
    let is_authority = authority == airspace_authority;
    let is_issuer = authority == issuer;
    let is_open_revocation = restricted && issuer_data_empty;

    // Based on the `authority` definition in `AirspacePermitRevoke`,
    // revocation is allowed when either of the three conditions are met:
    is_authority || is_issuer || is_open_revocation
}

#[cfg(test)]
mod tests {
    use crate::airspace_revocation_allowed;
    use anchor_lang::prelude::Pubkey;

    #[test]
    fn test_airspace_permit_revoke_allowed_when_authority_is_airspace_authority() {
        let authority = Pubkey::new_unique();
        let issuer = Pubkey::new_unique();
        let issuer_empty = true;

        // If the authority is the airspace authority, revocation is always allowed
        // independent of the issuer and issuer_empty status.
        assert!(airspace_revocation_allowed(
            true,
            authority,
            authority,
            issuer,
            issuer_empty
        ));
        assert!(airspace_revocation_allowed(
            false,
            authority,
            authority,
            issuer,
            !issuer_empty
        ));
        assert!(airspace_revocation_allowed(
            false,
            authority,
            authority,
            Pubkey::new_unique(),
            !issuer_empty
        ));
        assert!(airspace_revocation_allowed(
            false,
            authority,
            authority,
            Pubkey::new_unique(),
            issuer_empty
        ));
    }

    #[test]
    fn test_airspace_permit_revoke_allowed_when_authority_is_issuer() {
        let permit_issuer = Pubkey::new_unique();
        let issuer_empty = true;

        // If the authority is the permit issuer, revocation is always allowed
        // independent of the airspace authority and issuer_empty status.
        assert!(airspace_revocation_allowed(
            true,
            permit_issuer,
            Pubkey::new_unique(),
            permit_issuer,
            issuer_empty
        ));
        assert!(airspace_revocation_allowed(
            false,
            permit_issuer,
            Pubkey::new_unique(),
            permit_issuer,
            issuer_empty
        ));
        assert!(airspace_revocation_allowed(
            true,
            permit_issuer,
            Pubkey::new_unique(),
            permit_issuer,
            !issuer_empty
        ));
        assert!(airspace_revocation_allowed(
            false,
            permit_issuer,
            Pubkey::new_unique(),
            permit_issuer,
            !issuer_empty
        ));
    }

    #[test]
    fn test_airspace_permit_revoke_allowed_when_open_revocation() {
        let restricted = true;
        let issuer_empty = true;

        // Open revocation is allowed when the airspace is restricted and the issuer is empty
        assert!(airspace_revocation_allowed(
            restricted,
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            issuer_empty
        ));
    }

    #[test]
    fn test_airspace_permit_revoke_disallowed() {
        // When authority is not the:
        // - airspace authority
        // - permit issuer
        //
        // In addition, open revocation has to be disabled.
        // We test all permutations by inverting the boolean values below.
        let issuer_empty = true;
        let restricted = false;

        assert!(!airspace_revocation_allowed(
            restricted,
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            issuer_empty
        ));
        assert!(!airspace_revocation_allowed(
            restricted,
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            !issuer_empty
        ));
        assert!(!airspace_revocation_allowed(
            !restricted,
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            !issuer_empty
        ));
    }
}
