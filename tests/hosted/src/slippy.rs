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

//! Helpers for creating and funding slippy pools for testing

// Create pool
// Fund the pool

use std::sync::Arc;

use glow_instructions::{test_service::init_slippy_pool, MintInfo};
use glow_simulation::solana_rpc_api::SolanaRpcClient;
use solana_program_test::ProgramTestContext;
use solana_sdk::{pubkey::Pubkey, signature::Keypair, signer::Signer};

#[derive(Clone)]
pub struct TestSlippyPool {
    pub address: Pubkey,
    pub mint_a: MintInfo,
    pub mint_b: MintInfo,
    pub vault_a: Pubkey,
    pub vault_b: Pubkey,
}

impl TestSlippyPool {
    pub async fn setup_pool(
        rpc: &Arc<dyn SolanaRpcClient>,
        mint_a: MintInfo,
        mint_b: MintInfo,
        owner: &Keypair,
    ) -> anyhow::Result<Self> {
        let (address, pool_ix) = init_slippy_pool(mint_a, mint_b, owner.pubkey());
        let tx = rpc.create_transaction(&[owner], &[pool_ix]).await?;
        rpc.send_and_confirm_transaction(tx).await?;

        Ok(Self {
            address,
            mint_a,
            mint_b,
            vault_a: Pubkey::find_program_address(
                &[address.as_ref(), mint_a.address.as_ref()],
                &glow_test_service::ID,
            )
            .0,
            vault_b: Pubkey::find_program_address(
                &[address.as_ref(), mint_b.address.as_ref()],
                &glow_test_service::ID,
            )
            .0,
        })
    }
}
