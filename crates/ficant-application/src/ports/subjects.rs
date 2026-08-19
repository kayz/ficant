use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ficant_domain::DomainErrorCode;
use ficant_domain::governance::{
    ChangeJustification, FoundationChangeOperation, FoundationChangeRecord,
    FoundationChangeRecordInput, FoundationResourceKind, FoundationResourceRef, PlatformRole,
};
use ficant_domain::primitives::{ContentHash, OwnerRef, Ulid, VersionRef};
use ficant_domain::subject::{FundingTier, SubjectRecord, SubjectStateSnapshot};

use super::fingerprint::{
    FingerprintBuilder, OperationFingerprint, owner_bytes, version_ref_bytes,
};
use super::{AccessScope, ApplicationResult, FoundationChangeContext, IdempotencyKey};
use crate::{ApplicationError, ApplicationErrorCategory, map_domain_error};

pub const SUBJECT_READ_SCOPE: &str = "registry:read";
pub const SUBJECT_WRITE_SCOPE: &str = "registry:write";

/// One complete Subject version append bound to administrator identity and change evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GovernedRegisterSubject {
    change_context: FoundationChangeContext,
    value: SubjectRecord,
    idempotency_key: IdempotencyKey,
    fingerprint: OperationFingerprint,
}

impl GovernedRegisterSubject {
    /// Creates a fail-closed governed Subject registration.
    ///
    /// # Errors
    ///
    /// Returns forbidden for role/scope/owner drift and validation failure for a legacy owner-less
    /// Subject.
    pub fn new(
        change_context: FoundationChangeContext,
        value: SubjectRecord,
        idempotency_key: IdempotencyKey,
    ) -> ApplicationResult<Self> {
        let owner = required_subject_owner(&value)?;
        change_context.principal().authorize_mutation(
            PlatformRole::PlatformAdmin,
            SUBJECT_WRITE_SCOPE,
            owner,
        )?;
        let mut canonical = FingerprintBuilder::new("register-subject/v2");
        canonical.field(
            2,
            change_context
                .principal()
                .fingerprint()
                .content_hash()
                .as_bytes(),
        );
        canonical.field(3, &subject_record_bytes(&value)?);
        canonical.field(4, &change_bytes(change_context.change()));
        let fingerprint = canonical.finish();
        Ok(Self {
            change_context,
            value,
            idempotency_key,
            fingerprint,
        })
    }

    #[must_use]
    pub fn change_context(&self) -> &FoundationChangeContext {
        &self.change_context
    }

    #[must_use]
    pub fn scope(&self) -> &AccessScope {
        self.change_context.principal().access_scope()
    }

    #[must_use]
    pub fn value(&self) -> &SubjectRecord {
        &self.value
    }

    #[must_use]
    pub fn idempotency_key(&self) -> &IdempotencyKey {
        &self.idempotency_key
    }

    #[must_use]
    pub fn fingerprint(&self) -> &OperationFingerprint {
        &self.fingerprint
    }

    /// Materializes the append-only audit record after storage resolves the previous version.
    ///
    /// # Errors
    ///
    /// Returns validation failure when the Subject lost its exact owner evidence.
    pub fn change_record(
        &self,
        before_hash: Option<ContentHash>,
    ) -> ApplicationResult<FoundationChangeRecord> {
        FoundationChangeRecord::new(FoundationChangeRecordInput {
            record_id: self.change_context.record_id().clone(),
            actor_id: self.change_context.principal().actor_id().clone(),
            owner: required_subject_owner(&self.value)?.clone(),
            active_role: PlatformRole::PlatformAdmin,
            operation: FoundationChangeOperation::RegisterSubject,
            resource: FoundationResourceRef::versioned(
                FoundationResourceKind::Subject,
                self.value.version().reference().clone(),
            ),
            before_hash,
            after_hash: subject_record_content_hash(&self.value)?,
            change: self.change_context.change().clone(),
            request_fingerprint: self.fingerprint.content_hash().clone(),
            occurred_at: self.change_context.occurred_at().clone(),
            authorization_ref: None,
        })
        .map_err(map_domain_error)
    }
}

