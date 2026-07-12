use ficant_domain::market::MarketRulePack;
use ficant_domain::primitives::{ContentHash, MarketTime, OwnerRef, Ulid, VersionRef};
use ficant_domain::research::{DataSnapshot, ExperimentRun};
use ficant_domain::{ContentAddressed, DomainErrorCode, VersionedDefinition};

use super::definitions::{DefinitionRepository, DefinitionValue};
use super::fingerprint::{
    FingerprintBuilder, fact_bytes, market_time_bytes, owner_bytes, run_bytes, snapshot_bytes,
    version_ref_bytes,
};
use super::snapshots::{SnapshotRepository, SnapshotValue};
use super::unit_resolution::ValidatedMarketFact;
use super::{AccessScope, ApplicationResult, MarketFact, OperationFingerprint};
use crate::map_domain_error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarketFactRuleProofKind {
    NoRule,
    Valuation,
}

/// Opaque proof that a valuation's exact `RulePack` covered `valuation_at`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedValuationRuleProof {
    scope_fingerprint: OperationFingerprint,
    tenant_id: Ulid,
    fact_id: Ulid,
    fact_digest: ContentHash,
    subject: MarketTime,
    rule_pack: VersionRef,
    effective_from: MarketTime,
    effective_to: MarketTime,
    binding_hash: OperationFingerprint,
}

impl ResolvedValuationRuleProof {
    #[must_use]
    pub fn scope_fingerprint(&self) -> &OperationFingerprint {
        &self.scope_fingerprint
    }

    #[must_use]
    pub fn tenant_id(&self) -> &Ulid {
        &self.tenant_id
    }

    #[must_use]
    pub fn fact_id(&self) -> &Ulid {
        &self.fact_id
    }

    #[must_use]
    pub fn fact_digest(&self) -> &ContentHash {
        &self.fact_digest
    }

    #[must_use]
    pub fn subject(&self) -> &MarketTime {
        &self.subject
    }

    #[must_use]
    pub fn rule_pack(&self) -> &VersionRef {
        &self.rule_pack
    }

    #[must_use]
    pub fn effective_from(&self) -> &MarketTime {
        &self.effective_from
    }

    #[must_use]
    pub fn effective_to(&self) -> &MarketTime {
        &self.effective_to
    }

    #[must_use]
    pub fn binding_hash(&self) -> &OperationFingerprint {
        &self.binding_hash
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum MarketFactRuleProofInner {
    NoRule {
        scope_fingerprint: OperationFingerprint,
        tenant_id: Ulid,
        fact_id: Ulid,
        fact_digest: ContentHash,
        binding_hash: OperationFingerprint,
    },
    Valuation(ResolvedValuationRuleProof),
}

/// Opaque evidence that a non-valuation has no `RulePack` obligation, or that a valuation's
/// exact `RulePack` covers its valuation subject time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarketFactRuleProof {
    inner: MarketFactRuleProofInner,
}

impl MarketFactRuleProof {
    #[must_use]
    pub fn kind(&self) -> MarketFactRuleProofKind {
        match self.inner {
            MarketFactRuleProofInner::NoRule { .. } => MarketFactRuleProofKind::NoRule,
            MarketFactRuleProofInner::Valuation(_) => MarketFactRuleProofKind::Valuation,
        }
    }

    #[must_use]
    pub fn valuation(&self) -> Option<&ResolvedValuationRuleProof> {
        match &self.inner {
            MarketFactRuleProofInner::Valuation(value) => Some(value),
            MarketFactRuleProofInner::NoRule { .. } => None,
        }
    }

    #[must_use]
    pub fn scope_fingerprint(&self) -> &OperationFingerprint {
        match &self.inner {
            MarketFactRuleProofInner::NoRule {
                scope_fingerprint, ..
            } => scope_fingerprint,
            MarketFactRuleProofInner::Valuation(value) => &value.scope_fingerprint,
        }
    }

    #[must_use]
    pub fn tenant_id(&self) -> &Ulid {
        match &self.inner {
            MarketFactRuleProofInner::NoRule { tenant_id, .. } => tenant_id,
            MarketFactRuleProofInner::Valuation(value) => &value.tenant_id,
        }
    }

    #[must_use]
    pub fn fact_id(&self) -> &Ulid {
        match &self.inner {
            MarketFactRuleProofInner::NoRule { fact_id, .. } => fact_id,
            MarketFactRuleProofInner::Valuation(value) => &value.fact_id,
        }
    }

    #[must_use]
    pub fn fact_digest(&self) -> &ContentHash {
        match &self.inner {
            MarketFactRuleProofInner::NoRule { fact_digest, .. } => fact_digest,
            MarketFactRuleProofInner::Valuation(value) => &value.fact_digest,
        }
    }

    #[must_use]
    pub fn binding_hash(&self) -> &OperationFingerprint {
        match &self.inner {
            MarketFactRuleProofInner::NoRule { binding_hash, .. } => binding_hash,
            MarketFactRuleProofInner::Valuation(value) => &value.binding_hash,
        }
    }

