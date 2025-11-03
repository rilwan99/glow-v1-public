use std::{sync::Arc, time::Duration};

use anchor_lang::{AccountDeserialize, AccountSerialize};
use anyhow::{bail, Result};
use glow_solana_client::{
    rpc::{AccountFilter, ClientError, ClientResult, SolanaRpc},
    transaction::{ToTransaction, TransactionBuilder},
};
use pyth_solana_receiver_sdk::price_update::PriceUpdateV2;
use solana_client::rpc_client::SerializableTransaction;
use solana_program_runtime::invoke_context::BuiltinFunctionWithContext;
use solana_program_test::{BanksClientError, ProgramTest, ProgramTestContext};
use solana_sdk::{
    account::{Account, AccountSharedData},
    clock::{Clock, Slot},
    commitment_config::{CommitmentConfig, CommitmentLevel},
    hash::Hash,
    instruction::Instruction,
    message::Message,
    native_token::LAMPORTS_PER_SOL,
    program_pack::Pack,
    pubkey::Pubkey,
    rent::Rent,
    signature::{Keypair, Signature},
    signer::Signer,
    system_program,
    transaction::{Transaction, VersionedTransaction},
};
use solana_test_framework::ClientExtensions;
use solana_transaction_status::TransactionStatus;
use spl_token::state::Account as TokenAccount;
use tokio::sync::{RwLock, RwLockWriteGuard};

use crate::{solana_rpc_api::SolanaRpcClient, DeterministicKeygen, Keygen};

#[doc(hidden)]
pub use solana_sdk::entrypoint::ProcessInstruction;

pub type Entrypoint = fn(*mut u8) -> u64;

#[macro_export]
macro_rules! anchor_processor {
    ($program:ident) => {{
        fn entry(
            program_id: &::solana_program::pubkey::Pubkey,
            accounts: &[::solana_program::account_info::AccountInfo],
            instruction_data: &[u8],
        ) -> ::solana_program::entrypoint::ProgramResult {
            let accounts = Box::leak(Box::new(accounts.to_vec()));

            $program::entry(program_id, accounts, instruction_data)
        }

        ::solana_program_test::processor!(entry)
    }};
}

#[derive(Clone)]
pub struct TestRuntimeRpcClient {
    pub context: Arc<RwLock<ProgramTestContext>>,
    pub user_keypair: Arc<Keypair>,
    pub keygen: Arc<DeterministicKeygen>,
}

impl TestRuntimeRpcClient {
    pub async fn new(programs: Vec<SolanaProgram>) -> Self {
        Self::new_with_accounts(programs, &[]).await
    }

    pub async fn new_with_accounts(
        programs: Vec<SolanaProgram>,
        accounts: &[(Pubkey, Account)],
    ) -> Self {
        let mut program = create_program_test(programs);
        let keygen = Arc::new(DeterministicKeygen::new("seed"));
        let user = keygen.generate_key();
        program.add_account(
            user.pubkey(),
            Account {
                lamports: 100_000 * LAMPORTS_PER_SOL,
                data: vec![],
                owner: system_program::ID,
                ..Account::default()
            },
        );
        for (address, account) in accounts {
            program.add_account(*address, account.clone());
        }

        let mut context = program.start_with_context().await;

        context.warp_to_slot(99).unwrap();

        Self {
            context: Arc::new(RwLock::new(context)),
            user_keypair: Arc::new(user),
            keygen,
        }
    }

    pub async fn instructions_to_tx(
        &self,
        instructions: &[Instruction],
        signers: &[&Keypair],
    ) -> Result<Transaction> {
        self.instructions_to_tx_impl(instructions, signers).await
    }

    async fn instructions_to_tx_impl(
        &self,
        instructions: &[Instruction],
        signers: &[&Keypair],
    ) -> Result<Transaction> {
        let recent_blockhash = self
            .context_mut()
            .await
            .banks_client
            .get_latest_blockhash()
            .await?;
        let payer = self.payer();

        let mut signers_vec = vec![payer];
        signers_vec.extend_from_slice(signers);

        let message = Message::new(instructions, Some(&self.payer().pubkey()));
        Ok(Transaction::new(&signers_vec, message, recent_blockhash))
    }

    pub async fn run_instructions(
        &self,
        instructions: &[Instruction],
        signers: &[&Keypair],
    ) -> Result<()> {
        let transaction = self.instructions_to_tx(instructions, signers).await?;
        self.run_transaction(transaction).await?;

        Ok(())
    }