/// One immutable Subject state publication bound to administrator identity and change evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GovernedPublishSubjectState {
    change_context: FoundationChangeContext,
    value: SubjectStateSnapshot,
    idempotency_key: IdempotencyKey,
    fingerprint: OperationFingerprint,
}

impl GovernedPublishSubjectState {
    /// Creates a fail-closed governed Subject state publication.
    ///
    /// # Errors
    ///
    /// Returns forbidden for role/scope/owner drift and validation failure for a legacy owner-less
    /// snapshot.
    pub fn new(
        change_context: FoundationChangeContext,
        value: SubjectStateSnapshot,
        idempotency_key: IdempotencyKey,
    ) -> ApplicationResult<Self> {
        let owner = required_state_owner(&value)?;
        change_context.principal().authorize_mutation(
            PlatformRole::PlatformAdmin,
            SUBJECT_WRITE_SCOPE,
            owner,
        )?;
        let mut canonical = FingerprintBuilder::new("publish-subject-state/v2");
        canonical.field(
            2,
            change_context
                .principal()
                .fingerprint()
                .content_hash()
                .as_bytes(),
        );
        canonical.field(3, &subject_state_bytes(&value)?);
        canonical.field(4, &change_bytes(change_context.change()));
        let fingerprint = canonical.finish();
        Ok(Self {
            change_context,
            value,
            idempotency_key,
            fingerprint,
        })
    }

    #[must_use]
    pub fn change_context(&self) -> &FoundationChangeContext {
        &self.change_context
    }

    #[must_use]
    pub fn scope(&self) -> &AccessScope {
        self.change_context.principal().access_scope()
    }

    #[must_use]
    pub fn value(&self) -> &SubjectStateSnapshot {
        &self.value
    }

    #[must_use]
    pub fn idempotency_key(&self) -> &IdempotencyKey {
        &self.idempotency_key
    }

    #[must_use]
    pub fn fingerprint(&self) -> &OperationFingerprint {
        &self.fingerprint
    }

    /// Materializes the append-only audit record for the immutable state snapshot.
    ///
    /// # Errors
    ///
    /// Returns validation failure when the snapshot lost its exact owner evidence.
    pub fn change_record(&self) -> ApplicationResult<FoundationChangeRecord> {
        FoundationChangeRecord::new(FoundationChangeRecordInput {
            record_id: self.change_context.record_id().clone(),
            actor_id: self.change_context.principal().actor_id().clone(),
            owner: required_state_owner(&self.value)?.clone(),
            active_role: PlatformRole::PlatformAdmin,
            operation: FoundationChangeOperation::PublishSubjectState,
            resource: FoundationResourceRef::unversioned(
                FoundationResourceKind::SubjectState,
                self.value.id().clone(),
            ),
            before_hash: None,
            after_hash: subject_state_content_hash(&self.value)?,
            change: self.change_context.change().clone(),
            request_fingerprint: self.fingerprint.content_hash().clone(),
            occurred_at: self.change_context.occurred_at().clone(),
            authorization_ref: None,
        })
        .map_err(map_domain_error)
    }
}

/// Computes the canonical immutable identity for an owner-bound Subject record.
///
/// # Errors
///
/// Returns validation failure for a legacy owner-less record.
pub fn subject_record_content_hash(value: &SubjectRecord) -> ApplicationResult<ContentHash> {
    Ok(ContentHash::digest(&subject_record_bytes(value)?))
}

/// Computes the canonical immutable identity for an owner-bound Subject state snapshot.
///
/// # Errors
///
/// Returns validation failure for a legacy owner-less snapshot.
pub fn subject_state_content_hash(value: &SubjectStateSnapshot) -> ApplicationResult<ContentHash> {
    Ok(ContentHash::digest(&subject_state_bytes(value)?))
}

#[async_trait]
pub trait SubjectRepository: Send + Sync {
    /// Atomically appends a Subject version and its `FoundationChange` record.
    async fn register_governed_subject(
        &self,
        _command: GovernedRegisterSubject,
    ) -> ApplicationResult<SubjectRecord> {
        Err(fail_closed())
    }