    fn validate_for(
        &self,
        scope: Option<&AccessScope>,
        fact: &MarketFact,
    ) -> ApplicationResult<()> {
        let digest = ContentHash::digest(&fact_bytes(fact));
        match (&self.inner, fact) {
            (
                MarketFactRuleProofInner::NoRule {
                    scope_fingerprint,
                    tenant_id,
                    fact_id,
                    fact_digest,
                    binding_hash,
                },
                MarketFact::Cashflow(_) | MarketFact::Quote(_) | MarketFact::Trade(_),
            ) => {
                if scope.is_some_and(|scope| scope_fingerprint != scope.fingerprint())
                    || scope.is_some_and(|scope| tenant_id != scope.tenant_id())
                    || tenant_id != fact.owner().tenant_id()
                    || fact_id != fact.id()
                    || fact_digest != &digest
                    || binding_hash
                        != &no_rule_hash(scope_fingerprint, tenant_id, fact_id, fact_digest)
                {
                    return Err(lineage_incomplete());
                }
            }
            (MarketFactRuleProofInner::Valuation(proof), MarketFact::Valuation(valuation)) => {
                if scope.is_some_and(|scope| proof.scope_fingerprint != *scope.fingerprint())
                    || scope.is_some_and(|scope| proof.tenant_id != *scope.tenant_id())
                    || proof.tenant_id != *fact.owner().tenant_id()
                    || proof.fact_id != *fact.id()
                    || proof.fact_digest != digest
                    || proof.subject != *valuation.valuation_at()
                    || proof.rule_pack != *valuation.rule_pack()
                    || !covers(&proof.effective_from, &proof.effective_to, &proof.subject)
                    || proof.binding_hash != valuation_rule_hash(proof)
                {
                    return Err(lineage_incomplete());
                }
            }
            _ => return Err(lineage_incomplete()),
        }
        Ok(())
    }
}

/// A fact carrying both its D-019 unit proof and its D-020 RulePack/NoRule proof.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FullyValidatedMarketFact {
    unit_validated: ValidatedMarketFact,
    rule_proof: MarketFactRuleProof,
}

impl FullyValidatedMarketFact {
    #[must_use]
    pub fn fact(&self) -> &MarketFact {
        self.unit_validated.fact()
    }

    #[must_use]
    pub fn unit_proof(&self) -> &super::ResolvedMarketFactProof {
        self.unit_validated.proof()
    }

    #[must_use]
    pub fn rule_proof(&self) -> &MarketFactRuleProof {
        &self.rule_proof
    }

    pub(crate) fn validate(&self) -> ApplicationResult<()> {
        self.unit_validated.validate()?;
        if self.rule_proof.scope_fingerprint() != self.unit_validated.proof().scope_fingerprint()
            || self.rule_proof.tenant_id() != self.unit_validated.proof().tenant_id()
        {
            return Err(lineage_incomplete());
        }
        self.rule_proof
            .validate_for(None, self.unit_validated.fact())
    }

    pub(crate) fn authorize_scope(&self, scope: &AccessScope) -> ApplicationResult<()> {
        self.unit_validated.authorize_scope(scope)?;
        self.rule_proof
            .validate_for(Some(scope), self.unit_validated.fact())
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        MarketFact,
        super::ResolvedMarketFactProof,
        MarketFactRuleProof,
    ) {
        let (fact, unit_proof) = self.unit_validated.into_parts();
        (fact, unit_proof, self.rule_proof)
    }
}

pub struct MarketFactRulePackResolver<'a> {
    definitions: &'a dyn DefinitionRepository,
}

impl<'a> MarketFactRulePackResolver<'a> {
    #[must_use]
    pub fn new(definitions: &'a dyn DefinitionRepository) -> Self {
        Self { definitions }
    }