    pub async fn run_transaction(&self, transaction: Transaction) -> Result<()> {
        print!("  Running Transaction... ");

        let mut ctx = tarpc::context::current();
        ctx.deadline += Duration::from_secs(10);

        let transaction_result = self
            .context_mut()
            .await
            .banks_client
            .process_transaction_with_preflight_and_commitment_and_context(
                ctx,
                transaction,
                CommitmentLevel::default(),
            )
            .await?;

        if let Some(Err(e)) = transaction_result.result {
            println!("failed :(");
            if let Some(details) = &transaction_result.simulation_details {
                for msg in &details.logs {
                    println!("    {}", msg);
                }
            }
            Err(e.into())
        } else {
            println!("success!");
            Ok(())
        }
    }

    pub async fn rent_exemption_amount(&mut self, size: usize) -> Result<u64> {
        Ok(self
            .context_mut()
            .await
            .banks_client
            .get_rent()
            .await?
            .minimum_balance(size))
    }

    pub async fn create_wallet(&self, sol: u64) -> Result<Keypair> {
        let keypair = self.generate_key();
        let mut context = self.context_mut().await;

        context
            .banks_client
            .create_account(
                &self.user_keypair,
                &keypair,
                sol * LAMPORTS_PER_SOL,
                0,
                system_program::ID,
            )
            .await
            .unwrap();

        Ok(keypair)
    }

    pub async fn get_account(&self, pubkey: Pubkey) -> Result<Option<Account>> {
        let mut context = self.context_mut().await;
        Ok(context.banks_client.get_account(pubkey).await?)
    }

    pub fn rpc(&self) -> Arc<dyn SolanaRpcClient> {
        Arc::new(self.clone())
    }

    pub fn rpc2(&self) -> Arc<dyn SolanaRpc> {
        Arc::new(self.clone())
    }

    pub fn payer(&self) -> &Keypair {
        &self.user_keypair
    }

    pub async fn context_mut(&self) -> RwLockWriteGuard<ProgramTestContext> {
        self.context.as_ref().write().await
    }

    pub async fn create_transaction(
        &self,
        ixs: &[Instruction],
        payer: &Keypair,
        signers: Vec<&Keypair>,
    ) -> Result<Transaction> {
        let mut context = self.context_mut().await;
        Ok(context
            .banks_client
            .transaction_from_instructions(ixs, payer, signers)
            .await
            .unwrap())
    }

    pub async fn send_and_confirm(
        &self,
        transaction: impl Into<VersionedTransaction>,
    ) -> Result<()> {
        let mut context = self.context_mut().await;
        Ok(context
            .banks_client
            .process_transaction_with_commitment(transaction, CommitmentLevel::Confirmed)
            .await?)
    }

    /// Compile a [TransactionBuilder] and process it
    pub async fn complie_and_send(&self, builder: TransactionBuilder) -> Result<()> {
        let mut context = self.context_mut().await;
        let blockhash = context.banks_client.get_latest_blockhash().await?;
        let transaction = builder.to_transaction(&self.payer().pubkey(), blockhash);
        Ok(context
            .banks_client
            .process_transaction(transaction)
            .await?)
    }

    pub fn generate_key(&self) -> Keypair {
        self.keygen.generate_key()
    }

    async fn get_signature_statuses_(
        &self,
        signatures: Vec<Signature>,
    ) -> Result<Vec<Option<TransactionStatus>>> {
        let mut context = self.context_mut().await;
        let statuses = context
            .banks_client
            .get_transaction_statuses(signatures.clone())
            .await?
            .into_iter()
            .map(|s| {
                s.map(|s| TransactionStatus {
                    slot: s.slot,
                    confirmations: s.confirmations,
                    status: Ok(()),
                    err: s.err.clone(),
                    confirmation_status: s.confirmation_status.map(|status| match status {
                        solana_banks_interface::TransactionConfirmationStatus::Finalized => {
                            solana_transaction_status::TransactionConfirmationStatus::Finalized
                        }
                        solana_banks_interface::TransactionConfirmationStatus::Confirmed => {
                            solana_transaction_status::TransactionConfirmationStatus::Confirmed
                        }
                        solana_banks_interface::TransactionConfirmationStatus::Processed => {
                            solana_transaction_status::TransactionConfirmationStatus::Processed
                        }
                    }),
                })
            })
            .collect::<Vec<_>>();

        Ok(statuses)
    }

