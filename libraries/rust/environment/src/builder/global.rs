use std::collections::{HashMap, HashSet};

use glow_margin::{Permissions, Permit, TokenFeatures};
use glow_program_common::oracle::TokenPriceOracle;
use glow_solana_client::rpc::SolanaRpcExtra;
use solana_sdk::pubkey::Pubkey;

use glow_instructions::{
    airspace::{derive_governor_id, AirspaceIxBuilder},
    margin::{
        derive_adapter_config, derive_margin_permit, TokenAdmin, TokenConfigUpdate, TokenKind,
    },
    test_service::{self, derive_token_info, derive_token_mint, TokenCreateParams},
};

use super::{
    filter_initializers, margin::configure_margin_token, margin_pool, Builder, BuilderError,
    NetworkKind, SetupPhase, TokenContext,
};
use crate::config::{
    AirspaceConfig, CrankWithPermissions, EnvironmentConfig, OraclePriceConfig, TokenDescription,
    DEFAULT_MARGIN_ADAPTERS,
};

pub async fn configure_environment(
    builder: &mut Builder,
    config: &EnvironmentConfig,
) -> Result<(), BuilderError> {
    if builder.network != config.network {
        return Err(BuilderError::WrongNetwork {
            expected: config.network,
            actual: builder.network,
        });
    }

    let payer = builder.payer();
    // The airspace is allowed to be empty here because we are only configuring the governor
    let as_ix = AirspaceIxBuilder::new("", payer, builder.proposal_authority());

    // global authority accounts
    builder.setup(
        SetupPhase::TokenMints,
        filter_initializers(
            builder,
            [(derive_governor_id(), as_ix.create_governor_id())],
        )
        .await?,
    );
    let oracle_authority = config.oracle_authority.unwrap_or(payer);

    // airspaces
    for airspace in &config.airspaces {
        configure_airspace(builder, &oracle_authority, airspace).await?;
    }

    Ok(())
}

async fn configure_airspace(
    builder: &mut Builder,
    oracle_authority: &Pubkey,
    config: &AirspaceConfig,
) -> Result<(), BuilderError> {
    let as_ix = AirspaceIxBuilder::new(
        &config.name,
        builder.proposal_payer(),
        builder.proposal_authority(),
    );
    log::info!("Configuring airspace {}", config.name);

    if !builder.account_exists(&as_ix.address()).await? {
        log::info!("create airspace '{}' as {}", &config.name, as_ix.address());
        builder.propose(
            [as_ix.create(builder.proposal_authority(), config.is_restricted)],
            Some(format!("create airspace {}", config.name)),
        );
    }

    if builder.network != NetworkKind::Mainnet {
        create_test_tokens(builder, oracle_authority, &config.tokens).await?;
    }

    let mut new_margin_adapters = config
        .margin_adapters
        .iter()
        .copied()
        .collect::<HashSet<_>>();
    for adapter in DEFAULT_MARGIN_ADAPTERS {
        new_margin_adapters.insert(*adapter);
    }

    register_airspace_adapters(builder, &as_ix.address(), &new_margin_adapters).await?;

    configure_permits(builder, &as_ix.address(), &config.cranks).await?;

    configure_tokens(builder, &as_ix.address(), &config.tokens).await?;

    Ok(())
}

async fn register_airspace_adapters<'a>(
    builder: &mut Builder,
    airspace: &Pubkey,
    adapters: &HashSet<Pubkey>,
) -> Result<(), BuilderError> {
    builder.propose(
        filter_initializers(
            builder,
            adapters.iter().map(|addr| {
                (
                    derive_adapter_config(airspace, addr),
                    builder
                        .margin_config_ix(airspace)
                        .configure_adapter(*addr, true),
                )
            }),
        )
        .await?,
        Some(format!("Register airspace adapters for {airspace}")),
    );

    Ok(())
}

pub async fn configure_tokens<'a>(
    builder: &mut Builder,
    airspace: &Pubkey,
    tokens: impl IntoIterator<Item = &'a TokenDescription>,
) -> Result<(), BuilderError> {
    for desc in tokens {
        let token_context = token_context(builder.network, airspace, desc)?;

        // Set margin config for the token itself
        log::info!(
            "Configuring margin token {} {}",
            token_context.desc.name,
            token_context.mint
        );
        configure_margin_token(
            builder,
            airspace,
            &token_context.mint,
            &token_context.desc.name,
            Some(TokenConfigUpdate {
                underlying_mint: token_context.mint,
                underlying_mint_token_program: token_context.token_program,
                admin: TokenAdmin::Margin {
                    oracle: token_context.price_oracle,
                },
                token_kind: TokenKind::Collateral,
                value_modifier: desc.collateral_weight,
                max_staleness: desc.max_staleness,
                token_features: TokenFeatures::from_bits(desc.token_features).unwrap(),
            }),
        )
        .await?;

        // Create a pool if configured
        margin_pool::configure_for_token(builder, &token_context).await?;
    }

    Ok(())
}