    /// Resolves the exact `RulePack` for valuations. Other fact kinds receive an internal `NoRule`
    /// proof and callers cannot choose that state.
    ///
    /// # Errors
    ///
    /// Returns a non-retryable lineage error for an invalid exact reference or a validation error
    /// when `valuation_at` is outside the half-open effective interval.
    pub async fn resolve(
        &self,
        scope: &AccessScope,
        unit_validated: ValidatedMarketFact,
    ) -> ApplicationResult<FullyValidatedMarketFact> {
        unit_validated.authorize_scope(scope)?;
        let fact = unit_validated.fact();
        let fact_digest = ContentHash::digest(&fact_bytes(fact));
        let proof = match fact {
            MarketFact::Valuation(valuation) => {
                let reference = valuation.rule_pack();
                let resolved = self
                    .definitions
                    .get_version(scope, reference.id().clone(), reference.version())
                    .await?
                    .ok_or_else(lineage_incomplete)?;
                let DefinitionValue::MarketRulePack(rule_pack) = resolved else {
                    return Err(lineage_incomplete());
                };
                validate_exact_rule_pack(scope, fact.owner().tenant_id(), reference, &rule_pack)?;
                if !covers(
                    rule_pack.effective().from(),
                    rule_pack.effective().to(),
                    valuation.valuation_at(),
                ) {
                    return Err(coverage_miss());
                }
                let scope_fingerprint = scope.fingerprint().clone();
                let tenant_id = scope.tenant_id().clone();
                let fact_id = fact.id().clone();
                let subject = valuation.valuation_at().clone();
                let rule_pack_reference = reference.clone();
                let effective_from = rule_pack.effective().from().clone();
                let effective_to = rule_pack.effective().to().clone();
                let binding_hash = valuation_rule_hash_parts(
                    &scope_fingerprint,
                    &tenant_id,
                    &fact_id,
                    &fact_digest,
                    &subject,
                    &rule_pack_reference,
                    &effective_from,
                    &effective_to,
                );
                let value = ResolvedValuationRuleProof {
                    scope_fingerprint,
                    tenant_id,
                    fact_id,
                    fact_digest,
                    subject,
                    rule_pack: rule_pack_reference,
                    effective_from,
                    effective_to,
                    binding_hash,
                };
                MarketFactRuleProof {
                    inner: MarketFactRuleProofInner::Valuation(value),
                }
            }
            MarketFact::Cashflow(_) | MarketFact::Quote(_) | MarketFact::Trade(_) => {
                let scope_fingerprint = scope.fingerprint().clone();
                let tenant_id = scope.tenant_id().clone();
                let fact_id = fact.id().clone();
                let binding_hash =
                    no_rule_hash(&scope_fingerprint, &tenant_id, &fact_id, &fact_digest);
                MarketFactRuleProof {
                    inner: MarketFactRuleProofInner::NoRule {
                        scope_fingerprint,
                        tenant_id,
                        fact_id,
                        fact_digest,
                        binding_hash,
                    },
                }
            }
        };
        let validated = FullyValidatedMarketFact {
            unit_validated,
            rule_proof: proof,
        };
        validated.authorize_scope(scope)?;
        Ok(validated)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedRunRuleBinding {
    rule_pack: VersionRef,
    effective_from: MarketTime,
    effective_to: MarketTime,
}

impl ResolvedRunRuleBinding {
    #[must_use]
    pub fn rule_pack(&self) -> &VersionRef {
        &self.rule_pack
    }

    #[must_use]
    pub fn effective_from(&self) -> &MarketTime {
        &self.effective_from
    }

    #[must_use]
    pub fn effective_to(&self) -> &MarketTime {
        &self.effective_to
    }
}

/// Opaque proof binding a run to its exact `DataSnapshot` and complete ordered `RulePack` set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedRunRuleProof {
    scope_fingerprint: OperationFingerprint,
    tenant_id: Ulid,
    run_id: Ulid,
    run_digest: ContentHash,
    snapshot_id: Ulid,
    snapshot_content_hash: ContentHash,
    snapshot_digest: ContentHash,
    as_of: MarketTime,
    bindings: Vec<ResolvedRunRuleBinding>,
    binding_hash: OperationFingerprint,
}

impl ResolvedRunRuleProof {
    #[must_use]
    pub fn scope_fingerprint(&self) -> &OperationFingerprint {
        &self.scope_fingerprint
    }

    #[must_use]
    pub fn tenant_id(&self) -> &Ulid {
        &self.tenant_id
    }

    #[must_use]
    pub fn run_id(&self) -> &Ulid {
        &self.run_id
    }

    #[must_use]
    pub fn run_digest(&self) -> &ContentHash {
        &self.run_digest
    }

    #[must_use]
    pub fn snapshot_id(&self) -> &Ulid {
        &self.snapshot_id
    }

    #[must_use]
    pub fn snapshot_content_hash(&self) -> &ContentHash {
        &self.snapshot_content_hash
    }

    #[must_use]
    pub fn snapshot_digest(&self) -> &ContentHash {
        &self.snapshot_digest
    }

    #[must_use]
    pub fn as_of(&self) -> &MarketTime {
        &self.as_of
    }

    #[must_use]
    pub fn bindings(&self) -> &[ResolvedRunRuleBinding] {
        &self.bindings
    }

    #[must_use]
    pub fn binding_hash(&self) -> &OperationFingerprint {
        &self.binding_hash
    }

    fn validate_for(&self, scope: &AccessScope, run: &ExperimentRun) -> ApplicationResult<()> {
        if self.scope_fingerprint != *scope.fingerprint()
            || self.tenant_id != *scope.tenant_id()
            || self.run_id != *run.id()
            || self.run_digest != ContentHash::digest(&run_bytes(run))
            || self.snapshot_id != *run.data_snapshot().object_id()
            || run.data_snapshot().version().is_some()
            || run.data_snapshot().content_hash() != Some(&self.snapshot_content_hash)
            || run.rule_packs().len() != self.bindings.len()
            || has_duplicate_refs(run.rule_packs())
        {
            return Err(lineage_incomplete());
        }
        for (reference, binding) in run.rule_packs().iter().zip(&self.bindings) {
            if reference != &binding.rule_pack
                || !covers(&binding.effective_from, &binding.effective_to, &self.as_of)
            {
                return Err(lineage_incomplete());
            }
        }
        if self.binding_hash != run_rule_hash(self) {
            return Err(lineage_incomplete());
        }
        Ok(())
    }