    /// Add an account to the test environment
    pub async fn add_account(&mut self, address: Pubkey, account: Account) {
        let mut context = self.context_mut().await;
        context.set_account(&address, &AccountSharedData::from(account));
    }

    pub async fn add_account_with_data(
        &mut self,
        pubkey: Pubkey,
        owner: Pubkey,
        data: &[u8],
        executable: bool,
    ) {
        self.add_account(
            pubkey,
            Account {
                lamports: Rent::default().minimum_balance(data.len()),
                data: data.to_vec(),
                executable,
                owner,
                rent_epoch: 0,
            },
        )
        .await;
    }

    pub async fn add_pyth_pull_oracle(
        &mut self,
        oracle: Pubkey,
        program_id: Pubkey,
        price_update: PriceUpdateV2,
    ) -> Result<(), BanksClientError> {
        let mut data = vec![];
        price_update.try_serialize(&mut data).unwrap();
        self.add_account_with_data(oracle, program_id, &data, false)
            .await;

        Ok(())
    }

    pub async fn get_pyth_price_account(
        &mut self,
        address: Pubkey,
    ) -> Result<PriceUpdateV2, Box<dyn std::error::Error>> {
        let account = self.get_account(address).await?.unwrap();

        let price_account = PriceUpdateV2::try_deserialize(&mut &account.data[..])
            .map_err(|_| BanksClientError::ClientError("Failed to deserialize price account"))?;
        Ok(price_account)
    }
}

#[async_trait::async_trait]
impl SolanaRpcClient for TestRuntimeRpcClient {
    fn as_any(&self) -> &dyn std::any::Any {
        self as &dyn std::any::Any
    }

    fn clone_with_payer(&self, payer: Keypair) -> Box<dyn SolanaRpcClient> {
        Box::new(Self {
            context: self.context.clone(),
            user_keypair: Arc::new(payer),
            keygen: self.keygen.clone(),
        })
    }
    async fn get_account(&self, address: &Pubkey) -> Result<Option<Account>> {
        Ok(self.get_account(*address).await?)
    }
    async fn get_token_balance(&self, address: &Pubkey) -> Result<Option<u64>> {
        if let Some(account) = self.get_account(*address).await? {
            // NOTE: if this throws errors about invalid data, then we have to hack around the balance
            Ok(Some(
                spl_token::state::Account::unpack(&account.data)?.amount,
            ))
        } else {
            Ok(None)
        }
    }

    async fn get_multiple_accounts(&self, pubkeys: &[Pubkey]) -> Result<Vec<Option<Account>>> {
        let mut accounts = Vec::with_capacity(pubkeys.len());
        for pubkey in pubkeys {
            accounts.push(self.get_account(*pubkey).await?);
        }
        Ok(accounts)
    }
    async fn get_genesis_hash(&self) -> Result<Hash> {
        Ok(self.context_mut().await.last_blockhash)
    }
    async fn get_latest_blockhash(&self) -> Result<Hash> {
        Ok(self
            .context_mut()
            .await
            .banks_client
            .get_latest_blockhash()
            .await?)
    }
    async fn get_minimum_balance_for_rent_exemption(&self, length: usize) -> Result<u64> {
        let mut context = self.context_mut().await;
        Ok(context
            .banks_client
            .get_rent()
            .await?
            .minimum_balance(length))
    }
    async fn send_transaction(&self, transaction: VersionedTransaction) -> Result<Signature> {
        let signature = *transaction.get_signature();
        self.context_mut()
            .await
            .banks_client
            .process_transaction(transaction)
            .await?;
        Ok(signature)
    }
    // async fn send_versioned_transaction(
    //     &self,
    //     transaction: &VersionedTransaction,
    // ) -> Result<Signature> {
    //     let signature = transaction.get_signature().clone();
    //     self.bank_mut().await.send_transaction(transaction.clone()).await?;
    //     Ok(signature)
    // }
    async fn get_signature_statuses(
        &self,
        signatures: &[Signature],
    ) -> Result<Vec<Option<solana_transaction_status::TransactionStatus>>> {
        let mut statuses = Vec::with_capacity(signatures.len());
        for signature in signatures {
            let status = self
                .context_mut()
                .await
                .banks_client
                .get_transaction_status(*signature)
                .await
                .map(|opt| {
                    opt.map(|_| TransactionStatus {
                        slot: 0,
                        confirmations: None,
                        status: Ok(()),
                        err: None,
                        confirmation_status: None,
                    })
                })
                .unwrap();

            statuses.push(status);
        }

        Ok(statuses)
    }