    /// Atomically publishes a Subject state and its `FoundationChange` record.
    async fn publish_governed_subject_state(
        &self,
        _command: GovernedPublishSubjectState,
    ) -> ApplicationResult<SubjectStateSnapshot> {
        Err(fail_closed())
    }

    async fn register_subject(&self, value: SubjectRecord) -> ApplicationResult<SubjectRecord>;

    async fn get_subject(&self, reference: VersionRef) -> ApplicationResult<Option<SubjectRecord>>;

    async fn register_subject_state(
        &self,
        value: SubjectStateSnapshot,
    ) -> ApplicationResult<SubjectStateSnapshot>;

    async fn get_subject_state(
        &self,
        snapshot_id: Ulid,
        knowledge_at: DateTime<Utc>,
    ) -> ApplicationResult<Option<SubjectStateSnapshot>>;

    /// Reads one exact Subject only after applying the request principal's owner boundary.
    async fn get_subject_scoped(
        &self,
        scope: &AccessScope,
        reference: VersionRef,
    ) -> ApplicationResult<Option<SubjectRecord>> {
        let value = self.get_subject(reference).await?;
        if let Some(value) = value.as_ref() {
            scope.authorize(required_subject_owner(value)?)?;
        }
        Ok(value)
    }

    /// Reads one visible Subject state only after applying the request principal's owner boundary.
    async fn get_subject_state_scoped(
        &self,
        scope: &AccessScope,
        snapshot_id: Ulid,
        knowledge_at: DateTime<Utc>,
    ) -> ApplicationResult<Option<SubjectStateSnapshot>> {
        let value = self.get_subject_state(snapshot_id, knowledge_at).await?;
        if let Some(value) = value.as_ref() {
            scope.authorize(required_state_owner(value)?)?;
        }
        Ok(value)
    }
}

fn subject_record_bytes(value: &SubjectRecord) -> ApplicationResult<Vec<u8>> {
    let owner = required_subject_owner(value)?;
    let version = value.version();
    let mut canonical = FingerprintBuilder::new("subject-record/v1");
    canonical.field(2, value.subject().id().as_str().as_bytes());
    canonical.field(3, &owner_bytes(owner));
    canonical.field(4, value.subject().display_name().as_bytes());
    canonical.field(5, &version_ref_bytes(version.reference()));
    canonical.u64(
        6,
        u64::try_from(version.access_set().market_codes().len()).map_err(|_| validation())?,
    );
    for market in version.access_set().market_codes() {
        canonical.field(7, market.as_bytes());
    }
    canonical.u64(
        8,
        u64::try_from(version.access_set().tool_codes().len()).map_err(|_| validation())?,
    );
    for tool in version.access_set().tool_codes() {
        canonical.field(9, tool.as_bytes());
    }
    canonical.field(
        10,
        &[match version.funding_tier() {
            FundingTier::DrAvailable => 1,
            FundingTier::ROnly => 2,
        }],
    );
    canonical.field(
        11,
        version.tax_treatment().value_added_tax_profile().as_bytes(),
    );
    canonical.field(12, version.tax_treatment().income_tax_profile().as_bytes());
    canonical.field(13, version.assessment_mechanism().as_bytes());
    canonical.field(14, version.liability_profile().as_bytes());
    match version.constraint_set_ref() {
        Some(reference) => {
            canonical.field(15, &[1]);
            canonical.field(16, &version_ref_bytes(reference.reference()));
        }
        None => {
            canonical.field(15, &[0]);
        }
    }
    Ok(canonical.into_bytes())
}

