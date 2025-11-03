use anchor_lang::error::ErrorCode;
use glow_airspace::{
    state::{Airspace, AirspacePermit, AirspacePermitIssuerId},
    AirspaceErrorCode,
};
use glow_instructions::airspace::{derive_governor_id, AirspaceIxBuilder};
use glow_margin_sdk::{
    get_state::get_anchor_account,
    {solana::transaction::TransactionBuilderExt, solana::transaction::WithSigner},
};
use glow_simulation::assert_custom_program_error;
use solana_sdk::signature::Signer;

use hosted_tests::margin_test_context;

/// Test comprehensive airspace functionality
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
async fn test_airspace_complete_workflow() -> Result<(), anyhow::Error> {
    let ctx = margin_test_context!("airspace_complete_workflow");

    // Generate test accounts
    let user = ctx.generate_key();
    let permit_issuer = ctx.generate_key();
    let new_permit_issuer = ctx.generate_key();
    let airspace_seed = "test-airspace";
    let airspace_authority = ctx.airspace_authority.pubkey();

    // 0. Governor account should already be set up in the test context
    let governor_id_address = derive_governor_id();
    let governor_account = ctx.rpc().get_account(&governor_id_address).await?;
    assert!(governor_account.is_some());

    // 1. Create an airspace
    let airspace_ix =
        AirspaceIxBuilder::new(airspace_seed, ctx.payer().pubkey(), airspace_authority);
    let airspace_address = airspace_ix.address();
    let airspace_account_before = ctx.rpc().get_account(&airspace_address).await?;
    assert!(
        airspace_account_before.is_none(),
        "Airspace should not exist initially"
    );

    airspace_ix
        .create(airspace_authority, true) // Create restricted airspace
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await?;

    let airspace: Airspace = get_anchor_account(&ctx.rpc(), &airspace_address).await?;
    assert_eq!(airspace.authority, airspace_authority);
    assert!(airspace.is_restricted);

    // 2. Create a permit issuer
    let issuer_id_address = airspace_ix.derive_issuer_id(&permit_issuer.pubkey());
    let issuer_account_before = ctx.rpc().get_account(&issuer_id_address).await?;
    assert!(
        issuer_account_before.is_none(),
        "Permit issuer should not exist initially"
    );

    airspace_ix
        .permit_issuer_create(permit_issuer.pubkey())
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await?;

    let issuer_id: AirspacePermitIssuerId =
        get_anchor_account(&ctx.rpc(), &issuer_id_address).await?;
    assert_eq!(issuer_id.airspace, airspace_address);
    assert_eq!(issuer_id.issuer, permit_issuer.pubkey());

    // 3. Create a permit
    let permit_address = airspace_ix.derive_permit(&user.pubkey());
    let permit_account_before = ctx.rpc().get_account(&permit_address).await?;
    assert!(
        permit_account_before.is_none(),
        "Permit should not exist initially"
    );

    let permit_ix_builder =
        AirspaceIxBuilder::new(airspace_seed, ctx.payer().pubkey(), permit_issuer.pubkey());
    permit_ix_builder
        .permit_create(user.pubkey())
        .with_signer(&permit_issuer)
        .send_and_confirm(&ctx.rpc())
        .await?;

    let permit: AirspacePermit = get_anchor_account(&ctx.rpc(), &permit_address).await?;
    assert_eq!(permit.airspace, airspace_address);
    assert_eq!(permit.owner, user.pubkey());
    assert_eq!(permit.issuer, permit_issuer.pubkey());

    // 4. Verify correct account setup
    let airspace: Airspace = get_anchor_account(&ctx.rpc(), &airspace_address).await?;
    let issuer: AirspacePermitIssuerId = get_anchor_account(&ctx.rpc(), &issuer_id_address).await?;
    let permit: AirspacePermit = get_anchor_account(&ctx.rpc(), &permit_address).await?;
    assert_eq!(airspace.authority, airspace_authority);
    assert!(airspace.is_restricted);
    assert_eq!(issuer.issuer, permit_issuer.pubkey());
    assert_eq!(permit.owner, user.pubkey());

    // 5. Update the permit issuer by creating a new one and revoking the old
    let new_issuer_id_address = airspace_ix.derive_issuer_id(&new_permit_issuer.pubkey());

    airspace_ix
        .permit_issuer_create(new_permit_issuer.pubkey())
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await?;

    let new_issuer_id: AirspacePermitIssuerId =
        get_anchor_account(&ctx.rpc(), &new_issuer_id_address).await?;
    assert_eq!(new_issuer_id.airspace, airspace_address);
    assert_eq!(new_issuer_id.issuer, new_permit_issuer.pubkey());

    let issuer_account_exists = ctx.rpc().get_account(&new_issuer_id_address).await?;
    assert!(
        issuer_account_exists.is_some(),
        "New permit issuer should exist"
    );

    // 6. Revoke the permit and permit issuer
    airspace_ix
        .permit_revoke(user.pubkey(), permit_issuer.pubkey())
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await?;

    let permit_after_revoke = ctx.rpc().get_account(&permit_address).await?;
    assert!(
        permit_after_revoke.is_none(),
        "Permit should not exist after revocation"
    );

    airspace_ix
        .permit_issuer_revoke(permit_issuer.pubkey())
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await?;

    let issuer_after_revoke = ctx.rpc().get_account(&issuer_id_address).await?;
    assert!(
        issuer_after_revoke.is_none(),
        "Permit issuer should not exist after revocation"
    );

    // 7. Add a new permit issuer & permit (using the new issuer we created earlier)
    let new_user = ctx.generate_key();
    let new_permit_address = airspace_ix.derive_permit(&new_user.pubkey());

    let new_permit_ix_builder = AirspaceIxBuilder::new(
        airspace_seed,
        ctx.payer().pubkey(),
        new_permit_issuer.pubkey(),
    );
    new_permit_ix_builder
        .permit_create(new_user.pubkey())
        .with_signer(&new_permit_issuer)
        .send_and_confirm(&ctx.rpc())
        .await?;

    let new_permit: AirspacePermit = get_anchor_account(&ctx.rpc(), &new_permit_address).await?;
    assert_eq!(new_permit.airspace, airspace_address);
    assert_eq!(new_permit.owner, new_user.pubkey());
    assert_eq!(new_permit.issuer, new_permit_issuer.pubkey());

    // 8. Revoke the permit issuer, allowing anyone to revoke the permit
    let permit_issuer_revoke_tx =
        AirspaceIxBuilder::new(airspace_seed, ctx.payer().pubkey(), airspace_authority);
    permit_issuer_revoke_tx
        .permit_issuer_revoke(new_permit_issuer.pubkey())
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await?;

    let issuer_account_exists = ctx.rpc().get_account(&new_issuer_id_address).await?;
    assert!(
        issuer_account_exists.is_none(),
        "New permit issuer is now revoked"
    );

    let revoke_user = ctx.generate_key();
    let permit_revoke_ix =
        AirspaceIxBuilder::new(airspace_seed, ctx.payer().pubkey(), revoke_user.pubkey());
    permit_revoke_ix
        .permit_revoke(new_user.pubkey(), new_permit_issuer.pubkey())
        .with_signer(&revoke_user) // Not airspace authority but a random user
        .send_and_confirm(&ctx.rpc())
        .await?;

    // This is the permit we created above in the previous step
    let permit_after_revoke = ctx.rpc().get_account(&new_permit_address).await?;
    assert!(
        permit_after_revoke.is_none(),
        "Permit should not exist after revocation"
    );

    // 9. Verify that the open revocation works: permit should be gone
    let revoked_permit_check = ctx.rpc().get_account(&new_permit_address).await?;
    assert!(
        revoked_permit_check.is_none(),
        "New permit should be revoked"
    );

    // Verify the airspace still exists and has correct data
    let final_airspace: Airspace = get_anchor_account(&ctx.rpc(), &airspace_address).await?;
    assert_eq!(final_airspace.authority, airspace_authority);
    assert!(final_airspace.is_restricted);

    // Verify the old accounts are also gone
    let old_permit_check = ctx.rpc().get_account(&permit_address).await?;
    let old_issuer_check = ctx.rpc().get_account(&issuer_id_address).await?;
    assert!(old_permit_check.is_none());
    assert!(old_issuer_check.is_none());

    Ok(())
}

