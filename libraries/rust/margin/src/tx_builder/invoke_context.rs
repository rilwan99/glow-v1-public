//! This module only defines the generic code for executing margin invocations.
//! Other modules define ways to use this context to invoke specific adapters.

use glow_instructions::{
    margin::{
        accounting_invoke, accounting_invoke_many, adapter_invoke, adapter_invoke_many,
        liquidator_invoke, liquidator_invoke_many,
    },
    MintInfo,
};
use glow_solana_client::{signature::NeedsSignature, transaction::TransactionBuilder};
use solana_sdk::{instruction::Instruction, pubkey::Pubkey};

/// Minimum information necessary to decide how to wrap an instruction in a
/// margin invoke. Simpler alternative to MarginTxBuilder, to minimize
/// dependencies.
///
/// Data that is needed for a MarginTxBuilder, but not this:
/// - an RPC client
/// - signer keypair
/// - margin account seed
/// - margin account owner
/// - payer
pub struct MarginInvokeContext {
    /// The airspace where the margin account is authorized.
    pub airspace: Pubkey,
    /// The margin account that will wrap the instruction.
    pub margin_account: Pubkey,
    /// The signer who may authorize actions for the margin account.
    pub authority: Pubkey,
    /// Is the authority a liquidator?
    pub is_liquidator: bool,
}

impl MarginInvokeContext {
    fn invoke_ix(&self, inner: Instruction, fee_mint: Option<MintInfo>) -> Instruction {
        let MarginInvokeContext {
            airspace,
            margin_account,
            authority,
            is_liquidator,
        } = self;
        if inner.needs_signature(*margin_account) {
            if *is_liquidator {
                liquidator_invoke(
                    *airspace,
                    *authority,
                    *margin_account,
                    fee_mint.unwrap(),
                    inner,
                )
            } else {
                adapter_invoke(*airspace, *authority, *margin_account, inner)
            }
        } else {
            accounting_invoke(*airspace, *margin_account, inner)
        }
    }

    fn invoke_ix_many(&self, inner: Vec<Instruction>, fee_mint: Option<MintInfo>) -> Instruction {
        let MarginInvokeContext {
            airspace,
            margin_account,
            authority,
            is_liquidator,
        } = self;
        if inner.needs_signature(*margin_account) {
            if *is_liquidator {
                liquidator_invoke_many(
                    *airspace,
                    *authority,
                    *margin_account,
                    fee_mint.unwrap(),
                    inner,
                )
            } else {
                adapter_invoke_many(*airspace, *authority, *margin_account, inner)
            }
        } else {
            accounting_invoke_many(*airspace, *margin_account, inner)
        }
    }
}

impl MarginInvokeContext {
    /// Invoke margin adapters through a margin account using whatever wrapper
    /// is needed: adapter_invoke, accounting_invoke, or liquidator_invoke.  
    pub fn invoke(&self, inner: Instruction, fee_mint: Option<MintInfo>) -> TransactionBuilder {
        self.invoke_ix(inner, fee_mint).into()
    }

    // /// Applies `invoke` individually to each instruction and returns a vec of results
    // pub fn invoke_each(&self, inners: Vec<Instruction>) -> Vec<TransactionBuilder> {
    //     inners.into_iter().map(|ix| self.invoke(ix)).collect()
    // }

    /// Invokes many instructions as part of a single transaction call
    pub fn invoke_many(
        &self,
        inners: Vec<Instruction>,
        fee_mint: Option<MintInfo>,
    ) -> TransactionBuilder {
        self.invoke_ix_many(inners, fee_mint).into()
    }

    // /// Individually invokes each instruction and combines it into a single
    // /// bundle.
    // ///
    // /// This is a more efficient alterative to joining the result from
    // /// invoke_each:
    // /// ```rust ignore
    // /// self.invoke_joined(ixs) == self.invoke_each(ixs).join()
    // /// ```
    // pub fn invoke_joined(&self, inners: Vec<Instruction>) -> TransactionBuilder {
    //     let mut all_wrapped = vec![];
    //     for inner in inners {
    //         all_wrapped.push(self.invoke_ix(inner));
    //     }
    //     all_wrapped.into()
    // }
}

/// Extension methods for Instruction and TransactionBuilder.
///
/// These traits are for improving the readability of operations with
/// collections of instructions and chained method calls.
///
/// Inverts the receiver for methods of MarginTestContext, so Instruction or
/// Vec<Instruction> can be the receiver. This means you can chain method calls
/// in a builder pattern as a clear sequence of steps:
/// ```ignore
/// ix_builder
///     .borrow(100)
///     .invoke(ctx)
///     .send_and_confirm(rpc)
/// ```
pub mod invoke_into {
    use super::*;

    /// Defines the way to unpack some type, invoke any containing instructions
    /// through margin, and pack it back into the same type.
    pub trait InvokeEachInto: Sized {
        /// Separately invokes each instruction into a desired type.
        fn invoke_each_into(self, ctx: &MarginInvokeContext) -> Vec<TransactionBuilder>;
    }

    impl InvokeEachInto for Instruction {
        fn invoke_each_into(self, ctx: &MarginInvokeContext) -> Vec<TransactionBuilder> {
            vec![ctx.invoke(self, None)]
        }
    }

    // impl InvokeEachInto for Vec<Instruction> {
    //     fn invoke_each_into(self, ctx: &MarginInvokeContext) -> Vec<TransactionBuilder> {
    //         ctx.invoke_each(self)
    //     }
    // }

    /// Defines the way to unpack some type, invoke any containing instructions
    /// through margin, and pack it all into a single TransactionBuilder.
    ///
    /// NOTE: This is unsuitable for use by liquidators as the `fee_mint` is empty.
    pub trait InvokeInto: Sized {
        /// Invoke a margin adapter through a margin account using whichever wrapper
        /// is needed: adapter_invoke, accounting_invoke, or liquidator_invoke. If
        /// there are multiple instructions, they are combined into a single
        /// TransactionBuilder
        fn invoke_into(self, ctx: &MarginInvokeContext) -> TransactionBuilder;

        /// Invoke a margin adapter through a margin account using whichever wrapper
        /// is needed for all the instructions.
        fn invoke_many_into(self, ctx: &MarginInvokeContext) -> TransactionBuilder;
    }

    impl InvokeInto for Instruction {
        fn invoke_into(self, ctx: &MarginInvokeContext) -> TransactionBuilder {
            ctx.invoke(self, None)
        }

        fn invoke_many_into(self, ctx: &MarginInvokeContext) -> TransactionBuilder {
            ctx.invoke(self, None)
        }
    }

    impl InvokeInto for Vec<Instruction> {
        fn invoke_into(self, ctx: &MarginInvokeContext) -> TransactionBuilder {
            ctx.invoke_many(self, None)
        }

        fn invoke_many_into(self, ctx: &MarginInvokeContext) -> TransactionBuilder {
            ctx.invoke_many(self, None)
        }
    }
}