fn subject_state_bytes(value: &SubjectStateSnapshot) -> ApplicationResult<Vec<u8>> {
    let owner = required_state_owner(value)?;
    let mut canonical = FingerprintBuilder::new("subject-state-snapshot/v1");
    canonical.field(2, value.id().as_str().as_bytes());
    canonical.field(3, &owner_bytes(owner));
    canonical.field(4, &version_ref_bytes(value.subject_ref()));
    canonical.field(5, value.net_capital().coefficient().as_bytes());
    canonical.u64(6, u64::from(value.net_capital().scale()));
    canonical.field(
        7,
        &version_ref_bytes(&VersionRef::new(
            value.net_capital().unit().unit_id().clone(),
            value.net_capital().unit().version(),
        )),
    );
    canonical.u64(
        8,
        u64::try_from(value.limit_ceilings().len()).map_err(|_| validation())?,
    );
    for ceiling in value.limit_ceilings() {
        canonical.field(9, ceiling.limit_code().as_bytes());
        canonical.field(10, ceiling.ceiling().coefficient().as_bytes());
        canonical.u64(11, u64::from(ceiling.ceiling().scale()));
        canonical.field(
            12,
            &version_ref_bytes(&VersionRef::new(
                ceiling.ceiling().unit().unit_id().clone(),
                ceiling.ceiling().unit().version(),
            )),
        );
    }
    canonical.field(13, &value.observed_at().timestamp().to_be_bytes());
    canonical.field(
        14,
        &value.observed_at().timestamp_subsec_nanos().to_be_bytes(),
    );
    canonical.field(15, &value.visible_at().timestamp().to_be_bytes());
    canonical.field(
        16,
        &value.visible_at().timestamp_subsec_nanos().to_be_bytes(),
    );
    canonical.field(17, value.market_timezone().as_bytes());
    Ok(canonical.into_bytes())
}

fn change_bytes(change: &ChangeJustification) -> Vec<u8> {
    let mut canonical = FingerprintBuilder::new("change-justification/v1");
    canonical.field(2, change.reason().as_bytes());
    canonical.u64(
        3,
        u64::try_from(change.sources().len()).expect("source count fits u64"),
    );
    for source in change.sources() {
        canonical.field(4, source.uri().as_bytes());
        canonical.field(5, source.sha256().as_bytes());
    }
    canonical.into_bytes()
}

fn required_subject_owner(value: &SubjectRecord) -> ApplicationResult<&OwnerRef> {
    value.subject().owner().ok_or_else(validation)
}

fn required_state_owner(value: &SubjectStateSnapshot) -> ApplicationResult<&OwnerRef> {
    value.owner().ok_or_else(validation)
}

fn validation() -> ApplicationError {
    map_domain_error(DomainErrorCode::InvalidValue)
}