    fn validate_snapshot(&self, snapshot: &DataSnapshot) -> ApplicationResult<()> {
        let value = SnapshotValue::Data(snapshot.clone());
        if self.snapshot_id != *snapshot.id()
            || self.snapshot_content_hash != *snapshot.content_hash()
            || self.snapshot_digest != ContentHash::digest(&snapshot_bytes(&value))
            || self.as_of != *snapshot.as_of()
            || self.tenant_id != *snapshot.owner().tenant_id()
        {
            return Err(lineage_incomplete());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedExperimentRun {
    run: ExperimentRun,
    proof: ResolvedRunRuleProof,
}

impl ValidatedExperimentRun {
    #[must_use]
    pub fn run(&self) -> &ExperimentRun {
        &self.run
    }

    #[must_use]
    pub fn proof(&self) -> &ResolvedRunRuleProof {
        &self.proof
    }

    pub(crate) fn authorize_scope(&self, scope: &AccessScope) -> ApplicationResult<()> {
        scope.authorize(self.run.owner())?;
        self.proof.validate_for(scope, &self.run)
    }

    pub(crate) fn validate_snapshot(&self, snapshot: &DataSnapshot) -> ApplicationResult<()> {
        self.proof.validate_snapshot(snapshot)
    }

    pub(crate) fn into_parts(self) -> (ExperimentRun, ResolvedRunRuleProof) {
        (self.run, self.proof)
    }
}

/// Opaque Phase 1 candidate proof built from a pre-stage `DataSnapshot` without reading a
/// persisted snapshot repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Phase1ResolvedRunRuleProof {
    resolved: ResolvedRunRuleProof,
    run_owner: OwnerRef,
    snapshot_owner: OwnerRef,
    candidate_binding_hash: OperationFingerprint,
}

impl Phase1ResolvedRunRuleProof {
    #[must_use]
    pub fn scope_fingerprint(&self) -> &OperationFingerprint {
        self.resolved.scope_fingerprint()
    }

    #[must_use]
    pub fn tenant_id(&self) -> &Ulid {
        self.resolved.tenant_id()
    }

    #[must_use]
    pub fn run_id(&self) -> &Ulid {
        self.resolved.run_id()
    }

    #[must_use]
    pub fn run_digest(&self) -> &ContentHash {
        self.resolved.run_digest()
    }

    #[must_use]
    pub fn run_owner(&self) -> &OwnerRef {
        &self.run_owner
    }

    #[must_use]
    pub fn snapshot_id(&self) -> &Ulid {
        self.resolved.snapshot_id()
    }

    #[must_use]
    pub fn snapshot_content_hash(&self) -> &ContentHash {
        self.resolved.snapshot_content_hash()
    }

    #[must_use]
    pub fn snapshot_digest(&self) -> &ContentHash {
        self.resolved.snapshot_digest()
    }

    #[must_use]
    pub fn snapshot_owner(&self) -> &OwnerRef {
        &self.snapshot_owner
    }

    #[must_use]
    pub fn as_of(&self) -> &MarketTime {
        self.resolved.as_of()
    }

    #[must_use]
    pub fn bindings(&self) -> &[ResolvedRunRuleBinding] {
        self.resolved.bindings()
    }

    #[must_use]
    pub fn binding_hash(&self) -> &OperationFingerprint {
        &self.candidate_binding_hash
    }

    fn validate_for(&self, scope: &AccessScope, run: &ExperimentRun) -> ApplicationResult<()> {
        self.resolved.validate_for(scope, run)?;
        if self.run_owner != *run.owner()
            || self.run_owner.tenant_id() != scope.tenant_id()
            || self.snapshot_owner.tenant_id() != scope.tenant_id()
            || self.candidate_binding_hash != phase1_candidate_hash(self)
        {
            return Err(lineage_incomplete());
        }
        Ok(())
    }

    fn validate_snapshot(&self, snapshot: &DataSnapshot) -> ApplicationResult<()> {
        self.resolved.validate_snapshot(snapshot)?;
        if self.snapshot_owner != *snapshot.owner()
            || self.snapshot_owner != self.run_owner
            || self.candidate_binding_hash != phase1_candidate_hash(self)
        {
            return Err(lineage_incomplete());
        }
        Ok(())
    }
}

/// A raw run that has been validated specifically against the pre-stage Phase 1 snapshot.
/// This type is deliberately distinct from [`ValidatedExperimentRun`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Phase1ValidatedExperimentRun {
    run: ExperimentRun,
    proof: Phase1ResolvedRunRuleProof,
}

impl Phase1ValidatedExperimentRun {
    #[must_use]
    pub fn run(&self) -> &ExperimentRun {
        &self.run
    }

    #[must_use]
    pub fn proof(&self) -> &Phase1ResolvedRunRuleProof {
        &self.proof
    }

    pub(crate) fn authorize_scope(&self, scope: &AccessScope) -> ApplicationResult<()> {
        scope.authorize(self.run.owner())?;
        self.proof.validate_for(scope, &self.run)
    }

    pub(crate) fn validate_snapshot(&self, snapshot: &DataSnapshot) -> ApplicationResult<()> {
        self.proof.validate_snapshot(snapshot)
    }

