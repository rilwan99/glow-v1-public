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

use anchor_lang::{InstructionData, ToAccountMetas};
use solana_sdk::{instruction::Instruction, pubkey::Pubkey, system_program};

use glow_airspace::seeds::{AIRSPACE, AIRSPACE_PERMIT, AIRSPACE_PERMIT_ISSUER, GOVERNOR_ID};

pub use glow_airspace::ID as AIRSPACE_PROGRAM;

/// A builder for [`airspace::instruction`] instructions.
#[derive(Debug, Clone)]
pub struct AirspaceIxBuilder {
    /// The user address that will pay for the transactions
    payer: Pubkey,

    /// The airspace and its manager
    airspace_manager: AirspaceDetails,
}

impl AirspaceIxBuilder {
    /// Create a new instruction builder referencing an airspace by using a seed
    pub fn new(seed: &str, payer: Pubkey, authority: Pubkey) -> Self {
        let address = derive_airspace(seed);

        Self {
            payer,
            airspace_manager: AirspaceDetails {
                address,
                name: seed.to_owned(),
                authority,
            },
        }
    }

    /// Create a new instruction builder referencing an airspace by its address
    pub fn new_from_address(address: Pubkey, payer: Pubkey, authority: Pubkey) -> Self {
        Self {
            payer,
            airspace_manager: AirspaceDetails {
                address,
                name: "".to_owned(),
                authority,
            },
        }
    }

    /// making the field public would allow invalid states because it can
    /// diverge from the seed.
    pub fn address(&self) -> Pubkey {
        self.airspace_manager.address
    }

    /// getter for the seed to derive the address
    pub fn seed(&self) -> String {
        self.airspace_manager.name.clone()
    }

    pub fn airspace_manager(&self) -> &AirspaceDetails {
        // If the user is getting this struct, they must be intending on signing as an authority.
        // Ensure that the authority is present
        assert_ne!(self.airspace_manager.authority, Pubkey::default());
        &self.airspace_manager
    }

    /// Create the governor identity account
    pub fn create_governor_id(&self) -> Instruction {
        let accounts = glow_airspace::accounts::CreateGovernorId {
            payer: self.payer,
            governor_id: derive_governor_id(),
            system_program: system_program::ID,
        }
        .to_account_metas(None);

        Instruction {
            accounts,
            program_id: glow_airspace::ID,
            data: glow_airspace::instruction::CreateGovernorId {}.data(),
        }
    }

    /// Set the protocol governor address
    ///
    /// # Params
    ///
    /// `proposed_governor` - The new governor address
    pub fn propose_governor(&self, proposed_governor: Pubkey) -> Instruction {
        let accounts = glow_airspace::accounts::GovernorPropose {
            governor: self.airspace_manager.authority,
            governor_id: derive_governor_id(),
            transfer: derive_governor_transfer(),
            system_program: system_program::ID,
        }
        .to_account_metas(None);

        Instruction {
            accounts,
            program_id: glow_airspace::ID,
            data: glow_airspace::instruction::GovernorPropose { proposed_governor }.data(),
        }
    }

    /// Finalize a governor change for the airspace
    pub fn finalize_propose_governor(&self, old_governor: Pubkey) -> Instruction {
        let accounts = glow_airspace::accounts::GovernorFinalizePropose {
            proposed_governor: self.payer,
            governor_id: derive_governor_id(),
            old_governor,
            transfer: derive_governor_transfer(),
        }
        .to_account_metas(None);

        Instruction {
            accounts,
            program_id: glow_airspace::ID,
            data: glow_airspace::instruction::GovernorFinalizePropose.data(),
        }
    }

    /// Create the airspace
    ///
    /// # Params
    ///
    /// `authority` - The address to set as the authority in the airspace
    /// `is_restricted` - If true, the airspace requires specific issuers to enable user access
    pub fn create(&self, authority: Pubkey, is_restricted: bool) -> Instruction {
        let accounts = glow_airspace::accounts::AirspaceCreate {
            payer: self.payer,
            airspace: self.airspace_manager.address,
            governor: self.airspace_manager.authority,
            governor_id: derive_governor_id(),
            system_program: system_program::ID,
        }
        .to_account_metas(None);

        Instruction {
            accounts,
            program_id: glow_airspace::ID,
            data: glow_airspace::instruction::AirspaceCreate {
                seed: self.airspace_manager.name.clone(),
                is_restricted,
                authority,
            }
            .data(),
        }
    }

    /// Propose an authority change for the airspace
    ///
    /// # Params
    ///
    /// `proposed_authority` - The new address
    pub fn propose_authority(&self, proposed_authority: Pubkey) -> Instruction {
        let accounts = glow_airspace::accounts::AirspaceAuthorityPropose {
            authority: self.airspace_manager.authority,
            airspace: self.airspace_manager.address,
            transfer: derive_airspace_transfer(&self.address()),
            system_program: system_program::ID,
        }
        .to_account_metas(None);

        Instruction {
            accounts,
            program_id: glow_airspace::ID,
            data: glow_airspace::instruction::AirspaceProposeAuthority { proposed_authority }
                .data(),
        }
    }

    /// Finalize an authority change for the airspace
    pub fn finalize_propose_authority(&self, old_authority: Pubkey) -> Instruction {
        let accounts = glow_airspace::accounts::AirspaceAuthorityFinalize {
            proposed_authority: self.payer, // Assumes the payer is the proposed authority
            airspace: self.airspace_manager.address,
            old_authority,
            transfer: derive_airspace_transfer(&self.address()),
        }
        .to_account_metas(None);

        Instruction {
            accounts,
            program_id: glow_airspace::ID,
            data: glow_airspace::instruction::AirspaceFinalizeAuthority.data(),
        }
    }