fn fail_closed() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::StateConflict, false)
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveDate, TimeZone, Utc};
    use ficant_domain::governance::{ChangeJustification, SourceDocumentRef};
    use ficant_domain::primitives::{MarketTime, Version};
    use ficant_domain::subject::{AccessSet, Subject, SubjectVersion, TaxTreatment};

    use super::*;
    use crate::ports::AuthorizedPrincipal;

    #[test]
    fn governed_subject_requires_exact_owner_admin_scope_and_binds_change() {
        let owner = OwnerRef::new(id('T'), id('P'));
        let admin_principal = principal(PlatformRole::PlatformAdmin, &owner);
        let value = subject_record(Some(owner.clone()));
        let command = GovernedRegisterSubject::new(
            context(admin_principal, "register subject", 'R'),
            value.clone(),
            IdempotencyKey::new("subject-v1").unwrap(),
        )
        .unwrap();
        let record = command.change_record(None).unwrap();
        assert_eq!(record.owner(), &owner);
        assert_eq!(record.resource().version().unwrap().get(), 1);
        assert_eq!(
            record.after_hash(),
            &subject_record_content_hash(&value).unwrap()
        );

        let ownerless = subject_record(None);
        assert_eq!(
            GovernedRegisterSubject::new(
                context(
                    principal(PlatformRole::PlatformAdmin, &owner),
                    "owner missing",
                    'M',
                ),
                ownerless,
                IdempotencyKey::new("owner-missing").unwrap(),
            )
            .unwrap_err()
            .category(),
            ApplicationErrorCategory::ValidationFailed,
        );
        assert_eq!(
            FoundationChangeContext::administrator(
                principal(PlatformRole::Researcher, &owner),
                change("wrong role"),
                id('W'),
                market_time(),
            )
            .unwrap_err()
            .category(),
            ApplicationErrorCategory::Forbidden,
        );
    }

    #[test]
    fn governed_subject_state_requires_owner_and_binds_every_time_byte() {
        let owner = OwnerRef::new(id('T'), id('P'));
        let value = subject_state(Some(owner.clone()));
        let command = GovernedPublishSubjectState::new(
            context(
                principal(PlatformRole::PlatformAdmin, &owner),
                "publish state",
                'Q',
            ),
            value.clone(),
            IdempotencyKey::new("state-v1").unwrap(),
        )
        .unwrap();
        let record = command.change_record().unwrap();
        assert_eq!(record.owner(), &owner);
        assert_eq!(
            record.after_hash(),
            &subject_state_content_hash(&value).unwrap()
        );
        assert_eq!(
            GovernedPublishSubjectState::new(
                context(
                    principal(PlatformRole::PlatformAdmin, &owner),
                    "missing owner",
                    'N',
                ),
                subject_state(None),
                IdempotencyKey::new("state-owner-missing").unwrap(),
            )
            .unwrap_err()
            .category(),
            ApplicationErrorCategory::ValidationFailed,
        );
    }

    fn subject_record(owner: Option<OwnerRef>) -> SubjectRecord {
        let subject_id = id('S');
        let subject = owner.map_or_else(
            || Subject::new(subject_id.clone(), "Subject").unwrap(),
            |owner| Subject::new_owned(subject_id.clone(), owner, "Subject").unwrap(),
        );
        SubjectRecord::new(
            subject,
            SubjectVersion::new(
                VersionRef::new(subject_id, Version::new(1).unwrap()),
                AccessSet::new(["CGB"], ["rates"]).unwrap(),
                FundingTier::DrAvailable,
                TaxTreatment::new("vat", "income").unwrap(),
                "daily",
                "general",
                None,
            )
            .unwrap(),
        )
        .unwrap()
    }

    fn subject_state(owner: Option<OwnerRef>) -> SubjectStateSnapshot {
        use ficant_domain::primitives::{DecimalValue, UnitRef};

        let instant = Utc.with_ymd_and_hms(2026, 8, 18, 1, 2, 3).unwrap();
        let capital =
            DecimalValue::new("1000", 0, UnitRef::new(id('V'), Version::new(1).unwrap())).unwrap();
        match owner {
            None => SubjectStateSnapshot::new(
                id('J'),
                VersionRef::new(id('S'), Version::new(1).unwrap()),
                capital.clone(),
                Vec::new(),
                instant,
                instant,
                "UTC",
            )
            .unwrap(),
            Some(owner) => SubjectStateSnapshot::new_owned(
                id('J'),
                VersionRef::new(id('S'), Version::new(1).unwrap()),
                capital,
                Vec::new(),
                instant,
                instant,
                "UTC",
                owner,
            )
            .unwrap(),
        }
    }

    fn context(
        principal: AuthorizedPrincipal,
        reason: &str,
        record_suffix: char,
    ) -> FoundationChangeContext {
        FoundationChangeContext::administrator(
            principal,
            change(reason),
            id(record_suffix),
            market_time(),
        )
        .unwrap()
    }

    fn change(reason: &str) -> ChangeJustification {
        ChangeJustification::new(
            reason,
            vec![
                SourceDocumentRef::new("urn:test:subject", ContentHash::digest(b"source")).unwrap(),
            ],
        )
        .unwrap()
    }

    fn market_time() -> MarketTime {
        MarketTime::new(
            Utc.with_ymd_and_hms(2026, 8, 18, 0, 0, 0).unwrap(),
            "UTC",
            NaiveDate::from_ymd_opt(2026, 8, 18).unwrap(),
        )
        .unwrap()
    }

    fn principal(role: PlatformRole, owner: &OwnerRef) -> AuthorizedPrincipal {
        AuthorizedPrincipal::new(
            "subject-test".to_owned(),
            id('A'),
            owner.tenant_id().clone(),
            vec![owner.owner_id().clone()],
            role,
            vec![SUBJECT_WRITE_SCOPE.to_owned()],
            ContentHash::digest(b"credential"),
        )
        .unwrap()
    }

    fn id(suffix: char) -> Ulid {
        Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
    }
}