    pub(crate) fn into_persisted_validation(
        self,
        scope: &AccessScope,
    ) -> ApplicationResult<ValidatedExperimentRun> {
        self.authorize_scope(scope)?;
        Ok(ValidatedExperimentRun {
            run: self.run,
            proof: self.proof.resolved,
        })
    }
}

/// Definitions-only resolver for a first-use Phase 1 run candidate.
pub struct Phase1RunCandidateResolver<'a> {
    definitions: &'a dyn DefinitionRepository,
}

impl<'a> Phase1RunCandidateResolver<'a> {
    #[must_use]
    pub fn new(definitions: &'a dyn DefinitionRepository) -> Self {
        Self { definitions }
    }

    /// Validates a raw run against its exact pre-stage snapshot and all explicit `RulePack`
    /// references without reading `SnapshotRepository` or invoking mutation-capable ports.
    ///
    /// # Errors
    ///
    /// Returns non-retryable lineage errors for mismatched references and validation errors for
    /// effective-interval misses.
    pub async fn resolve(
        &self,
        scope: &AccessScope,
        run: ExperimentRun,
        snapshot: &DataSnapshot,
    ) -> ApplicationResult<Phase1ValidatedExperimentRun> {
        scope.authorize(run.owner())?;
        let resolved = resolve_run_rule_proof(self.definitions, scope, &run, snapshot).await?;
        let run_owner = run.owner().clone();
        let snapshot_owner = snapshot.owner().clone();
        let candidate_binding_hash =
            phase1_candidate_hash_parts(&resolved, &run_owner, &snapshot_owner);
        let proof = Phase1ResolvedRunRuleProof {
            resolved,
            run_owner,
            snapshot_owner,
            candidate_binding_hash,
        };
        let validated = Phase1ValidatedExperimentRun { run, proof };
        validated.authorize_scope(scope)?;
        validated.validate_snapshot(snapshot)?;
        Ok(validated)
    }
}

pub struct MarketRunRulePackResolver<'a> {
    definitions: &'a dyn DefinitionRepository,
    snapshots: &'a dyn SnapshotRepository,
}

impl<'a> MarketRunRulePackResolver<'a> {
    #[must_use]
    pub fn new(
        definitions: &'a dyn DefinitionRepository,
        snapshots: &'a dyn SnapshotRepository,
    ) -> Self {
        Self {
            definitions,
            snapshots,
        }
    }

    /// Resolves the run's exact `DataSnapshot` and every explicit `RulePack` before any command can
    /// consume mutation-capable ports.
    ///
    /// # Errors
    ///
    /// Returns a non-retryable lineage error for incomplete or mismatched references, or a
    /// validation error when any rule does not cover the snapshot's `as_of` subject.
    pub async fn resolve(
        &self,
        scope: &AccessScope,
        run: ExperimentRun,
    ) -> ApplicationResult<ValidatedExperimentRun> {
        scope.authorize(run.owner())?;
        if run.data_snapshot().version().is_some()
            || run.data_snapshot().content_hash().is_none()
            || has_duplicate_refs(run.rule_packs())
        {
            return Err(lineage_incomplete());
        }
        let snapshot = self
            .snapshots
            .get_by_id(scope, run.data_snapshot().object_id().clone())
            .await?
            .ok_or_else(lineage_incomplete)?;
        let SnapshotValue::Data(snapshot) = snapshot else {
            return Err(lineage_incomplete());
        };
        let proof = resolve_run_rule_proof(self.definitions, scope, &run, &snapshot).await?;
        let validated = ValidatedExperimentRun { run, proof };
        validated.authorize_scope(scope)?;
        validated.validate_snapshot(&snapshot)?;
        Ok(validated)
    }
}

async fn resolve_run_rule_proof(
    definitions: &dyn DefinitionRepository,
    scope: &AccessScope,
    run: &ExperimentRun,
    snapshot: &DataSnapshot,
) -> ApplicationResult<ResolvedRunRuleProof> {
    if run.data_snapshot().version().is_some()
        || run.data_snapshot().content_hash().is_none()
        || has_duplicate_refs(run.rule_packs())
        || snapshot.id() != run.data_snapshot().object_id()
        || run.data_snapshot().content_hash() != Some(snapshot.content_hash())
        || snapshot.owner().tenant_id() != scope.tenant_id()
        || snapshot.owner().tenant_id() != run.owner().tenant_id()
    {
        return Err(lineage_incomplete());
    }

    let mut bindings = Vec::with_capacity(run.rule_packs().len());
    for reference in run.rule_packs() {
        let resolved = definitions
            .get_version(scope, reference.id().clone(), reference.version())
            .await?
            .ok_or_else(lineage_incomplete)?;
        let DefinitionValue::MarketRulePack(rule_pack) = resolved else {
            return Err(lineage_incomplete());
        };
        validate_exact_rule_pack(scope, run.owner().tenant_id(), reference, &rule_pack)?;
        if !covers(
            rule_pack.effective().from(),
            rule_pack.effective().to(),
            snapshot.as_of(),
        ) {
            return Err(coverage_miss());
        }
        bindings.push(ResolvedRunRuleBinding {
            rule_pack: reference.clone(),
            effective_from: rule_pack.effective().from().clone(),
            effective_to: rule_pack.effective().to().clone(),
        });
    }

    let snapshot_value = SnapshotValue::Data(snapshot.clone());
    let scope_fingerprint = scope.fingerprint().clone();
    let tenant_id = scope.tenant_id().clone();
    let run_id = run.id().clone();
    let run_digest = ContentHash::digest(&run_bytes(run));
    let snapshot_id = snapshot.id().clone();
    let snapshot_content_hash = snapshot.content_hash().clone();
    let snapshot_digest = ContentHash::digest(&snapshot_bytes(&snapshot_value));
    let as_of = snapshot.as_of().clone();
    let binding_hash = run_rule_hash_parts(
        &scope_fingerprint,
        &tenant_id,
        &run_id,
        &run_digest,
        &snapshot_id,
        &snapshot_content_hash,
        &snapshot_digest,
        &as_of,
        &bindings,
    );
    Ok(ResolvedRunRuleProof {
        scope_fingerprint,
        tenant_id,
        run_id,
        run_digest,
        snapshot_id,
        snapshot_content_hash,
        snapshot_digest,
        as_of,
        bindings,
        binding_hash,
    })
}

