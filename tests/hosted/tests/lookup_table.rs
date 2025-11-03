use futures::future::join_all;
use glow_margin::AccountFeatureFlags;
use hosted_tests::margin_test_context;

/// Tests for lookup table, to check that it behaves fine on simulator and test envs
#[tokio::test(flavor = "current_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
async fn lookup_table() -> anyhow::Result<()> {
    use glow_margin_sdk::lookup_tables::LookupTable;
    use solana_sdk::pubkey::Pubkey;

    // Get the mocked runtime
    let ctx = margin_test_context!();

    for _ in [(); 3] {
        ctx.solana
            .context_mut()
            .await
            .warp_forward_force_reward_interval_end()?;
    }

    let table = LookupTable::create_lookup_table(&ctx.rpc(), None)
        .await
        .unwrap();
    const NUM_ADDRESSES: usize = 40;

    let accounts = &[Pubkey::new_unique(); NUM_ADDRESSES];

    LookupTable::extend_lookup_table(&ctx.rpc(), table, None, accounts)
        .await
        .unwrap();

    // Lookup table should not add duplicate accounts
    let result = LookupTable::extend_lookup_table(&ctx.rpc(), table, None, accounts).await;
    assert!(result.is_err());

    // The lookup table should have 40 accounts
    let table = LookupTable::get_lookup_table(&ctx.rpc(), &table)
        .await?
        .unwrap();
    assert_eq!(table.addresses.len(), NUM_ADDRESSES);

    Ok(())
}

/// Test that a user can create a lookup table registry for a margin account
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
async fn margin_lookup_table_registry() -> anyhow::Result<()> {
    use solana_sdk::signer::Signer;

    // Get the mocked runtime
    let ctx = margin_test_context!();

    let wallet = ctx.create_wallet(2).await?;
    ctx.issue_permit(wallet.pubkey()).await?;
    let user = ctx
        .margin_client()
        .user(&wallet, 0, glow_client::NetworkKind::Localnet)
        .created(AccountFeatureFlags::default())
        .await?;

    user.init_lookup_registry().await?;

    for _ in [(); 3] {
        ctx.solana
            .context_mut()
            .await
            .warp_forward_force_reward_interval_end()?;
    }

    // Create a lookup table in a registry
    let lookup_table = user.create_lookup_table().await?;

    // Add accounts to the lookup table
    let futures = (0..12).map(|_| async { ctx.generate_key().pubkey() });
    let addresses = join_all(futures).await;
    user.append_to_lookup_table(lookup_table, &addresses[..])
        .await?;

    Ok(())
}
