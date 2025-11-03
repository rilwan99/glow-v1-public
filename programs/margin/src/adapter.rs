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

use std::collections::{HashMap, HashSet};

use crate::events;
use anchor_lang::{
    prelude::*,
    solana_program::{instruction::Instruction, program},
};
use glow_program_common::{
    oracle::TokenPriceOracle, Number128, JUPITER_V6, KNOWN_EXTERNAL_PROGRAMS,
    SAFE_RETURN_DATA_PROGRAMS,
};
use pyth_solana_receiver_sdk::price_update::PriceUpdateV2;
use solana_program::clock::UnixTimestamp;

use crate::{
    syscall::{sys, Sys},
    util::Require,
    AccountPositionKey, AdapterConfig, AdapterPositionFlags, Approver, ErrorCode, MarginAccount,
    PositionConfigUpdate, PriceInfo, SignerSeeds, TokenConfig, MAX_ORACLE_CONFIDENCE,
    MAX_ORACLE_STALENESS,
};
pub struct InvokeAdapter<'b, 'c: 'info, 'info> {
    /// The margin account to proxy an action for
    pub margin_account: &'b AccountLoader<'info, MarginAccount>,

    /// The program to be invoked
    pub adapter_program: &'b AccountInfo<'info>,

    /// The accounts to be passed through to the adapter
    pub accounts: &'c [AccountInfo<'info>],

    /// The transaction was signed by the authority of the margin account.
    /// Thus, the invocation should be signed by the margin account.
    pub signed: bool,
}

impl InvokeAdapter<'_, '_, '_> {
    /// those who approve of the requests within the adapter result
    fn adapter_result_approvals(&self) -> Vec<Approver> {
        let mut ret = Vec::new();
        if self.signed {
            ret.push(Approver::MarginAccountAuthority);
        }
        ret.push(Approver::Adapter(self.adapter_program.key()));

        ret
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Default)]
pub struct AdapterResult {
    /// keyed by token mint, same as position
    pub position_changes: Vec<(Pubkey, Vec<PositionChange>)>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub enum PositionChange {
    /// The price/value of the position has already changed,
    /// so the margin account must update its price
    Price(PriceChangeInfo),

    /// Flags that are true here will be set to the bool in the position
    /// Flags that are false here will be unchanged in the position
    Flags(AdapterPositionFlags, bool),

    /// Register a new position, or assert that a position is registered
    /// if the position cannot be registered, instruction fails
    /// if the position is already registered, instruction succeeds without taking action
    Register(Pubkey),

    /// Close a position, or assert that a position is closed
    /// if the position cannot be closed, instruction fails
    /// if the position does not exist, instruction succeeds without taking action
    Close(Pubkey),

    /// A change in a tokens during margin pool actions.
    /// Used by the program to assess a correct liquidation fee.
    /// Liquidators can exploit the fee calculation by over-trading tokens and only
    /// paying a small portion. If their fee is based only on how much they traded,
    /// they could over-extract liquidation fees.
    /// Thus this exists as a guard to ensure that the lower of the traded amount and
    /// the repaid amount(s).
    ///
    /// The return number is a USD value
    TokenChange(TokenBalanceChange),
}

/// Price change info, that is convertible to [PriceInfo].
///
/// NOTE: The struct's fields are intentionally set as private. Please do not change them
///       nor access them directly. Prefer using [PriceInfo] as it includes validations
///       for EMA and confidence.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy)]
pub struct PriceChangeInfo {
    /// The current price of the asset
    value: i64,

    /// The current confidence value for the asset price
    confidence: u64,

    /// The recent average price (https://www.pyth.network/blog/whats-in-a-name)
    ema: i64,

    /// The time that the price was published at
    publish_time: i64,

    /// The exponent for the price values
    exponent: i32,
}
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, Default)]
pub struct TokenBalanceChange {
    pub mint: Pubkey,
    pub tokens: u64,
    pub change_cause: TokenBalanceChangeCause,
}

#[repr(u8)]
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, Default, PartialEq)]
pub enum TokenBalanceChangeCause {
    // Default cause, should not be encountered in the wild
    #[default]
    Default,
    // Tokens were borrowed
    Borrow,
    // Tokens were repaid
    Repay,
    // An external adapter caused tokens to increase (e.g. swap in)
    ExternalIncrease,
    // An external adapter caused tokens to decrease
    ExternalDecrease,
}

