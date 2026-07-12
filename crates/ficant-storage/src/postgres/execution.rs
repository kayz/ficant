use async_trait::async_trait;
use ficant_application::ApplicationError;
use ficant_application::ports::{Phase1AtomicWork, TransactionRunner};

use super::PostgresRepository;
use super::artifacts::persist_artifact;
use super::common::{IdempotencyOutcome, lock_idempotency, map_sqlx_error};
use super::facts::{persist_market_fact, validate_market_fact_rule, validate_market_fact_units};
use super::journal::persist_journal;
use super::runs::{persist_run, persist_transition, validate_run_rule};
use super::signals::persist_signal;
use super::snapshots::persist_snapshot;

#[async_trait]
impl TransactionRunner for PostgresRepository {
    async fn commit_phase1(&self, work: &Phase1AtomicWork) -> Result<(), ApplicationError> {
        let scope = work.run().scope();
        let owner = work.run().target_owner();
        scope.authorize(owner)?;
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        validate_market_fact_units(&mut transaction, work.fact().fact(), work.fact().proof())
            .await?;
        validate_market_fact_rule(
            &mut transaction,
            work.fact().fact(),
            work.fact().rule_proof(),
        )
        .await?;
        persist_market_fact(
            &mut transaction,
            work.fact().fact(),
            work.fact().idempotency_key().as_str(),
            work.fact().fingerprint().content_hash().as_bytes(),
        )
        .await?;
        for snapshot in work.snapshots() {
            persist_snapshot(&mut transaction, snapshot).await?;
        }
        validate_run_rule(&mut transaction, work.run().run(), work.run().proof()).await?;
        let outcome = lock_idempotency(
            &mut transaction,
            owner.tenant_id().as_str(),
            "phase1:atomic:v2",
            work.idempotency_key().as_str(),
            work.fingerprint().content_hash().as_bytes(),
            work.run().run().id().as_str(),
        )
        .await?;
        if outcome == IdempotencyOutcome::Replay {
            transaction.commit().await.map_err(map_sqlx_error)?;
            return Ok(());
        }
        persist_run(&mut transaction, work.run()).await?;
        for transition in work.transitions() {
            persist_transition(&mut transaction, transition).await?;
        }
        persist_artifact(&mut transaction, work.artifact()).await?;
        persist_signal(&mut transaction, work.signal()).await?;
        for journal in work.journal() {
            persist_journal(&mut transaction, journal).await?;
        }
        transaction.commit().await.map_err(map_sqlx_error)
    }
}