pub fn token_context(
    network: NetworkKind,
    airspace: &Pubkey,
    desc: &TokenDescription,
) -> Result<TokenContext, BuilderError> {
    let (mint, price_config) = match network {
        NetworkKind::Localnet | NetworkKind::Devnet => {
            let mint = match desc.mint {
                Some(mint) => mint,
                None => derive_token_mint(&desc.name),
            };

            (
                mint,
                match desc.token_oracle {
                    OraclePriceConfig::NoOracle => TokenPriceOracle::NoOracle,
                    OraclePriceConfig::PythPull => TokenPriceOracle::PythPull {
                        feed_id: glow_program_common::oracle::get_feed_id_from_hex(
                            desc.pyth_feed_id.as_ref().unwrap(),
                        ),
                    },
                    OraclePriceConfig::PythPullRedemption => TokenPriceOracle::PythPullRedemption {
                        feed_id: glow_program_common::oracle::get_feed_id_from_hex(
                            desc.pyth_feed_id.as_ref().unwrap(),
                        ),
                        quote_feed_id: glow_program_common::oracle::get_feed_id_from_hex(
                            desc.pyth_redemption_feed_id.as_ref().unwrap(),
                        ),
                    },
                },
            )
        }

        NetworkKind::Mainnet => {
            let Some(mint) = desc.mint else {
                return Err(BuilderError::MissingMint(desc.name.clone()));
            };

            (
                mint,
                match desc.token_oracle {
                    OraclePriceConfig::NoOracle => TokenPriceOracle::NoOracle,
                    OraclePriceConfig::PythPull => TokenPriceOracle::PythPull {
                        feed_id: glow_program_common::oracle::get_feed_id_from_hex(
                            desc.pyth_feed_id.as_ref().unwrap(),
                        ),
                    },
                    OraclePriceConfig::PythPullRedemption => TokenPriceOracle::PythPullRedemption {
                        feed_id: glow_program_common::oracle::get_feed_id_from_hex(
                            desc.pyth_feed_id.as_ref().unwrap(),
                        ),
                        quote_feed_id: glow_program_common::oracle::get_feed_id_from_hex(
                            desc.pyth_redemption_feed_id.as_ref().unwrap(),
                        ),
                    },
                },
            )
        }
    };

    Ok(TokenContext {
        airspace: *airspace,
        desc: desc.clone(),
        mint,
        price_oracle: price_config,
        token_program: desc.token_program,
    })
}

pub async fn create_test_tokens<'a>(
    builder: &mut Builder,
    oracle_authority: &Pubkey,
    tokens: impl IntoIterator<Item = &'a TokenDescription>,
) -> Result<(), BuilderError> {
    let payer = builder.payer();

    let ixns = filter_initializers(
        builder,
        tokens
            .into_iter()
            // Skip tokens that already have mints set, as they are created elsewhere
            .filter(|desc| desc.mint.is_none())
            .map(|desc| match &*desc.symbol {
                // SOL is a bit of a special case, since we want to have an oracle for it but
                // the mint already exists
                "SOL" => {
                    let address = derive_token_info(&spl_token::native_mint::ID);
                    log::info!("register SOL token with token info {}", address);

                    Ok((
                        address,
                        test_service::token_init_native(&payer, oracle_authority),
                    ))
                }
                _ => {
                    let decimals = match desc.decimals {
                        Some(d) => d,
                        None => return Err(BuilderError::MissingDecimals(desc.name.clone())),
                    };
                    let amount_one = 10u64.pow(decimals.into());
                    let max_amount = desc
                        .max_test_amount
                        .map(|x| x * amount_one)
                        .unwrap_or(u64::MAX);

                    let token_mint = derive_token_mint(&desc.name);
                    log::info!(
                        "create token {} {} with faucet limit {}",
                        &desc.name,
                        token_mint,
                        max_amount
                    );

                    Ok((
                        token_mint,
                        test_service::token_create(
                            &payer,
                            &TokenCreateParams {
                                symbol: desc.symbol.clone(),
                                name: desc.name.clone(),
                                authority: builder.proposal_authority(),
                                oracle_authority: *oracle_authority,
                                source_symbol: desc.symbol.clone(),
                                price_ratio: 1.0,
                                decimals,
                                max_amount,
                                price_oracle: match desc.token_oracle {
                                    OraclePriceConfig::NoOracle => TokenPriceOracle::NoOracle,
                                    OraclePriceConfig::PythPull => TokenPriceOracle::PythPull {
                                        feed_id: glow_program_common::oracle::get_feed_id_from_hex(
                                            &desc.pyth_feed_id.clone().unwrap(),
                                        ),
                                    },
                                    OraclePriceConfig::PythPullRedemption => {
                                        TokenPriceOracle::PythPullRedemption {
                                            feed_id:
                                                glow_program_common::oracle::get_feed_id_from_hex(
                                                    &desc.pyth_feed_id.clone().unwrap(),
                                                ),
                                            quote_feed_id:
                                                glow_program_common::oracle::get_feed_id_from_hex(
                                                    &desc.pyth_redemption_feed_id.clone().unwrap(),
                                                ),
                                        }
                                    }
                                },
                            },
                            desc.token_program,
                        ),
                    ))
                }
            })
            .collect::<Result<Vec<_>, _>>()?,
    )
    .await?;

    builder.setup(SetupPhase::TokenMints, ixns);

    Ok(())
}