impl PriceChangeInfo {
    pub const fn new(
        value: i64,
        confidence: u64,
        ema: i64,
        publish_time: i64,
        exponent: i32,
    ) -> Self {
        Self {
            value,
            confidence,
            ema,
            publish_time,
            exponent,
        }
    }

    /// Convert into [PriceInfo], checking that EMA and confidence are valid.
    /// The returned price info should be checked for validity if used directly.
    pub fn to_price_info(self, unix_timestamp: UnixTimestamp) -> PriceInfo {
        let max_confidence = Number128::from_bps(MAX_ORACLE_CONFIDENCE);

        let ema = Number128::from_decimal(self.ema, self.exponent);
        let confidence = Number128::from_decimal(self.confidence, self.exponent);

        if ema == Number128::ZERO {
            msg!("avg price cannot be zero");
            return PriceInfo::new_invalid();
        }

        match (confidence, self.publish_time) {
            (c, _) if (c / ema) > max_confidence => {
                msg!("price confidence exceeding max");
                PriceInfo::new_invalid()
            }
            (_, publish_time) if (unix_timestamp - publish_time) > MAX_ORACLE_STALENESS as i64 => {
                msg!(
                    "price timestamp is too old/stale. published: {}, now: {}",
                    publish_time,
                    unix_timestamp
                );
                PriceInfo::new_invalid()
            }
            _ => PriceInfo::new_valid(self.exponent, self.value, unix_timestamp as u64),
        }
    }

    pub fn try_from_pyth_pull(
        price: &PriceUpdateV2,
        feed_id: &[u8; 32],
        clock: &Clock,
    ) -> Result<Self> {
        let price_obj = price.get_price_no_older_than(clock, MAX_ORACLE_STALENESS, feed_id)?;
        Ok(Self {
            value: price_obj.price,
            confidence: price_obj.conf,
            ema: price.price_message.ema_price,
            publish_time: price_obj.publish_time,
            exponent: price_obj.exponent,
        })
    }

    pub fn try_from_pyth_pull_redemption(
        price: &PriceUpdateV2,
        quote: &PriceUpdateV2,
        feed_id: &[u8; 32],
        quote_feed_id: &[u8; 32],
        clock: &Clock,
    ) -> Result<Self> {
        let price_obj = price.get_price_no_older_than(clock, MAX_ORACLE_STALENESS, feed_id)?;
        let quote_obj =
            quote.get_price_no_older_than(clock, MAX_ORACLE_STALENESS, quote_feed_id)?;

        let price_value = Number128::from_decimal(price_obj.price, price_obj.exponent);
        let price_ema = Number128::from_decimal(price.price_message.ema_price, price_obj.exponent);
        let price_conf = Number128::from_decimal(price_obj.conf, price_obj.exponent);

        let quote_value = Number128::from_decimal(quote_obj.price, quote_obj.exponent);
        let quote_ema = Number128::from_decimal(quote.price_message.ema_price, quote_obj.exponent);
        let quote_conf = Number128::from_decimal(quote_obj.conf, quote_obj.exponent);

        let value = (price_value * quote_value)
            .as_u64(price_obj.exponent)
            .try_into()
            .map_err(|_| error!(crate::ErrorCode::MathOpFailed))?;
        // Confidence = price_conf * quote_value + quote_conf
        let confidence = ((price_conf * quote_value) + quote_conf).as_u64(price_obj.exponent);
        let ema = (price_ema * quote_ema)
            .as_u64(price_obj.exponent)
            .try_into()
            .map_err(|_| error!(crate::ErrorCode::MathOpFailed))?;
        let publish_time = price_obj.publish_time.min(quote_obj.publish_time);

        Ok(Self {
            value,
            confidence,
            ema,
            publish_time,
            exponent: price_obj.exponent,
        })
    }

