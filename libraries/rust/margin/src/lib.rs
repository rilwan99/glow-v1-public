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

//! Margin SDK
//!
//! This crate is the official Rust SDK for the Margin family of programs.
//! It includes instruction and transaction builders that allow users of our
//! programs to conveniently interact with them.
//!
//! The SDK currently supports the following programs and adapters:
//! * Margin - create, manage and interact with [margin::MarginAccount]s
//! * Margin Pool - an adapter for borrowing and lending in our pools
//!
//! A good starting point for using the SDK is to create a margin account.
//!
//! ```ignore
//! use std::sync::Arc;
//!
//! use glow_simulation::solana_rpc_api::{RpcConnection, SolanaRpcClient};
//! use solana_client::rpc_client::nonblocking::RpcClient;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!   // Create an RPC connection
//!   let client = RpcClient::new("https://my-endpoint.com");
//!   let rpc = RpcConnection::new(payer, client);
//!   // Create a transaction builder
//!   let tx_builder = margin_sdk::tx_builder::MarginTxBuilder::new(&rpc, ...);
//!   // Create a transaction to register a margin account
//!   let tx = tx_builder.create_account().await?;
//!   // Submit transaction
//!   rpc.send_and_confirm_transaction(&tx).await?;
//! }
//! ```

#![warn(missing_docs)]

/// retrieve on-chain state
pub mod get_state;
/// Instruction builders for programs and adapters supported by the SDK
pub mod ix_builder;
/// ease of use for reading a MarginAccount
pub mod margin_account_ext;
/// generic code to integrate adapters with margin
pub mod margin_integrator;
/// generically refreshing positions in a margin account
pub mod refresh;
/// things that should be provided by the solana sdk, but are not
pub mod solana;
/// Utilities for tokens and token prices
pub mod tokens;
/// Transaction builder
pub mod tx_builder;
/// General purpose logic used by this lib and clients, unrelated to glow or solana
pub mod util;

/// Lookup tables
pub mod lookup_tables;

/// Utilities for test environments
pub mod test_service;

pub use glow_airspace;
pub use glow_margin;
pub use glow_margin_pool;
pub use glow_metadata;
pub use glow_test_service;

pub use glow_solana_client::cat;
