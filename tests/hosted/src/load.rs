// use anchor_lang::prelude::Pubkey;
// use anyhow::Result;
// use glow_margin_sdk::util::asynchronous::MapAsync;
// use serde_json::from_str;
// use shellexpand::tilde;
// use solana_sdk::{signature::Keypair, signer::Signer};
// use std::{fs::read_to_string, time::Duration};

// use crate::{
//     margin_test_context,
//     pricing::TokenPricer,
//     setup_helper::{create_tokens, create_users},
//     test_user::ONE,
// };

// pub struct UnhealthyAccountsLoadTestScenario {
//     pub airspace: String,
//     pub user_count: usize,
//     pub mint_count: usize,
//     pub repricing_delay: usize,
//     pub repricing_scale: f64,
//     pub keep_looping: bool,
//     pub liquidator: Option<Pubkey>,
// }

// impl Default for UnhealthyAccountsLoadTestScenario {
//     fn default() -> Self {
//         Self {
//             airspace: "default".into(),
//             user_count: 2,
//             mint_count: 2,
//             repricing_delay: 1,
//             repricing_scale: 0.999,
//             keep_looping: true,
//             liquidator: None,
//         }
//     }
// }

// pub fn load_default_keypair() -> anyhow::Result<Keypair> {
//     let keypair_data = read_to_string(tilde("~/.config/solana/id.json").to_string())?;
//     let keypair_bytes = from_str::<Vec<u8>>(&keypair_data)?;
//     Keypair::from_bytes(&keypair_bytes).map_err(Into::into)
// }

// pub async fn unhealthy_accounts_load_test(
//     scenario: UnhealthyAccountsLoadTestScenario,
// ) -> Result<(), anyhow::Error> {
//     let UnhealthyAccountsLoadTestScenario {
//         user_count,
//         mint_count,
//         repricing_delay,
//         repricing_scale,
//         keep_looping,
//         liquidator,
//         airspace,
//     } = scenario;
//     let ctx = margin_test_context!(&airspace);
//     let liquidator = liquidator.unwrap_or_else(|| load_default_keypair().unwrap().pubkey());
//     println!("authorizing liquidator: {liquidator}");
//     ctx.margin_client()
//         .set_liquidator_metadata(liquidator, true)
//         .await?;

//     let (mut mints, pricer) = create_tokens(&ctx, mint_count).await?;
//     let mut users = create_users(&ctx, user_count + 1).await?;
//     let big_depositor = users.pop().unwrap();
//     mints
//         .iter()
//         .map_async(|(mint, oracle)| big_depositor.deposit(*mint, 1000 * ONE))
//         .await?;

//     users
//         .iter()
//         .zip(mints.iter().cycle())
//         .map_async_chunked(16, |(user, (mint, oracle))| user.deposit(*mint, 100 * ONE))
//         .await?;
//     mints.rotate_right(mint_count / 2);
//     users
//         .iter()
//         .zip(mints.iter().cycle())
//         .map_async_chunked(32, |(user, (mint, oracle))| {
//             user.borrow_to_wallet(*mint, 80 * ONE)
//         })
//         .await?;

//     println!("incrementally lowering prices of half of the assets");
//     let assets_to_devalue = mints[0..mints.len() / 2]
//         .iter()
//         .map(|(mint_info, oracle)| mint_info.address)
//         .collect::<Vec<_>>();
//     devalue_assets(
//         1.0,
//         pricer,
//         assets_to_devalue,
//         vec![],
//         keep_looping,
//         repricing_scale,
//         repricing_delay,
//         &ctx.airspace_details.address,
//     )
//     .await
// }

// async fn devalue_assets(
//     starting_price: f64,
//     pricer: TokenPricer,
//     assets_to_devalue: Vec<Pubkey>,
//     assets_to_refresh: Vec<Pubkey>,
//     keep_looping: bool,
//     repricing_scale: f64,
//     repricing_delay: usize,
//     airspace: &Pubkey,
// ) -> anyhow::Result<()> {
//     println!("for assets {assets_to_devalue:?}...");
//     let mut price = starting_price;
//     loop {
//         price *= repricing_scale;
//         println!("setting price to {price}");
//         for _ in 0..repricing_delay {
//             pricer
//                 .set_prices(
//                     assets_to_devalue
//                         .iter()
//                         .map(|&a| (a, price))
//                         .chain(assets_to_refresh.iter().map(|&a| (a, 1.0)))
//                         .collect(),
//                     airspace,
//                     true,
//                 )
//                 .await?;
//             tokio::time::sleep(Duration::from_secs(1)).await;
//         }
//         if !keep_looping {
//             return Ok(());
//         }
//     }
// }