fn validate_exact_rule_pack(
    scope: &AccessScope,
    subject_tenant: &Ulid,
    reference: &VersionRef,
    rule_pack: &MarketRulePack,
) -> ApplicationResult<()> {
    if rule_pack.identity() != reference.id().as_str()
        || rule_pack.version() != reference.version().get()
        || rule_pack.owner().tenant_id() != scope.tenant_id()
        || rule_pack.owner().tenant_id() != subject_tenant
    {
        return Err(lineage_incomplete());
    }
    Ok(())
}

fn covers(from: &MarketTime, to: &MarketTime, subject: &MarketTime) -> bool {
    from.instant() <= subject.instant() && subject.instant() < to.instant()
}

fn has_duplicate_refs(references: &[VersionRef]) -> bool {
    references.iter().enumerate().any(|(index, reference)| {
        references[..index]
            .iter()
            .any(|previous| previous == reference)
    })
}

fn no_rule_hash(
    scope: &OperationFingerprint,
    tenant_id: &Ulid,
    fact_id: &Ulid,
    fact_digest: &ContentHash,
) -> OperationFingerprint {
    let mut value = FingerprintBuilder::new("market-fact-no-rule-proof/v1");
    value.field(2, scope.content_hash().as_bytes());
    value.field(3, tenant_id.as_str().as_bytes());
    value.field(4, fact_id.as_str().as_bytes());
    value.field(5, fact_digest.as_bytes());
    value.finish()
}

fn valuation_rule_hash(proof: &ResolvedValuationRuleProof) -> OperationFingerprint {
    valuation_rule_hash_parts(
        &proof.scope_fingerprint,
        &proof.tenant_id,
        &proof.fact_id,
        &proof.fact_digest,
        &proof.subject,
        &proof.rule_pack,
        &proof.effective_from,
        &proof.effective_to,
    )
}

#[allow(clippy::too_many_arguments)]
fn valuation_rule_hash_parts(
    scope_fingerprint: &OperationFingerprint,
    tenant_id: &Ulid,
    fact_id: &Ulid,
    fact_digest: &ContentHash,
    subject: &MarketTime,
    rule_pack: &VersionRef,
    effective_from: &MarketTime,
    effective_to: &MarketTime,
) -> OperationFingerprint {
    let mut value = FingerprintBuilder::new("resolved-valuation-rule-proof/v1");
    value.field(2, scope_fingerprint.content_hash().as_bytes());
    value.field(3, tenant_id.as_str().as_bytes());
    value.field(4, fact_id.as_str().as_bytes());
    value.field(5, fact_digest.as_bytes());
    value.field(6, &market_time_bytes(subject));
    value.field(7, &version_ref_bytes(rule_pack));
    value.field(8, &market_time_bytes(effective_from));
    value.field(9, &market_time_bytes(effective_to));
    value.finish()
}

fn run_rule_hash(proof: &ResolvedRunRuleProof) -> OperationFingerprint {
    run_rule_hash_parts(
        &proof.scope_fingerprint,
        &proof.tenant_id,
        &proof.run_id,
        &proof.run_digest,
        &proof.snapshot_id,
        &proof.snapshot_content_hash,
        &proof.snapshot_digest,
        &proof.as_of,
        &proof.bindings,
    )
}

fn phase1_candidate_hash(proof: &Phase1ResolvedRunRuleProof) -> OperationFingerprint {
    phase1_candidate_hash_parts(&proof.resolved, &proof.run_owner, &proof.snapshot_owner)
}

fn phase1_candidate_hash_parts(
    resolved: &ResolvedRunRuleProof,
    run_owner: &OwnerRef,
    snapshot_owner: &OwnerRef,
) -> OperationFingerprint {
    let mut value = FingerprintBuilder::new("phase1-run-candidate-proof/v1");
    value.field(2, resolved.binding_hash().content_hash().as_bytes());
    value.field(3, &owner_bytes(run_owner));
    value.field(4, &owner_bytes(snapshot_owner));
    value.finish()
}