/// Test error cases and edge conditions
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
async fn test_airspace_error_conditions() -> Result<(), anyhow::Error> {
    let ctx = margin_test_context!("airspace_error_conditions");

    let user = ctx.generate_key();
    let permit_issuer = ctx.generate_key();
    let airspace_seed = "test-error-airspace";
    let airspace_authority = ctx.airspace_authority.pubkey();

    let governor_id_address = derive_governor_id();
    let governor_account = ctx.rpc().get_account(&governor_id_address).await?;
    assert!(governor_account.is_some());

    let airspace_ix =
        AirspaceIxBuilder::new(airspace_seed, ctx.payer().pubkey(), airspace_authority);
    airspace_ix
        .create(airspace_authority, true)
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await?;

    // Create a permit issuer for the airspace authority
    airspace_ix
        .permit_issuer_create(airspace_authority)
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await?;

    let result = airspace_ix
        .permit_create(user.pubkey())
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await;
    assert!(result.is_ok());

    // Test 1: Try to create permit as an unauthorised issuer
    let unauthorised_issuer = ctx.generate_key();
    let unauthorised_ix_builder = AirspaceIxBuilder::new(
        airspace_seed,
        ctx.payer().pubkey(),
        unauthorised_issuer.pubkey(),
    );
    let result = unauthorised_ix_builder
        .permit_create(ctx.generate_key().pubkey())
        .with_signer(&unauthorised_issuer)
        .send_and_confirm(&ctx.rpc())
        .await;
    assert_custom_program_error(AirspaceErrorCode::PermissionDenied, result);

    // Test 2: Try to revoke permit issuer with unauthorised authority
    let permit_issuer_ix_builder =
        AirspaceIxBuilder::new(airspace_seed, ctx.payer().pubkey(), airspace_authority);
    let result = permit_issuer_ix_builder
        .permit_issuer_create(permit_issuer.pubkey())
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await;
    assert!(result.is_ok());

    let wrong_authority = ctx.generate_key();
    let wrong_ix_builder = AirspaceIxBuilder::new(
        airspace_seed,
        ctx.payer().pubkey(),
        wrong_authority.pubkey(),
    );

    let result = wrong_ix_builder
        .permit_issuer_revoke(permit_issuer.pubkey())
        .with_signer(&wrong_authority)
        .send_and_confirm(&ctx.rpc())
        .await;
    assert_custom_program_error(ErrorCode::ConstraintHasOne, result);

    // Test 3: Try to revoke permit with unauthorised authority
    let permit_revoke_ix = AirspaceIxBuilder::new(
        airspace_seed,
        ctx.payer().pubkey(),
        wrong_authority.pubkey(),
    );
    let result = permit_revoke_ix
        .permit_revoke(user.pubkey(), permit_issuer.pubkey())
        .with_signer(&wrong_authority) // Not airspace authority but a random user
        .send_and_confirm(&ctx.rpc())
        .await;
    assert_custom_program_error(ErrorCode::ConstraintSeeds, result);

    Ok(())
}