    async fn get_program_accounts(
        &self,
        program_id: &Pubkey,
        filters: Vec<AccountFilter>,
    ) -> Result<Vec<(Pubkey, Account)>> {
        self.rpc().get_program_accounts(program_id, filters).await
    }

    async fn airdrop(&self, account: &Pubkey, amount: u64) -> Result<()> {
        let ix =
            solana_program::system_instruction::transfer(&self.payer().pubkey(), account, amount);
        let tx = self
            .create_transaction(&[ix], self.payer(), vec![self.payer()])
            .await?;
        self.send_and_confirm(tx).await?;

        Ok(())
    }

    async fn confirm_transactions(
        &self,
        signatures: &[Signature],
    ) -> Result<Vec<TransactionStatus>> {
        let signatures = signatures.to_vec();
        for _ in 0..120 {
            let statuses = self
                .get_signature_statuses_(signatures.clone())
                .await
                .unwrap();

            if statuses.iter().all(|s| s.is_some()) {
                return Ok(statuses.into_iter().map(|s| s.unwrap()).collect());
            }

            // come back later
            tokio::time::sleep(Duration::from_millis(250)).await;
        }

        bail!("Could not confirm all transactions")
    }

    async fn get_slot(&self, _commitment_config: Option<CommitmentConfig>) -> Result<Slot> {
        Ok(self
            .context_mut()
            .await
            .banks_client
            .get_root_slot()
            .await?)
    }

    async fn get_clock(&self) -> Result<Clock> {
        let clock: Clock = self.context_mut().await.banks_client.get_sysvar().await?;
        Ok(clock)
    }
    async fn set_clock(&self, new_clock: Clock) -> Result<()> {
        self.context_mut().await.set_sysvar(&new_clock);
        self.context_mut()
            .await
            .warp_forward_force_reward_interval_end()?;
        Ok(())
    }
    async fn wait_for_next_block(&self) -> Result<()> {
        let latest_blockhash = SolanaRpc::get_latest_blockhash(self).await?;
        let mut new_blockhash = SolanaRpc::get_latest_blockhash(self).await?;
        while latest_blockhash == new_blockhash {
            tokio::time::sleep(Duration::from_millis(100)).await;
            new_blockhash = SolanaRpc::get_latest_blockhash(self).await?;
        }
        Ok(())
    }

    fn payer(&self) -> &Keypair {
        &self.user_keypair
    }
}

#[async_trait::async_trait]
impl SolanaRpc for TestRuntimeRpcClient {
    async fn get_genesis_hash(&self) -> ClientResult<Hash> {
        Ok(self.context_mut().await.last_blockhash)
    }
    async fn get_latest_blockhash(&self) -> ClientResult<Hash> {
        Ok(self
            .context_mut()
            .await
            .banks_client
            .get_latest_blockhash()
            .await
            .unwrap())
    }
    async fn get_slot(&self) -> ClientResult<u64> {
        Ok(self
            .context_mut()
            .await
            .banks_client
            .get_root_slot()
            .await
            .unwrap())
    }
    async fn get_block_time(&self, slot: u64) -> ClientResult<i64> {
        // For test runtime, we can approximate block time based on slot
        // Assuming standard slot duration of 400ms
        Ok((slot as i64) * 400)
    }

    async fn get_multiple_accounts(
        &self,
        pubkeys: &[Pubkey],
    ) -> ClientResult<Vec<Option<Account>>> {
        let mut accounts = Vec::with_capacity(pubkeys.len());
        for pubkey in pubkeys {
            accounts.push(self.get_account(*pubkey).await.unwrap())
        }
        Ok(accounts)
    }

    async fn get_signature_statuses(
        &self,
        signatures: &[Signature],
    ) -> ClientResult<Vec<Option<TransactionStatus>>> {
        let signatures = signatures.to_vec();
        Ok(self.get_signature_statuses_(signatures).await.unwrap())
    }