async fn configure_permits(
    builder: &mut Builder,
    airspace: &Pubkey,
    cranks: &[CrankWithPermissions],
) -> Result<(), BuilderError> {
    // Get all the existing permits and determine if we are:
    // - creating new ones
    // - amending existing ones
    // - revoking removed ones

    // TODO: we have a failure when getting permits in the sim, bypass for now. We are able to get them elsewhere.
    // This is fine because in localnet everything is permissionless.
    if builder.network == NetworkKind::Localnet {
        return Ok(());
    }
    let airspace_permits = builder.interface.find_anchor_accounts::<Permit>().await?;
    let mut airspace_permits = airspace_permits
        .into_iter()
        .filter(|(_, permit)| &permit.airspace == airspace)
        .collect::<HashMap<Pubkey, Permit>>();

    let new_cranks = cranks
        .iter()
        .map(|crank| {
            let permit_address = derive_margin_permit(airspace, &crank.address);
            (
                permit_address,
                (
                    crank.address,
                    Permissions::from_bits(crank.permissions).unwrap_or_default(),
                ),
            )
        })
        .collect::<HashMap<_, _>>();

    let margin_config_builder = builder.margin_config_ix(airspace);

    // New permissions
    for (permit_address, (owner, new_permissions)) in new_cranks {
        log::info!(
            "Checking permissions for crank {owner} with new permissions {:?}",
            new_permissions
        );
        // Find the crank and check its existing permissions
        let mut crank_ixs = vec![];
        let existing = airspace_permits.remove(&permit_address);
        match existing {
            None => {
                if new_permissions.contains(Permissions::LIQUIDATE) {
                    crank_ixs.push(margin_config_builder.configure_liquidator(owner, true));
                }
                if new_permissions.contains(Permissions::REFRESH_POSITION_CONFIG) {
                    crank_ixs.push(
                        margin_config_builder.configure_position_config_refresher(owner, true),
                    );
                }
            }
            Some(permit) => {
                if permit.permissions == new_permissions {
                    continue;
                }
                // TODO: rewrite with consice bit manipulation
                let may_liquidate = new_permissions.contains(Permissions::LIQUIDATE);
                if permit.permissions.contains(Permissions::LIQUIDATE) != may_liquidate {
                    crank_ixs
                        .push(margin_config_builder.configure_liquidator(owner, may_liquidate));
                }
                let may_refresh = new_permissions.contains(Permissions::REFRESH_POSITION_CONFIG);
                if permit
                    .permissions
                    .contains(Permissions::REFRESH_POSITION_CONFIG)
                    != may_refresh
                {
                    crank_ixs.push(
                        margin_config_builder
                            .configure_position_config_refresher(owner, may_refresh),
                    );
                }
            }
        }
        if !crank_ixs.is_empty() {
            builder.propose(
                crank_ixs,
                Some(format!(
                    "Configure permit and permissions for {owner} at {permit_address} {:?}",
                    new_permissions
                )),
            );
        }
    }

    // Remove permissions
    for (permit_address, permit) in airspace_permits {
        log::info!(
            "Removing permissions for {} at {permit_address}",
            permit.owner
        );
        let mut ixs = vec![];
        if permit.permissions.contains(Permissions::LIQUIDATE) {
            ixs.push(margin_config_builder.configure_liquidator(permit.owner, false));
        }
        if permit
            .permissions
            .contains(Permissions::REFRESH_POSITION_CONFIG)
        {
            ixs.push(
                margin_config_builder.configure_position_config_refresher(permit.owner, false),
            );
        }
        let len = ixs.len();
        if len > 0 {
            builder.propose(
                ixs,
                Some(format!(
                    "Removing {len} permissions for permit {permit_address} owned by {}",
                    permit.owner
                )),
            )
        }
    }

    Ok(())
}