#[allow(clippy::too_many_arguments)]
fn run_rule_hash_parts(
    scope_fingerprint: &OperationFingerprint,
    tenant_id: &Ulid,
    run_id: &Ulid,
    run_digest: &ContentHash,
    snapshot_id: &Ulid,
    snapshot_content_hash: &ContentHash,
    snapshot_digest: &ContentHash,
    as_of: &MarketTime,
    bindings: &[ResolvedRunRuleBinding],
) -> OperationFingerprint {
    let mut value = FingerprintBuilder::new("resolved-run-rule-proof/v1");
    value.field(2, scope_fingerprint.content_hash().as_bytes());
    value.field(3, tenant_id.as_str().as_bytes());
    value.field(4, run_id.as_str().as_bytes());
    value.field(5, run_digest.as_bytes());
    value.field(6, snapshot_id.as_str().as_bytes());
    value.field(7, snapshot_content_hash.as_bytes());
    value.field(8, snapshot_digest.as_bytes());
    value.field(9, &market_time_bytes(as_of));
    for binding in bindings {
        value.field(10, &version_ref_bytes(&binding.rule_pack));
        value.field(11, &market_time_bytes(&binding.effective_from));
        value.field(12, &market_time_bytes(&binding.effective_to));
    }
    value.finish()
}

fn coverage_miss() -> crate::ApplicationError {
    map_domain_error(DomainErrorCode::InvalidValue)
}