    /// Construct from Pyth oracles, validating the type of oracle in the process.
    pub fn try_from_oracle_accounts(
        price: &AccountInfo,
        quote: &Option<AccountInfo>,
        price_oracle: &TokenPriceOracle,
        clock: &Clock,
    ) -> Result<Self> {
        let min_oracle_freshness = clock.unix_timestamp - MAX_ORACLE_STALENESS as i64;
        // check account ownership
        match price_oracle {
            TokenPriceOracle::NoOracle => err!(crate::ErrorCode::InvalidOracle),
            TokenPriceOracle::PythPull { feed_id } => {
                verify_oracle_ownership(price)?;
                let oracle_data = price.try_borrow_data()?;
                let update = PriceUpdateV2::try_deserialize(&mut &oracle_data[..])?;
                if update.price_message.publish_time < min_oracle_freshness {
                    msg!("stale oracle: {}", update.price_message.publish_time);
                }
                Self::try_from_pyth_pull(&update, feed_id, clock)
            }
            TokenPriceOracle::PythPullRedemption {
                feed_id,
                quote_feed_id,
            } => {
                let quote = quote.as_ref().ok_or(crate::ErrorCode::InvalidOracle)?;
                verify_oracle_ownership(price)?;
                verify_oracle_ownership(quote)?;

                let oracle_data = price.try_borrow_data()?;
                let price = PriceUpdateV2::try_deserialize(&mut &oracle_data[..])?;
                if price.price_message.publish_time < min_oracle_freshness {
                    msg!("stale oracle: {}", price.price_message.publish_time);
                }

                let oracle_data = quote.try_borrow_data()?;
                let quote = PriceUpdateV2::try_deserialize(&mut &oracle_data[..])?;
                if quote.price_message.publish_time < min_oracle_freshness {
                    msg!("stale quote oracle: {}", quote.price_message.publish_time);
                }
                Self::try_from_pyth_pull_redemption(&price, &quote, feed_id, quote_feed_id, clock)
            }
        }
    }
}

/// Verify oracle ownership based on program comppile feature flags.
///
/// * On mainnet, accounts must be owned by the Pyth receiver program.
/// * On devnet, we allow the test service.
/// * If testing, any oracle is allowed.
pub fn verify_oracle_ownership(_account: &AccountInfo) -> Result<()> {
    #[cfg(not(feature = "testing"))]
    {
        // The account must be owned by the Pyth receiver or our test program (devnet) if not testing
        #[cfg(feature = "devnet")]
        require!(
            _account.owner == &pubkey!("test7JXXboKpc8hGTadvoXcFWN4xgnHLGANU92JKrwA"),
            crate::ErrorCode::InvalidOracle
        );
        #[cfg(not(feature = "devnet"))]
        require!(
            _account.owner == &pyth_solana_receiver_sdk::id(),
            crate::ErrorCode::InvalidOracle
        );
    }
    Ok(())
}

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct IxData {
    pub num_accounts: u8,
    pub data: Vec<u8>,
}

#[derive(Accounts)]
pub struct AdapterAccounts<'info> {
    /// CHECK:
    pub adapter_program: AccountInfo<'info>,

    #[account(has_one = adapter_program @ ErrorCode::UnauthorizedInvocation)]
    adapter_config: Account<'info, AdapterConfig>,
}

/// Invoke a margin adapter with the requested data
/// * `signed` - sign with the margin account
///
/// accounts structure:
///
/// remaining accounts repeat this pattern for each invoke:
///
/// /// The program to be invoked
/// adapter_program: AccountInfo<'info>,
///
/// /// The config about the proxy program
/// #[account(has_one = adapter_program)]
/// adapter_config: Account<'info, AdapterConfig>,
///
/// /// all accounts needed for specific instruction
/// instruction_accounts: Vec<AccountInfo<'info>>
pub fn invoke_many<'info>(
    margin_account: &AccountLoader<'info, MarginAccount>,
    accounts: &'info [AccountInfo<'info>],
    data: Vec<IxData>,
    signed: bool,
) -> Result<Vec<TokenBalanceChange>> {
    let mut token_changes = vec![];
    let mut account_ix = 0;
    let total_accounts_per_ix_data: usize = data.iter().map(|v| v.num_accounts as usize + 2).sum();
    require!(
        accounts.len() == total_accounts_per_ix_data,
        ErrorCode::InvalidInvokeAccounts
    );
    for IxData { num_accounts, data } in data {
        let mut bumps = Default::default();
        let mut reallocs = Default::default();
        let adapter_accounts = &accounts[account_ix..(account_ix + 2)];
        account_ix += 2;
        let adapter_accounts = AdapterAccounts::try_accounts(
            &crate::ID,
            &mut &adapter_accounts[..],
            &[],
            &mut bumps,
            &mut reallocs,
        )?;
        // Check airspace permission
        require!(
            adapter_accounts.adapter_config.airspace == margin_account.load()?.airspace,
            ErrorCode::WrongAirspace
        );
        let remaining_accounts = &accounts[account_ix..(account_ix + num_accounts as usize)];
        account_ix += num_accounts as usize;

        token_changes.extend_from_slice(&invoke(
            &InvokeAdapter {
                margin_account,
                adapter_program: &adapter_accounts.adapter_program,
                accounts: remaining_accounts,
                signed,
            },
            data,
        )?);

        if adapter_accounts.adapter_program.key() == JUPITER_V6 {
            emit!(events::JupiterSwap {
                margin_account: margin_account.key(),
                adapter_program: adapter_accounts.adapter_program.key(),
            });
        }
    }

    Ok(token_changes)
}

