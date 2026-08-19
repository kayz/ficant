use async_trait::async_trait;
use ficant_domain::primitives::{ContentHash, OwnerRef};
use ficant_runtime::{FormalOutputEvidence, FormalOutputEvidenceInput};

use super::{AccessScope, ApplicationResult};
use crate::{ApplicationError, ApplicationErrorCategory};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormalOutputRecord {
    owner: OwnerRef,
    evidence: FormalOutputEvidence,
    canonical_payload: Vec<u8>,
}

impl FormalOutputRecord {
    /// Creates one immutable formal output whose payload hash is bound by its evidence.
    ///
    /// # Errors
    ///
    /// Returns a fail-closed integrity error for an empty payload, owner drift, or payload hash
    /// mismatch.
    pub fn new(
        owner: OwnerRef,
        evidence: FormalOutputEvidence,
        canonical_payload: Vec<u8>,
    ) -> ApplicationResult<Self> {
        if canonical_payload.is_empty()
            || evidence.subject().owner() != &owner
            || &ContentHash::digest(&canonical_payload) != evidence.result_hash()
        {
            return Err(integrity_error());
        }
        Ok(Self {
            owner,
            evidence,
            canonical_payload,
        })
    }

    #[must_use]
    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    #[must_use]
    pub fn evidence(&self) -> &FormalOutputEvidence {
        &self.evidence
    }

    #[must_use]
    pub fn output_identity(&self) -> &ContentHash {
        self.evidence.output_identity()
    }

    #[must_use]
    pub fn canonical_payload(&self) -> &[u8] {
        &self.canonical_payload
    }

    #[must_use]
    pub fn into_parts(self) -> (OwnerRef, FormalOutputEvidence, Vec<u8>) {
        (self.owner, self.evidence, self.canonical_payload)
    }

    /// Recomputes every in-memory invariant before a value crosses a required-read boundary.
    ///
    /// # Errors
    ///
    /// Returns hash mismatch when the payload, subject owner, or claimed identity drifted.
    pub fn verify(&self) -> ApplicationResult<()> {
        if self.evidence.subject().owner() != &self.owner
            || &ContentHash::digest(&self.canonical_payload) != self.evidence.result_hash()
            || FormalOutputEvidence::from_claimed(
                FormalOutputEvidenceInput {
                    schema_id: self.evidence.schema_id().to_owned(),
                    subject: self.evidence.subject().clone(),
                    consumed_inputs: self.evidence.consumed_inputs().to_vec(),
                    code: self.evidence.code().clone(),
                    runtime: self.evidence.runtime().clone(),
                    implementations: self.evidence.implementations().to_vec(),
                    parameters_hash: self.evidence.parameters_hash().clone(),
                    seed: self.evidence.seed(),
                    result_hash: self.evidence.result_hash().clone(),
                },
                self.evidence.output_identity().clone(),
            )
            .is_err()
        {
            return Err(integrity_error());
        }
        Ok(())
    }
}

#[async_trait]
pub trait FormalOutputRepository: Send + Sync {
    /// Publishes idempotently by output identity. The same identity with any byte/evidence drift
    /// must fail closed.
    async fn publish(
        &self,
        scope: &AccessScope,
        record: FormalOutputRecord,
    ) -> ApplicationResult<FormalOutputRecord>;

    /// Loads one formal output by its stable identity, returning `None` only when it does not
    /// exist in the authorized owner scope.
    async fn get(
        &self,
        scope: &AccessScope,
        output_identity: &ContentHash,
    ) -> ApplicationResult<Option<FormalOutputRecord>>;
}

fn integrity_error() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::HashMismatch, false)
}