fn lineage_incomplete() -> crate::ApplicationError {
    map_domain_error(DomainErrorCode::BrokenLineage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ficant_domain::primitives::{LineageRef, OwnerRef, Version};
    use ficant_domain::research::{DataSnapshotInput, ExperimentRunInput};

    #[test]
    fn run_entry_rechecks_missing_extra_duplicate_and_run_swap() {
        let scope = scope();
        let data = snapshot('D', 3, hash(20));
        let original_run = run('X', &data, vec![reference('R'), reference('S')]);
        let validated = validated_run(&scope, original_run.clone(), &data);
        validated.authorize_scope(&scope).unwrap();

        let mut missing = validated.clone();
        missing.proof.bindings.pop();
        missing.proof.binding_hash = run_rule_hash(&missing.proof);
        assert_lineage(&missing.authorize_scope(&scope).unwrap_err());

        let mut extra = validated.clone();
        extra.proof.bindings.push(binding('T'));
        extra.proof.binding_hash = run_rule_hash(&extra.proof);
        assert_lineage(&extra.authorize_scope(&scope).unwrap_err());

        let duplicate_run = run('X', &data, vec![reference('R'), reference('R')]);
        let duplicate = validated_run(&scope, duplicate_run, &data);
        assert_lineage(&duplicate.authorize_scope(&scope).unwrap_err());

        let swapped_run = run('Y', &data, vec![reference('R'), reference('S')]);
        let swapped = ValidatedExperimentRun {
            run: swapped_run,
            proof: validated.proof,
        };
        assert_lineage(&swapped.authorize_scope(&scope).unwrap_err());
    }

    #[test]
    fn run_entry_rechecks_snapshot_id_hash_digest_and_as_of() {
        let scope = scope();
        let data = snapshot('D', 3, hash(20));
        let validated = validated_run(&scope, run('X', &data, vec![reference('R')]), &data);
        validated.validate_snapshot(&data).unwrap();

        for wrong in [
            snapshot('E', 3, hash(20)),
            snapshot('D', 3, hash(21)),
            snapshot('D', 2, hash(20)),
        ] {
            assert_lineage(&validated.validate_snapshot(&wrong).unwrap_err());
        }
    }

    #[test]
    fn phase1_candidate_rechecks_missing_extra_run_swap_and_staged_snapshot_drift() {
        let scope = scope();
        let data = snapshot('D', 3, hash(20));
        let original_run = run('X', &data, vec![reference('R'), reference('S')]);
        let candidate = phase1_validated_run(&scope, original_run, &data);
        candidate.authorize_scope(&scope).unwrap();
        candidate.validate_snapshot(&data).unwrap();

        let mut missing = candidate.clone();
        missing.proof.resolved.bindings.pop();
        missing.proof.resolved.binding_hash = run_rule_hash(&missing.proof.resolved);
        missing.proof.candidate_binding_hash = phase1_candidate_hash(&missing.proof);
        assert_lineage(&missing.authorize_scope(&scope).unwrap_err());

        let mut extra = candidate.clone();
        extra.proof.resolved.bindings.push(binding('T'));
        extra.proof.resolved.binding_hash = run_rule_hash(&extra.proof.resolved);
        extra.proof.candidate_binding_hash = phase1_candidate_hash(&extra.proof);
        assert_lineage(&extra.authorize_scope(&scope).unwrap_err());

        let swapped = Phase1ValidatedExperimentRun {
            run: run('Y', &data, vec![reference('R'), reference('S')]),
            proof: candidate.proof.clone(),
        };
        assert_lineage(&swapped.authorize_scope(&scope).unwrap_err());

        let owner_drift = snapshot_with_owner('D', 3, hash(20), OwnerRef::new(id('T'), id('B')));
        for wrong in [
            snapshot('E', 3, hash(20)),
            snapshot('D', 3, hash(21)),
            snapshot('D', 2, hash(20)),
            owner_drift,
        ] {
            assert_lineage(&candidate.validate_snapshot(&wrong).unwrap_err());
        }
    }

    fn validated_run(
        scope: &AccessScope,
        run: ExperimentRun,
        snapshot: &DataSnapshot,
    ) -> ValidatedExperimentRun {
        let bindings: Vec<ResolvedRunRuleBinding> = run
            .rule_packs()
            .iter()
            .map(|reference| ResolvedRunRuleBinding {
                rule_pack: reference.clone(),
                effective_from: time(1),
                effective_to: time(5),
            })
            .collect();
        let snapshot_value = SnapshotValue::Data(snapshot.clone());
        let scope_fingerprint = scope.fingerprint().clone();
        let tenant_id = scope.tenant_id().clone();
        let run_id = run.id().clone();
        let run_digest = ContentHash::digest(&run_bytes(&run));
        let snapshot_id = snapshot.id().clone();
        let snapshot_content_hash = snapshot.content_hash().clone();
        let snapshot_digest = ContentHash::digest(&snapshot_bytes(&snapshot_value));
        let as_of = snapshot.as_of().clone();
        let binding_hash = run_rule_hash_parts(
            &scope_fingerprint,
            &tenant_id,
            &run_id,
            &run_digest,
            &snapshot_id,
            &snapshot_content_hash,
            &snapshot_digest,
            &as_of,
            &bindings,
        );
        let proof = ResolvedRunRuleProof {
            scope_fingerprint,
            tenant_id,
            run_id,
            run_digest,
            snapshot_id,
            snapshot_content_hash,
            snapshot_digest,
            as_of,
            bindings,
            binding_hash,
        };
        ValidatedExperimentRun { run, proof }
    }

    fn phase1_validated_run(
        scope: &AccessScope,
        run: ExperimentRun,
        snapshot: &DataSnapshot,
    ) -> Phase1ValidatedExperimentRun {
        let persisted = validated_run(scope, run, snapshot);
        let (run, resolved) = persisted.into_parts();
        let run_owner = run.owner().clone();
        let snapshot_owner = snapshot.owner().clone();
        let candidate_binding_hash =
            phase1_candidate_hash_parts(&resolved, &run_owner, &snapshot_owner);
        Phase1ValidatedExperimentRun {
            run,
            proof: Phase1ResolvedRunRuleProof {
                resolved,
                run_owner,
                snapshot_owner,
                candidate_binding_hash,
            },
        }
    }

    fn binding(suffix: char) -> ResolvedRunRuleBinding {
        ResolvedRunRuleBinding {
            rule_pack: reference(suffix),
            effective_from: time(1),
            effective_to: time(5),
        }
    }

    fn run(suffix: char, snapshot: &DataSnapshot, rule_packs: Vec<VersionRef>) -> ExperimentRun {
        ExperimentRun::new(ExperimentRunInput {
            experiment_run_id: id(suffix),
            owner: owner(),
            data_snapshot: LineageRef::content_addressed(
                snapshot.id().clone(),
                snapshot.content_hash().clone(),
            ),
            universe_snapshot: LineageRef::content_addressed(id('U'), hash(30)),
            rule_packs,
            runtime_image_digest: hash(31),
            parameters_hash: hash(32),
            seed: 7,
        })
        .unwrap()
    }

    fn snapshot(suffix: char, as_of: u32, content_hash: ContentHash) -> DataSnapshot {
        snapshot_with_owner(suffix, as_of, content_hash, owner())
    }

    fn snapshot_with_owner(
        suffix: char,
        as_of: u32,
        content_hash: ContentHash,
        snapshot_owner: OwnerRef,
    ) -> DataSnapshot {
        DataSnapshot::new(DataSnapshotInput {
            data_snapshot_id: id(suffix),
            owner: snapshot_owner,
            visible_at: time(as_of + 1),
            as_of: time(as_of),
            schema_hash: hash(10),
            manifest_hash: hash(11),
            blob_content_hash: content_hash,
            lineage: vec![LineageRef::versioned(id('I'), version(1))],
        })
        .unwrap()
    }

    fn scope() -> AccessScope {
        AccessScope::new(id('T'), id('A'), vec![id('O')]).unwrap()
    }

    fn owner() -> OwnerRef {
        OwnerRef::new(id('T'), id('O'))
    }

    fn reference(suffix: char) -> VersionRef {
        VersionRef::new(id(suffix), version(1))
    }

    fn version(value: u64) -> Version {
        Version::new(value).unwrap()
    }

    fn id(suffix: char) -> Ulid {
        let suffix = match suffix {
            'I' => 'J',
            'O' => 'Q',
            'U' => 'W',
            value => value,
        };
        Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
    }

    fn time(hour: u32) -> MarketTime {
        MarketTime::new(
            format!("2026-03-04T{hour:02}:00:00Z").parse().unwrap(),
            "Asia/Shanghai",
            "2026-03-04".parse().unwrap(),
        )
        .unwrap()
    }

    fn hash(value: u8) -> ContentHash {
        ContentHash::digest(&[value])
    }

    fn assert_lineage(error: &crate::ApplicationError) {
        assert_eq!(
            error.category(),
            crate::ApplicationErrorCategory::LineageIncomplete
        );
        assert!(!error.retryable());
    }
}