/// Invoke a margin adapter with the requested data
/// * `signed` - sign with the margin account
pub fn invoke<'b, 'c: 'info, 'info>(
    ctx: &InvokeAdapter<'b, 'c, 'info>,
    data: Vec<u8>,
) -> Result<Vec<TokenBalanceChange>> {
    let signer = ctx.margin_account.load()?.signer_seeds_owned();

    let accounts = ctx
        .accounts
        .iter()
        .map(|info| AccountMeta {
            pubkey: info.key(),
            is_signer: if info.key() == ctx.margin_account.key() {
                ctx.signed
            } else {
                info.is_signer
            },
            is_writable: info.is_writable,
        })
        .collect::<Vec<AccountMeta>>();

    // <address: Pubkey, (mint: Pubkey, balance: u64, token_program: u8)>
    // where:
    //      token_program = 0
    //      token_2022_program = 1
    let mut token_balances: HashMap<Pubkey, (Pubkey, u64, u8)> = HashMap::new();

    if KNOWN_EXTERNAL_PROGRAMS.contains(ctx.adapter_program.key) {
        // Track balance changes if the invocation is for known external programs
        for account_info in ctx.accounts {
            // Looking for (writable) token accounts to get their balances before the invocation.
            // Short-circuit
            if !account_info.is_writable || account_info.executable {
                continue;
            }
            match account_info.owner {
                owner if owner == &anchor_spl::token::ID => {
                    // Check if it serializes to a token
                    let data = &mut &**account_info.try_borrow_data()?;
                    if let Ok(account) = anchor_spl::token::TokenAccount::try_deserialize(data) {
                        if account.owner == ctx.margin_account.key() {
                            // Only track margin account owned balance changes
                            token_balances
                                .insert(account_info.key(), (account.mint, account.amount, 0));
                        }
                    }
                }
                owner if owner == &anchor_spl::token_2022::ID => {
                    let data = &mut &**account_info.try_borrow_data()?;
                    if let Ok(account) =
                        anchor_spl::token_interface::TokenAccount::try_deserialize(data)
                    {
                        if account.owner == ctx.margin_account.key() {
                            // Only track margin account owned balance changes
                            token_balances
                                .insert(account_info.key(), (account.mint, account.amount, 1));
                        }
                    }
                }
                _ => continue,
            }
        }
    }

    let instruction = Instruction {
        program_id: ctx.adapter_program.key(),
        accounts,
        data,
    };

    ctx.margin_account.load_mut()?.invocation.start();
    if ctx.signed {
        program::invoke_signed(&instruction, ctx.accounts, &[&signer.signer_seeds()])?;
    } else {
        program::invoke(&instruction, ctx.accounts)?;
    }
    ctx.margin_account.load_mut()?.invocation.end();

    // Reconcile token balance changes
    // TODO: This is in a way duplicates update_balances, deduplicate them.
    let mut token_balance_changes = Vec::with_capacity(token_balances.len());
    // TODO: We should only call this for known external programs, and not both known ext and internal
    if KNOWN_EXTERNAL_PROGRAMS.contains(ctx.adapter_program.key) {
        let mut visited_tokens = HashSet::new();
        for account_info in ctx.accounts {
            // Prevent accounting for the same account more than once
            if visited_tokens.contains(account_info.key) {
                continue;
            }
            if let Some((mint, opening_balance, program)) = token_balances.get(account_info.key) {
                visited_tokens.insert(account_info.key());
                let new_balance = match *program {
                    0 => {
                        let data = &mut &**account_info.try_borrow_data()?;
                        let account = anchor_spl::token::TokenAccount::try_deserialize(data)?;
                        account.amount
                    }
                    1 => {
                        let data = &mut &**account_info.try_borrow_data()?;
                        let account =
                            anchor_spl::token_interface::TokenAccount::try_deserialize(data)?;
                        account.amount
                    }
                    _ => unreachable!(),
                };
                match new_balance.cmp(opening_balance) {
                    std::cmp::Ordering::Equal => continue,
                    std::cmp::Ordering::Greater => {
                        token_balance_changes.push(TokenBalanceChange {
                            mint: *mint,
                            tokens: new_balance.checked_sub(*opening_balance).unwrap(),
                            change_cause: TokenBalanceChangeCause::ExternalIncrease,
                        });
                    }
                    std::cmp::Ordering::Less => {
                        token_balance_changes.push(TokenBalanceChange {
                            mint: *mint,
                            tokens: opening_balance.checked_sub(new_balance).unwrap(),
                            change_cause: TokenBalanceChangeCause::ExternalDecrease,
                        });
                    }
                }
            }
        }
    }

    // SECURITY: handle_adapter_result should return an empty list of changes if it's not
    //           called on programs whose return data it is safe to read.
    //           Thus token_balance_changes should either be populated here or in h_a_r.
    token_balance_changes.extend_from_slice(&handle_adapter_result(ctx)?);

    Ok(token_balance_changes)
}

