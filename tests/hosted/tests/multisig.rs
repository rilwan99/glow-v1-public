use hosted_tests::{adapters::squads::TestSquad, margin_test_context};
use solana_sdk::{pubkey::Pubkey, signature::Signer};

/// Tests for lookup table, to check that it behaves fine on simulator and test envs
#[tokio::test(flavor = "current_thread")]
async fn multisig_tests() -> anyhow::Result<()> {
    let (initializer, test_squad) = TestSquad::initializer()?;
    let ctx = margin_test_context!("multisig", &initializer.accounts);

    let keypair = ctx.create_wallet(10).await?;

    let mut multisig = test_squad
        .create_multisig(&ctx, &keypair, &[keypair.pubkey()])
        .await?;

    assert_eq!(multisig.creator, keypair.pubkey());

    // Should be able to create a proposal
    let (transaction, proposal) = multisig
        .create_transaction(&ctx.rpc(), &keypair, vec![], 0)
        .await?;
    assert_ne!(proposal, Pubkey::default());
    assert_ne!(transaction, Pubkey::default());

    Ok(())
}