    /// Register an address as being allowed to issue new permits for users
    ///
    /// # Params
    ///
    /// `issuer` - The address authorized to issue permits
    pub fn permit_issuer_create(&self, issuer: Pubkey) -> Instruction {
        let accounts = glow_airspace::accounts::AirspacePermitIssuerCreate {
            airspace: self.airspace_manager.address,
            authority: self.airspace_manager.authority,
            payer: self.payer,
            issuer_id: self.derive_issuer_id(&issuer),
            system_program: system_program::ID,
        }
        .to_account_metas(None);

        Instruction {
            accounts,
            program_id: glow_airspace::ID,
            data: glow_airspace::instruction::AirspacePermitIssuerCreate { issuer }.data(),
        }
    }

    /// Revoke an issuer from issuing new permits
    ///
    /// # Params
    ///
    /// `issuer` - The address no longer authorized to issue permits
    pub fn permit_issuer_revoke(&self, issuer: Pubkey) -> Instruction {
        let accounts = glow_airspace::accounts::AirspacePermitIssuerRevoke {
            airspace: self.airspace_manager.address,
            authority: self.airspace_manager.authority,
            receiver: self.payer,
            issuer_id: self.derive_issuer_id(&issuer),
        }
        .to_account_metas(None);

        Instruction {
            accounts,
            program_id: glow_airspace::ID,
            data: glow_airspace::instruction::AirspacePermitIssuerRevoke {}.data(),
        }
    }

    /// Issue a permit for an address, allowing it to use the airspace
    ///
    /// # Params
    ///
    /// `user` - The address authorized to use the airspace
    pub fn permit_create(&self, user: Pubkey) -> Instruction {
        let accounts = glow_airspace::accounts::AirspacePermitCreate {
            airspace: self.airspace_manager.address,
            authority: self.airspace_manager.authority,
            payer: self.payer,
            permit: self.derive_permit(&user),
            issuer_id: self.derive_issuer_id(&self.airspace_manager.authority),
            system_program: system_program::ID,
        }
        .to_account_metas(None);

        Instruction {
            accounts,
            program_id: glow_airspace::ID,
            data: glow_airspace::instruction::AirspacePermitCreate { owner: user }.data(),
        }
    }

    /// Revoke a previously issued permit for an address
    ///
    /// # Params
    ///
    /// `user` - The address previously authorized to use the airspace
    /// `issuer` - The address that originally issued the permit
    pub fn permit_revoke(&self, user: Pubkey, issuer: Pubkey) -> Instruction {
        let accounts = glow_airspace::accounts::AirspacePermitRevoke {
            airspace: self.airspace_manager.address,
            authority: self.airspace_manager.authority,
            receiver: self.payer,
            permit: self.derive_permit(&user),
            issuer_id: self.derive_issuer_id(&issuer),
        }
        .to_account_metas(None);

        Instruction {
            accounts,
            program_id: glow_airspace::ID,
            data: glow_airspace::instruction::AirspacePermitRevoke {}.data(),
        }
    }

    /// Derive the address for the account identifying permit issuers
    pub fn derive_issuer_id(&self, issuer: &Pubkey) -> Pubkey {
        derive_issuer_id(&self.airspace_manager.address, issuer)
    }

    /// Derive the address for a user's permit to use the airspace
    pub fn derive_permit(&self, user: &Pubkey) -> Pubkey {
        derive_permit(&self.airspace_manager.address, user)
    }
}

/// Data structure of an airspace manager, which is the airspace and its authority
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AirspaceDetails {
    /// The address of the airspace
    pub address: Pubkey,
    /// Seed used to generate the airspace address
    pub name: String,
    /// Account authorized by the airspaces program to register and administrate
    /// margin adapters.
    pub authority: Pubkey,
}

impl AirspaceDetails {
    /// Create from an airspace address
    ///
    /// Convenient when the user only needs the airspace address, and the authority
    /// is never used.
    pub fn from_address(address: Pubkey) -> Self {
        Self {
            address,
            name: "".to_owned(),
            authority: Pubkey::default(),
        }
    }
}

/// Derive the governor id account address
pub fn derive_governor_id() -> Pubkey {
    Pubkey::find_program_address(&[GOVERNOR_ID], &glow_airspace::ID).0
}

/// Derive the governor transfer address
pub fn derive_governor_transfer() -> Pubkey {
    Pubkey::find_program_address(
        &[GOVERNOR_ID, derive_governor_id().as_ref()],
        &glow_airspace::ID,
    )
    .0
}

/// Derive the airspace address for a given seed
pub fn derive_airspace(seed: &str) -> Pubkey {
    Pubkey::find_program_address(&[AIRSPACE, seed.as_bytes()], &glow_airspace::ID).0
}

// Derive the airspace transfer addres for a given airspace
pub fn derive_airspace_transfer(airspace: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[AIRSPACE, airspace.as_ref()], &glow_airspace::ID).0
}

/// Derive the address for the account identifying permit issuers
pub fn derive_issuer_id(airspace: &Pubkey, issuer: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[AIRSPACE_PERMIT_ISSUER, airspace.as_ref(), issuer.as_ref()],
        &glow_airspace::ID,
    )
    .0
}

/// Derive the address for a user's permit to use the airspace
pub fn derive_permit(airspace: &Pubkey, user: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[AIRSPACE_PERMIT, airspace.as_ref(), user.as_ref()],
        &glow_airspace::ID,
    )
    .0
}