fn handle_adapter_result<'b, 'c: 'info, 'info>(
    ctx: &InvokeAdapter<'b, 'c, 'info>,
) -> Result<Vec<TokenBalanceChange>> {
    update_balances(ctx)?;

    let mut token_changes = vec![];

    // Only read return data set by us and other trusted programs
    if SAFE_RETURN_DATA_PROGRAMS.contains(ctx.adapter_program.key)
        || ctx.adapter_program.key == &crate::ID
    {
        require!(
            !KNOWN_EXTERNAL_PROGRAMS.contains(ctx.adapter_program.key),
            ErrorCode::IncorrectProgramReturnData
        );
        match program::get_return_data() {
            None => (),
            Some((program_id, _)) if program_id != ctx.adapter_program.key() => (),
            Some((_, data)) => {
                let result = AdapterResult::deserialize(&mut &data[..])?;
                for (mint, changes) in result.position_changes {
                    token_changes.extend_from_slice(&apply_changes(ctx, mint, changes)?);
                }
            }
        }
    }

    // clear return data after reading it
    program::set_return_data(&[]);

    Ok(token_changes)
}

fn update_balances(ctx: &InvokeAdapter) -> Result<()> {
    let margin_account = &mut ctx.margin_account.load_mut()?;
    for account_info in ctx.accounts {
        if account_info.owner == &anchor_spl::token::ID {
            let data = &mut &**account_info.try_borrow_data()?;
            if let Ok(account) = anchor_spl::token::TokenAccount::try_deserialize(data) {
                match margin_account.set_position_balance_with_clock(
                    &account.mint,
                    account_info.key,
                    account.amount,
                ) {
                    Ok(_) => (),
                    Err(ErrorCode::PositionNotRegistered) => (),
                    Err(err) => return Err(err.into()),
                }
            }
        } else if account_info.owner == &anchor_spl::token_2022::ID {
            let data = &mut &**account_info.try_borrow_data()?;
            if let Ok(account) = anchor_spl::token_interface::TokenAccount::try_deserialize(data) {
                match margin_account.set_position_balance_with_clock(
                    &account.mint,
                    account_info.key,
                    account.amount,
                ) {
                    Ok(_) => (),
                    Err(ErrorCode::PositionNotRegistered) => (),
                    Err(err) => return Err(err.into()),
                }
            }
        }
    }

    Ok(())
}