/// Test unrestricted airspace functionality
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
async fn test_unrestricted_airspace() -> Result<(), anyhow::Error> {
    let ctx = margin_test_context!("unrestricted_airspace");

    let user = ctx.generate_key();
    let airspace_seed = "test-unrestricted";
    let airspace_authority = ctx.airspace_authority.pubkey();

    let governor_id_address = derive_governor_id();
    let governor_account = ctx.rpc().get_account(&governor_id_address).await?;
    assert!(governor_account.is_some());

    // Create unrestricted airspace
    let airspace_ix =
        AirspaceIxBuilder::new(airspace_seed, ctx.payer().pubkey(), airspace_authority);
    airspace_ix
        .create(airspace_authority, false)
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await?;

    let airspace: Airspace = get_anchor_account(&ctx.rpc(), &airspace_ix.address()).await?;
    assert!(!airspace.is_restricted);

    // In unrestricted airspace, anyone should be able to create permits
    let any_issuer = ctx.generate_key();
    let any_ix_builder =
        AirspaceIxBuilder::new(airspace_seed, ctx.payer().pubkey(), any_issuer.pubkey());
    any_ix_builder
        .permit_create(user.pubkey())
        .with_signer(&any_issuer)
        .send_and_confirm(&ctx.rpc())
        .await?;

    let permit: AirspacePermit =
        get_anchor_account(&ctx.rpc(), &airspace_ix.derive_permit(&user.pubkey())).await?;
    assert_eq!(permit.owner, user.pubkey());
    assert_eq!(permit.issuer, any_issuer.pubkey());

    Ok(())
}