    async fn airdrop(&self, _account: &Pubkey, _lamports: u64) -> ClientResult<()> {
        let ix = solana_program::system_instruction::transfer(
            &self.payer().pubkey(),
            _account,
            _lamports,
        );
        let tx = self
            .create_transaction(&[ix], self.payer(), vec![self.payer()])
            .await
            .unwrap();
        self.send_and_confirm(tx).await.unwrap();

        Ok(())
    }

    async fn send_transaction_legacy(&self, transaction: &Transaction) -> ClientResult<Signature> {
        let versioned_transaction = VersionedTransaction::from(transaction.clone());
        SolanaRpc::send_transaction(self, &versioned_transaction).await
    }

    async fn send_transaction(
        &self,
        transaction: &VersionedTransaction,
    ) -> ClientResult<Signature> {
        let signature = *transaction.get_signature();
        self.context_mut()
            .await
            .banks_client
            .process_transaction(transaction.clone())
            .await
            .map_err(banks_client_error_to_client_error)
            .map(|_| signature)
    }

    async fn get_program_accounts(
        &self,
        program: &Pubkey,
        filters: &[AccountFilter],
    ) -> ClientResult<Vec<(Pubkey, Account)>> {
        self.rpc()
            .get_program_accounts(program, filters.to_vec())
            .await
            .map_err(|_| {
                banks_client_error_to_client_error(BanksClientError::ClientError(
                    "Get Program Accounts Error",
                ))
            })
    }

    async fn get_token_accounts_by_owner(
        &self,
        owner: &Pubkey,
    ) -> ClientResult<Vec<(Pubkey, TokenAccount)>> {
        SolanaRpc::get_token_accounts_by_owner(self, owner).await
    }
}

#[inline]
fn banks_client_error_to_client_error(error: BanksClientError) -> ClientError {
    match error {
        BanksClientError::ClientError(e) => ClientError::Other(e.to_string()),
        BanksClientError::Io(e) => ClientError::Other(e.to_string()),
        BanksClientError::RpcError(e) => ClientError::Other(e.to_string()),
        BanksClientError::TransactionError(e) => ClientError::TransactionError(e),
        BanksClientError::SimulationError { err, logs, .. } => {
            ClientError::TransactionSimulationError {
                err: Some(err),
                logs,
            }
        }
    }
}

#[macro_export]
macro_rules! create_test_runtime {
    [$($program:tt),+$(,)?] => {{
        let mut programs = vec![];
        $(programs.push($crate::program!($program));)+
        $crate::TestRuntime::new(programs, [])
    }}
}

#[macro_export]
macro_rules! program {
    ($krate:ident) => {{
        (
            $krate::id(),
            $krate::entry as $crate::runtime::ProcessInstruction,
        )
    }};
    (($id:expr, $processor:path)) => {{
        ($id, $processor)
    }};
}

pub struct SolanaProgram {
    pub program_name: String,
    pub program_id: Pubkey,
    pub builtin_function: Option<BuiltinFunctionWithContext>,
}

fn create_program_test(programs: Vec<SolanaProgram>) -> ProgramTest {
    let mut program = ProgramTest::default();
    for SolanaProgram {
        program_name,
        program_id,
        builtin_function,
    } in programs
    {
        program.add_program(&program_name, program_id, builtin_function);
    }

    program
}
#[cfg(test)]
mod test {

    #[tokio::test]
    async fn can_simulate_simple_transfer() {
        // let rt = TestRuntime::new([], []);
        // let rpc = rt.rpc();

        // let source_wallet = Keypair::new();
        // let dest_wallet = Keypair::new();

        // SolanaRpc::airdrop(&rpc, &source_wallet.pubkey(), 421 * LAMPORTS_PER_SOL)
        //     .await
        //     .unwrap();

        // let recent_blockhash = SolanaRpc::get_latest_blockhash(&rpc).await.unwrap();
        // let transfer_tx = system_transaction::transfer(
        //     &source_wallet,
        //     &dest_wallet.pubkey(),
        //     420 * LAMPORTS_PER_SOL,
        //     recent_blockhash,
        // );

        // SolanaRpc::send_transaction_legacy(&rpc, &transfer_tx)
        //     .await
        //     .unwrap();

        // let dest_balance = SolanaRpc::get_account(&rpc, &dest_wallet.pubkey())
        //     .await
        //     .unwrap()
        //     .unwrap()
        //     .lamports;

        // assert_eq!(420 * LAMPORTS_PER_SOL, dest_balance);
    }
}