fn apply_changes<'b, 'c: 'info, 'info>(
    ctx: &InvokeAdapter<'b, 'c, 'info>,
    mint: Pubkey,
    changes: Vec<PositionChange>,
) -> Result<Vec<TokenBalanceChange>> {
    let margin_account = &mut ctx.margin_account.load_mut()?;
    let mut key = margin_account.get_position_key(&mint);
    let mut position = key.and_then(|k| margin_account.get_position_by_key_mut(&k));
    if let Some(ref p) = position {
        // There are margin owned positions, which should be considered too
        if p.adapter != ctx.adapter_program.key() && p.adapter != Pubkey::default() {
            return err!(ErrorCode::InvalidPositionAdapter);
        }
    }
    let mut token_changes = vec![];
    for change in changes {
        position = key.and_then(|k| margin_account.get_position_by_key_mut(&k));
        match change {
            PositionChange::Price(px) => {
                if let Some(pos) = position {
                    pos.set_price(&px.to_price_info(sys().unix_timestamp() as UnixTimestamp))?;
                }
            }
            PositionChange::Flags(flags, true) => position.require_mut()?.flags |= flags,
            PositionChange::Flags(flags, false) => position.require_mut()?.flags &= !flags,
            PositionChange::Register(token_account) => match position {
                Some(pos) => {
                    if pos.address != token_account {
                        msg!("position already registered for this mint with a different token account");
                        return err!(ErrorCode::PositionNotRegisterable);
                    } else {
                        // Should not try to register a position again as it already exists
                        msg!("position already registered for this mint with this token account");
                        return err!(ErrorCode::PositionAlreadyRegistered);
                    }
                }
                None => {
                    key = Some(register_position(
                        margin_account,
                        ctx.accounts,
                        ctx.adapter_result_approvals().as_slice(),
                        mint,
                        token_account,
                    )?);
                }
            },
            PositionChange::Close(token_account) => {
                if let Some(pos) = position {
                    if pos.address != token_account {
                        msg!("position registered for this mint with a different token account");
                        return err!(ErrorCode::PositionNotRegisterable);
                    }
                    margin_account.unregister_position(
                        &mint,
                        &token_account,
                        ctx.adapter_result_approvals().as_slice(),
                    )?;
                    key = None;
                } else {
                    // Should fail to close a position if it's not registered
                    msg!("trying to close a position that does not exist");
                    return err!(ErrorCode::PositionNotRegistered);
                }
            }
            PositionChange::TokenChange(token_change) => {
                // Propagate the token changes up
                token_changes.push(token_change);
            }
        }
    }
    Ok(token_changes)
}

