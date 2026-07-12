use async_trait::async_trait;
use ficant_domain::DomainErrorCode;
use ficant_domain::primitives::{OwnerRef, Ulid};
use ficant_domain::research::{ExperimentRun, RunState};

use super::fingerprint::{FingerprintBuilder, owner_bytes, run_bytes, run_state_code};
use super::{
    AccessScope, ApplicationResult, IdempotencyKey, OperationFingerprint, ResolvedRunRuleProof,
    ValidatedExperimentRun,
};
use crate::map_domain_error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateExperimentRun {
    scope: AccessScope,
    run: ExperimentRun,
    proof: ResolvedRunRuleProof,
    idempotency_key: IdempotencyKey,
    fingerprint: OperationFingerprint,
}

impl CreateExperimentRun {
    /// Creates an immutable run-creation command.
    ///
    /// ```compile_fail
    /// use ficant_application::ports::CreateExperimentRun;
    /// use ficant_application::{AccessScope, IdempotencyKey};
    /// use ficant_domain::research::ExperimentRun;
    /// let scope: AccessScope = panic!();
    /// let raw: ExperimentRun = panic!();
    /// let _ = CreateExperimentRun::new(scope, raw, IdempotencyKey::new("run").unwrap());
    /// ```
    ///
    /// A first-use Phase 1 candidate is not a persisted-run validation.
    ///
    /// ```compile_fail
    /// use ficant_application::ports::{CreateExperimentRun, Phase1ValidatedExperimentRun};
    /// use ficant_application::{AccessScope, IdempotencyKey};
    /// let scope: AccessScope = panic!();
    /// let candidate: Phase1ValidatedExperimentRun = panic!();
    /// let _ = CreateExperimentRun::new(
    ///     scope, candidate, IdempotencyKey::new("run").unwrap(),
    /// );
    /// ```
    ///
    /// # Errors
    ///
    /// Returns state conflict unless the run is at Created revision 1.
    pub fn new(
        scope: AccessScope,
        validated: ValidatedExperimentRun,
        idempotency_key: IdempotencyKey,
    ) -> ApplicationResult<Self> {
        validated.authorize_scope(&scope)?;
        let (run, proof) = validated.into_parts();
        if run.state() != RunState::Created || run.revision() != 1 {
            return Err(map_domain_error(DomainErrorCode::InvalidStateTransition));
        }
        let idempotency_key = idempotency_key.scoped_to(&scope)?;
        let mut canonical = FingerprintBuilder::new("create-experiment-run/v2");
        canonical.field(2, scope.fingerprint().content_hash().as_bytes());
        canonical.field(3, &owner_bytes(run.owner()));
        canonical.field(4, &run_bytes(&run));
        let fingerprint = canonical.finish();
        Ok(Self {
            scope,
            run,
            proof,
            idempotency_key,
            fingerprint,
        })
    }

    #[must_use]
    pub fn scope(&self) -> &AccessScope {
        &self.scope
    }

    #[must_use]
    pub fn target_owner(&self) -> &OwnerRef {
        self.run.owner()
    }

    #[must_use]
    pub fn run(&self) -> &ExperimentRun {
        &self.run
    }

    #[must_use]
    pub fn proof(&self) -> &ResolvedRunRuleProof {
        &self.proof
    }

    #[must_use]
    pub fn idempotency_key(&self) -> &IdempotencyKey {
        &self.idempotency_key
    }

    #[must_use]
    pub fn fingerprint(&self) -> &OperationFingerprint {
        &self.fingerprint
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransitionExperimentRun {
    scope: AccessScope,
    target_owner: OwnerRef,
    run_id: Ulid,
    expected_revision: u64,
    next_state: RunState,
    idempotency_key: IdempotencyKey,
    fingerprint: OperationFingerprint,
}

impl TransitionExperimentRun {
    /// Creates a revision-checked run transition command.
    ///
    /// # Errors
    ///
    /// Returns validation failure for revision zero or state conflict for Created.
    pub fn new(
        scope: AccessScope,
        target_owner: OwnerRef,
        run_id: Ulid,
        expected_revision: u64,
        next_state: RunState,
        idempotency_key: IdempotencyKey,
    ) -> ApplicationResult<Self> {
        scope.authorize(&target_owner)?;
        if expected_revision == 0 {
            return Err(map_domain_error(DomainErrorCode::InvalidValue));
        }
        if next_state == RunState::Created {
            return Err(map_domain_error(DomainErrorCode::InvalidStateTransition));
        }
        let idempotency_key = idempotency_key.scoped_to(&scope)?;
        let mut canonical = FingerprintBuilder::new("transition-experiment-run/v2");
        canonical.field(2, scope.fingerprint().content_hash().as_bytes());
        canonical.field(3, &owner_bytes(&target_owner));
        canonical.field(4, run_id.as_str().as_bytes());
        canonical.u64(5, expected_revision);
        canonical.field(6, &[run_state_code(next_state)]);
        let fingerprint = canonical.finish();
        Ok(Self {
            scope,
            target_owner,
            run_id,
            expected_revision,
            next_state,
            idempotency_key,
            fingerprint,
        })
    }

    #[must_use]
    pub fn scope(&self) -> &AccessScope {
        &self.scope
    }

    #[must_use]
    pub fn target_owner(&self) -> &OwnerRef {
        &self.target_owner
    }

    #[must_use]
    pub fn run_id(&self) -> &Ulid {
        &self.run_id
    }

    #[must_use]
    pub fn expected_revision(&self) -> u64 {
        self.expected_revision
    }

    #[must_use]
    pub fn next_state(&self) -> RunState {
        self.next_state
    }

    #[must_use]
    pub fn idempotency_key(&self) -> &IdempotencyKey {
        &self.idempotency_key
    }

    #[must_use]
    pub fn fingerprint(&self) -> &OperationFingerprint {
        &self.fingerprint
    }
}

#[async_trait]
pub trait ExperimentRepository: Send + Sync {
    /// Creates a run with frozen bindings.
    ///
    /// # Errors
    ///
    /// Returns an application error on validation or idempotency conflict.
    async fn create_run(&self, command: CreateExperimentRun) -> ApplicationResult<ExperimentRun>;

    /// Applies one optimistic-concurrency run transition.
    ///
    /// # Errors
    ///
    /// Returns an application error on revision or state conflict.
    async fn transition(
        &self,
        command: TransitionExperimentRun,
    ) -> ApplicationResult<ExperimentRun>;

    /// Reads a run by identity.
    ///
    /// # Errors
    ///
    /// Returns an application error when the query cannot be completed safely.
    async fn get_run(
        &self,
        scope: &AccessScope,
        run_id: Ulid,
    ) -> ApplicationResult<Option<ExperimentRun>>;
}