fn register_position<'info>(
    margin_account: &mut MarginAccount,
    remaining_accounts: &'info [AccountInfo<'info>],
    approvals: &[Approver],
    mint_address: Pubkey,
    token_account_address: Pubkey,
) -> Result<AccountPositionKey> {
    let mut token_config: Option<Account<TokenConfig>> = None;
    // Separated these as it was not possible to use token_interface
    let mut token_account: Result<Account<anchor_spl::token::TokenAccount>> =
        err!(ErrorCode::PositionNotRegisterable);
    let mut token_2022_account: Result<
        InterfaceAccount<anchor_spl::token_interface::TokenAccount>,
    > = err!(ErrorCode::PositionNotRegisterable);
    let mut mint: Result<Account<anchor_spl::token::Mint>> =
        err!(ErrorCode::PositionNotRegisterable);
    let mut mint_2022: Result<InterfaceAccount<anchor_spl::token_interface::Mint>> =
        err!(ErrorCode::PositionNotRegisterable);
    for info in remaining_accounts {
        if info.key == &token_account_address {
            if info.owner == &anchor_spl::token::ID {
                token_account = Ok(Account::<'info>::try_from(info)?);
            } else {
                token_2022_account = Ok(InterfaceAccount::<'info>::try_from(info)?);
            }
        } else if info.key == &mint_address {
            if info.owner == &anchor_spl::token::ID {
                mint = Ok(Account::<'info>::try_from(info)?)
            } else {
                mint_2022 = Ok(InterfaceAccount::<'info>::try_from(info)?)
            }
        } else if info.owner == &TokenConfig::owner() {
            // Also check if this is likely a tokenConfig, due to data length
            let maybe_config: Result<Account<TokenConfig>> = Account::<'info>::try_from(info);
            if let Ok(config) = maybe_config {
                if config.mint == mint_address {
                    token_config = Some(config);
                }
            }
        }
    }

    let (key, token_balance) = match (token_account, mint, token_2022_account, mint_2022) {
        (Ok(token), Ok(mint), Err(_), Err(_)) => {
            if mint.key() != token.mint {
                msg!("token account has the wrong mint");
                return err!(ErrorCode::PositionNotRegisterable);
            }
            let key = match token_config {
                Some(config) => margin_account.register_position(
                    PositionConfigUpdate::new_from_config(
                        &config,
                        mint.decimals,
                        token.key(),
                        config.adapter_program().unwrap_or_default(),
                    )?,
                    approvals,
                )?,
                // TODO: remove backwards compat
                None => {
                    msg!("a TokenConfig is necessary to register a margin position");
                    return err!(ErrorCode::PositionNotRegisterable);
                }
            };
            (key, token.amount)
        }
        (Err(_), Err(_), Ok(token), Ok(mint)) => {
            if mint.key() != token.mint {
                msg!("token account has the wrong mint");
                return err!(ErrorCode::PositionNotRegisterable);
            }
            let key = match token_config {
                Some(config) => margin_account.register_position(
                    PositionConfigUpdate::new_from_config(
                        &config,
                        mint.decimals,
                        token.key(),
                        config.adapter_program().unwrap_or_default(),
                    )?,
                    approvals,
                )?,
                // TODO: remove backwards compat
                None => {
                    msg!("a TokenConfig is necessary to register a margin position");
                    return err!(ErrorCode::PositionNotRegisterable);
                }
            };
            (key, token.amount)
        }
        _ => return err!(ErrorCode::PositionNotRegisterable),
    };

    margin_account.set_position_balance_with_clock(
        &mint_address,
        &token_account_address,
        token_balance,
    )?;

    Ok(key)
}

#[cfg(test)]
mod test {
    use std::{collections::HashMap, mem::size_of};

    use anchor_lang::Discriminator;

    use crate::{AccountConstraints, AccountFeatureFlags, TokenFeatures};

    use super::*;

    fn all_change_types(position_address: Pubkey) -> Vec<PositionChange> {
        vec![
            PositionChange::Price(PriceChangeInfo {
                value: 0,
                confidence: 0,
                ema: 0,
                publish_time: 0,
                exponent: 0,
            }),
            PositionChange::Flags(AdapterPositionFlags::empty(), true),
            PositionChange::Register(position_address),
            PositionChange::Close(position_address),
            PositionChange::TokenChange(TokenBalanceChange::default()),
        ]
    }

    #[test]
    fn can_apply_close_position_changes() {
        let mut data = [0u8; 100];
        let mut lamports = 0u64;
        let default = Pubkey::default();
        let margin = MarginAccount::owner();
        let adapter_address = Pubkey::new_unique();
        let adapter = AccountInfo::new(
            &adapter_address,
            false,
            false,
            &mut lamports,
            &mut data,
            &default,
            false,
            0,
        );
        let mut margin_account = MarginAccount {
            version: 1,
            bump_seed: [0],
            user_seed: [0, 0],
            invocation: Default::default(),
            features: AccountFeatureFlags::default(),
            constraints: AccountConstraints::default(),
            owner: Pubkey::new_unique(),
            airspace: Pubkey::new_unique(),
            liquidator: Pubkey::default(),
            positions: Default::default(),
        };
        // Register deposit
        let deposit_mint = Pubkey::new_unique();
        let deposit_address = Pubkey::new_unique();
        margin_account
            .register_position(
                PositionConfigUpdate {
                    mint: deposit_mint,
                    token_program: anchor_spl::token::ID,
                    decimals: 6,
                    address: deposit_address,
                    airspace: margin_account.airspace,
                    adapter: adapter_address,
                    kind: crate::TokenKind::AdapterCollateral,
                    value_modifier: 100,
                    max_staleness: 40,
                    token_features: TokenFeatures::empty(),
                },
                &[
                    Approver::MarginAccountAuthority,
                    Approver::Adapter(adapter_address),
                ],
            )
            .unwrap();

        // Register loan
        let loan_mint = Pubkey::new_unique();
        let loan_address = Pubkey::new_unique();
        margin_account
            .register_position(
                PositionConfigUpdate {
                    mint: loan_mint,
                    token_program: anchor_spl::token::ID,
                    decimals: 6,
                    address: loan_address,
                    airspace: margin_account.airspace,
                    adapter: adapter_address,
                    kind: crate::TokenKind::Claim,
                    value_modifier: 100,
                    max_staleness: 40,
                    token_features: TokenFeatures::empty(),
                },
                &[
                    Approver::MarginAccountAuthority,
                    Approver::Adapter(adapter_address),
                ],
            )
            .unwrap();

        let mut data = [0u8; 8 + size_of::<MarginAccount>()];
        data[..8].copy_from_slice(&MarginAccount::discriminator());
        data[8..].copy_from_slice(bytemuck::bytes_of(&margin_account));
        let mut lamports = 0u64;
        let margin_account = AccountInfo::new(
            &default,
            false,
            true,
            &mut lamports,
            &mut data,
            &margin,
            false,
            0,
        );
        let ctx = InvokeAdapter {
            margin_account: &AccountLoader::try_from(&margin_account).unwrap(),
            adapter_program: &adapter,
            accounts: &[],
            signed: true,
        };

        // Close both positions
        apply_changes(
            &ctx,
            deposit_mint,
            vec![PositionChange::Close(deposit_address)],
        )
        .unwrap();

        {
            let data = margin_account.data.borrow();
            let ser_margin_account: &MarginAccount = bytemuck::from_bytes(&data[8..]);
            // The remaining position should be the loan mint
            let positions = ser_margin_account.positions().collect::<Vec<_>>();
            assert_eq!(1, positions.len());
            let position = positions[0];
            assert_eq!(loan_mint, position.token);
            assert_eq!(loan_address, position.address);
        }
        apply_changes(&ctx, loan_mint, vec![PositionChange::Close(loan_address)]).unwrap();
    }

    #[test]
    fn position_change_types_are_required_when_appropriate() {
        let mut data = [0u8; 100];
        let mut lamports = 0u64;
        let default = Pubkey::default();
        let margin = MarginAccount::owner();
        let adapter_address = Pubkey::new_unique();
        let adapter = AccountInfo::new(
            &adapter_address,
            false,
            false,
            &mut lamports,
            &mut data,
            &default,
            false,
            0,
        );
        let mut margin_account = MarginAccount {
            version: 1,
            bump_seed: [0],
            user_seed: [0, 0],
            invocation: Default::default(),
            features: AccountFeatureFlags::default(),
            constraints: AccountConstraints::default(),
            owner: Pubkey::new_unique(),
            airspace: Pubkey::new_unique(),
            liquidator: Pubkey::default(),
            positions: Default::default(),
        };

        let position_mint = Pubkey::new_unique();
        let position_address = Pubkey::new_unique();
        margin_account
            .register_position(
                PositionConfigUpdate {
                    mint: position_mint,
                    token_program: anchor_spl::token::ID,
                    decimals: 6,
                    address: position_address,
                    airspace: margin_account.airspace,
                    adapter: adapter_address,
                    kind: crate::TokenKind::AdapterCollateral,
                    value_modifier: 100,
                    max_staleness: 40,
                    token_features: TokenFeatures::empty(),
                },
                &[
                    Approver::MarginAccountAuthority,
                    Approver::Adapter(adapter_address),
                ],
            )
            .unwrap();
        let mut data = [0u8; 8 + size_of::<MarginAccount>()];
        data[..8].copy_from_slice(&MarginAccount::discriminator());
        data[8..].copy_from_slice(bytemuck::bytes_of(&margin_account));
        let mut lamports = 0u64;
        let margin_account = AccountInfo::new(
            &default,
            false,
            true,
            &mut lamports,
            &mut data,
            &margin,
            false,
            0,
        );
        let ctx = InvokeAdapter {
            margin_account: &AccountLoader::try_from(&margin_account).unwrap(),
            adapter_program: &adapter,
            accounts: &[],
            signed: true,
        };

        for change in all_change_types(position_address) {
            let required = match change {
                // Should fail in unit tests because we can't get a sysvar in unit tests
                PositionChange::Price(_) => continue,
                PositionChange::Flags(_, _) => false,
                // If the position already exists, can't register it again
                PositionChange::Register(_) => true,
                PositionChange::Close(_) => false,
                PositionChange::TokenChange(_) => false, // TODO:
            };
            if required {
                apply_changes(&ctx, position_mint, vec![change]).unwrap_err();
            } else {
                apply_changes(&ctx, position_mint, vec![change]).unwrap();
            }
        }
    }

    #[test]
    fn ensure_that_tests_check_all_change_types() {
        assert_contains_all_variants! {
            all_change_types(Pubkey::default()) =>
                PositionChange::Price(_x)
                PositionChange::Flags(_x, _y)
                PositionChange::Register(_x)
                PositionChange::Close(_x)
                PositionChange::TokenChange(_x)
        }
    }

    macro_rules! assert_contains_all_variants {
        ($iterable:expr => $($type:ident::$var:ident $(($($_:ident),*))? )+ ) => {
            let mut index: HashMap<&str, usize> = HashMap::new();
            $(index.insert(stringify!($var), 1);)+
            for item in $iterable {
                match item {
                    $($type::$var $(($($_),*))? => index.insert(stringify!($var), 0)),+
                };
            }
            let sum: usize = index.values().sum();
            if sum > 0 {
                assert!(false);
            }
        };
    }
    use assert_contains_all_variants;
}
