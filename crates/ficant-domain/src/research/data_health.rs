use std::cmp::Ordering;

use crate::market::PriceSourceType;
use crate::primitives::{ContentHash, LineageRef, MarketTime, OwnerRef, Ulid, Version, VersionRef};
use crate::{ContentAddressed, DomainErrorCode, DomainResult, Lineaged};

use super::{AccountingClassificationState, CoverageDeclaration, PositionSnapshot};

const BASIS_POINTS_DENOMINATOR: u64 = 10_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataHealthThresholdProfile {
    profile_ref: VersionRef,
    max_position_snapshot_age_seconds: u64,
    unknown_accounting_warning_basis_points: u32,
    max_data_snapshot_age_seconds: u64,
    model_valuation_warning_basis_points: u32,
    content_hash: ContentHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataHealthThresholdProfileInput {
    pub profile_ref: VersionRef,
    pub max_position_snapshot_age_seconds: u64,
    pub unknown_accounting_warning_basis_points: u32,
    pub max_data_snapshot_age_seconds: u64,
    pub model_valuation_warning_basis_points: u32,
    pub content_hash: ContentHash,
}

impl DataHealthThresholdProfile {
    pub fn new(input: DataHealthThresholdProfileInput) -> DomainResult<Self> {
        if !(1..=10_000).contains(&input.unknown_accounting_warning_basis_points)
            || !(1..=10_000).contains(&input.model_valuation_warning_basis_points)
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        let actual = Self::content_hash_for(&input);
        if actual != input.content_hash {
            return Err(DomainErrorCode::ContentHashMismatch);
        }
        Ok(Self {
            profile_ref: input.profile_ref,
            max_position_snapshot_age_seconds: input.max_position_snapshot_age_seconds,
            unknown_accounting_warning_basis_points: input.unknown_accounting_warning_basis_points,
            max_data_snapshot_age_seconds: input.max_data_snapshot_age_seconds,
            model_valuation_warning_basis_points: input.model_valuation_warning_basis_points,
            content_hash: input.content_hash,
        })
    }

    pub fn content_hash_for(input: &DataHealthThresholdProfileInput) -> ContentHash {
        ContentHash::digest(&profile_canonical_bytes(input))
    }

    pub fn canonical_bytes(&self) -> Vec<u8> {
        profile_canonical_bytes(&DataHealthThresholdProfileInput {
            profile_ref: self.profile_ref.clone(),
            max_position_snapshot_age_seconds: self.max_position_snapshot_age_seconds,
            unknown_accounting_warning_basis_points: self.unknown_accounting_warning_basis_points,
            max_data_snapshot_age_seconds: self.max_data_snapshot_age_seconds,
            model_valuation_warning_basis_points: self.model_valuation_warning_basis_points,
            content_hash: self.content_hash.clone(),
        })
    }

    pub fn profile_ref(&self) -> &VersionRef {
        &self.profile_ref
    }

    pub const fn max_position_snapshot_age_seconds(&self) -> u64 {
        self.max_position_snapshot_age_seconds
    }

    pub const fn unknown_accounting_warning_basis_points(&self) -> u32 {
        self.unknown_accounting_warning_basis_points
    }

    pub const fn max_data_snapshot_age_seconds(&self) -> u64 {
        self.max_data_snapshot_age_seconds
    }

    pub const fn model_valuation_warning_basis_points(&self) -> u32 {
        self.model_valuation_warning_basis_points
    }
}

impl ContentAddressed for DataHealthThresholdProfile {
    fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataHealthState {
    Healthy,
    Warning,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PositionSetState {
    NonEmpty,
    VerifiedEmpty,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DataHealthIssueCode {
    EmptyPositions,
    UnknownAccountingClassification,
    StalePositionSnapshot,
    UntypedPriceSource,
    ModelValuationShare,
    StaleDataSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataHealthIssue {
    code: DataHealthIssueCode,
    affected_position_ids: Vec<Ulid>,
    data_source_ref: Option<VersionRef>,
    record_count: u64,
    ratio_basis_points: u32,
    observed_age_seconds: u64,
}

impl DataHealthIssue {
    fn new(
        code: DataHealthIssueCode,
        affected_position_ids: Vec<Ulid>,
        data_source_ref: Option<VersionRef>,
        record_count: u64,
        ratio_basis_points: u32,
        observed_age_seconds: u64,
    ) -> DomainResult<Self> {
        if affected_position_ids
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        match code {
            DataHealthIssueCode::EmptyPositions => {
                if !affected_position_ids.is_empty()
                    || data_source_ref.is_some()
                    || record_count != 0
                    || ratio_basis_points != 0
                    || observed_age_seconds != 0
                {
                    return Err(DomainErrorCode::InvalidValue);
                }
            }
            DataHealthIssueCode::UnknownAccountingClassification => {
                if affected_position_ids.is_empty()
                    || data_source_ref.is_some()
                    || record_count
                        != u64::try_from(affected_position_ids.len())
                            .map_err(|_| DomainErrorCode::InvalidValue)?
                    || ratio_basis_points == 0
                    || observed_age_seconds != 0
                {
                    return Err(DomainErrorCode::InvalidValue);
                }
            }
            DataHealthIssueCode::StalePositionSnapshot => {
                if !affected_position_ids.is_empty()
                    || data_source_ref.is_some()
                    || record_count != 0
                    || ratio_basis_points != 0
                    || observed_age_seconds == 0
                {
                    return Err(DomainErrorCode::InvalidValue);
                }
            }
            DataHealthIssueCode::UntypedPriceSource => {
                if !affected_position_ids.is_empty()
                    || data_source_ref.is_none()
                    || record_count == 0
                    || ratio_basis_points != 0
                    || observed_age_seconds != 0
                {
                    return Err(DomainErrorCode::InvalidValue);
                }
            }
            DataHealthIssueCode::ModelValuationShare => {
                if !affected_position_ids.is_empty()
                    || data_source_ref.is_none()
                    || record_count == 0
                    || ratio_basis_points == 0
                    || observed_age_seconds != 0
                {
                    return Err(DomainErrorCode::InvalidValue);
                }
            }
            DataHealthIssueCode::StaleDataSnapshot => {
                if !affected_position_ids.is_empty()
                    || data_source_ref.is_none()
                    || record_count != 0
                    || ratio_basis_points != 0
                    || observed_age_seconds == 0
                {
                    return Err(DomainErrorCode::InvalidValue);
                }
            }
        }
        Ok(Self {
            code,
            affected_position_ids,
            data_source_ref,
            record_count,
            ratio_basis_points,
            observed_age_seconds,
        })
    }

    pub const fn code(&self) -> DataHealthIssueCode {
        self.code
    }

    pub fn affected_position_ids(&self) -> &[Ulid] {
        &self.affected_position_ids
    }

    pub fn data_source_ref(&self) -> Option<&VersionRef> {
        self.data_source_ref.as_ref()
    }

    pub const fn record_count(&self) -> u64 {
        self.record_count
    }

    pub const fn ratio_basis_points(&self) -> u32 {
        self.ratio_basis_points
    }

    pub const fn observed_age_seconds(&self) -> u64 {
        self.observed_age_seconds
    }
}

/// Capability proving that an immutable snapshot was hash-verified and empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedEmptyPositionSnapshot {
    content_hash: ContentHash,
}

impl VerifiedEmptyPositionSnapshot {
    pub fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PositionHealthEvaluation {
    position_snapshot_hash: ContentHash,
    evaluated_at: MarketTime,
    threshold_profile_ref: VersionRef,
    threshold_profile_hash: ContentHash,
    position_set_state: PositionSetState,
    issues: Vec<DataHealthIssue>,
    coverage: CoverageDeclaration,
}

impl PositionHealthEvaluation {
    pub fn position_snapshot_hash(&self) -> &ContentHash {
        &self.position_snapshot_hash
    }

    pub const fn position_set_state(&self) -> PositionSetState {
        self.position_set_state
    }

    pub fn issues(&self) -> &[DataHealthIssue] {
        &self.issues
    }

    pub fn coverage(&self) -> &CoverageDeclaration {
        &self.coverage
    }
}

pub fn evaluate_position_snapshot(
    snapshot: &PositionSnapshot,
    profile: &DataHealthThresholdProfile,
    evaluated_at: &MarketTime,
) -> DomainResult<PositionHealthEvaluation> {
    if ContentHash::digest(&snapshot.canonical_payload()) != *snapshot.content_hash() {
        return Err(DomainErrorCode::ContentHashMismatch);
    }
    if evaluated_at.instant() < snapshot.visible_at().instant()
        || evaluated_at.instant() < snapshot.observed_at().instant()
    {
        return Err(DomainErrorCode::InvalidEffectiveTime);
    }

    let mut issues = Vec::new();
    let (position_set_state, coverage) = if snapshot.positions().is_empty() {
        issues.push(DataHealthIssue::new(
            DataHealthIssueCode::EmptyPositions,
            Vec::new(),
            None,
            0,
            0,
            0,
        )?);
        let verified = VerifiedEmptyPositionSnapshot {
            content_hash: snapshot.content_hash().clone(),
        };
        (
            PositionSetState::VerifiedEmpty,
            CoverageDeclaration::for_verified_empty(&verified),
        )
    } else {
        let position_ids = snapshot
            .positions()
            .iter()
            .map(|position| position.id().clone())
            .collect::<Vec<_>>();
        let unknown_ids = snapshot
            .positions()
            .iter()
            .filter(|position| {
                position.accounting_classification().state()
                    == AccountingClassificationState::Unknown
            })
            .map(|position| position.id().clone())
            .collect::<Vec<_>>();
        if !unknown_ids.is_empty()
            && ratio_reaches_threshold(
                unknown_ids.len(),
                snapshot.positions().len(),
                profile.unknown_accounting_warning_basis_points,
            )?
        {
            issues.push(DataHealthIssue::new(
                DataHealthIssueCode::UnknownAccountingClassification,
                unknown_ids.clone(),
                None,
                u64::try_from(unknown_ids.len()).map_err(|_| DomainErrorCode::InvalidValue)?,
                ratio_basis_points(unknown_ids.len(), snapshot.positions().len())?,
                0,
            )?);
        }
        (
            PositionSetState::NonEmpty,
            CoverageDeclaration::for_complete_positions(
                snapshot.positions(),
                &position_ids,
                None,
                0,
            )?,
        )
    };

    let age = elapsed_seconds(snapshot.observed_at(), evaluated_at)?;
    if age > profile.max_position_snapshot_age_seconds {
        issues.push(DataHealthIssue::new(
            DataHealthIssueCode::StalePositionSnapshot,
            Vec::new(),
            None,
            0,
            0,
            age,
        )?);
    }
    issues.sort_by(compare_issues);

    Ok(PositionHealthEvaluation {
        position_snapshot_hash: snapshot.content_hash().clone(),
        evaluated_at: evaluated_at.clone(),
        threshold_profile_ref: profile.profile_ref().clone(),
        threshold_profile_hash: profile.content_hash().clone(),
        position_set_state,
        issues,
        coverage,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataHealthPriceEvidence {
    data_snapshot_id: Ulid,
    owner: OwnerRef,
    data_snapshot_content_hash: ContentHash,
    data_snapshot_manifest_hash: ContentHash,
    data_source_ref: VersionRef,
    source_type: Option<PriceSourceType>,
    record_count: u64,
    visible_at: MarketTime,
    as_of: MarketTime,
    lineage: Vec<LineageRef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataHealthPriceEvidenceInput {
    pub data_snapshot_id: Ulid,
    pub owner: OwnerRef,
    pub data_snapshot_content_hash: ContentHash,
    pub data_snapshot_manifest_hash: ContentHash,
    pub data_source_ref: VersionRef,
    pub source_type: Option<PriceSourceType>,
    pub record_count: u64,
    pub visible_at: MarketTime,
    pub as_of: MarketTime,
    pub lineage: Vec<LineageRef>,
}

impl DataHealthPriceEvidence {
    pub fn new(input: DataHealthPriceEvidenceInput) -> DomainResult<Self> {
        if input.record_count == 0 || input.lineage.is_empty() {
            return Err(DomainErrorCode::InvalidValue);
        }
        if input.visible_at.instant() < input.as_of.instant() {
            return Err(DomainErrorCode::InvalidEffectiveTime);
        }
        Ok(Self {
            data_snapshot_id: input.data_snapshot_id,
            owner: input.owner,
            data_snapshot_content_hash: input.data_snapshot_content_hash,
            data_snapshot_manifest_hash: input.data_snapshot_manifest_hash,
            data_source_ref: input.data_source_ref,
            source_type: input.source_type,
            record_count: input.record_count,
            visible_at: input.visible_at,
            as_of: input.as_of,
            lineage: input.lineage,
        })
    }

    pub fn data_snapshot_id(&self) -> &Ulid {
        &self.data_snapshot_id
    }

    pub fn data_snapshot_content_hash(&self) -> &ContentHash {
        &self.data_snapshot_content_hash
    }

    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    pub fn data_snapshot_manifest_hash(&self) -> &ContentHash {
        &self.data_snapshot_manifest_hash
    }

    pub fn data_source_ref(&self) -> &VersionRef {
        &self.data_source_ref
    }

    pub const fn source_type(&self) -> Option<PriceSourceType> {
        self.source_type
    }

    pub const fn record_count(&self) -> u64 {
        self.record_count
    }

    pub fn as_of(&self) -> &MarketTime {
        &self.as_of
    }

    pub fn visible_at(&self) -> &MarketTime {
        &self.visible_at
    }
}

impl Lineaged for DataHealthPriceEvidence {
    fn lineage(&self) -> &[LineageRef] {
        &self.lineage
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataHealthReport {
    owner: OwnerRef,
    subject_ref: VersionRef,
    evaluated_at: MarketTime,
    position_snapshot_id: Ulid,
    position_snapshot_hash: ContentHash,
    data_snapshot_id: Option<Ulid>,
    data_snapshot_manifest_hash: Option<ContentHash>,
    data_source_ref: Option<VersionRef>,
    threshold_profile: DataHealthThresholdProfile,
    state: DataHealthState,
    issues: Vec<DataHealthIssue>,
    price_evidence_evaluated: bool,
    position_set_state: PositionSetState,
    coverage: CoverageDeclaration,
    request_fingerprint: ContentHash,
    content_hash: ContentHash,
    lineage: Vec<LineageRef>,
}

pub struct DataHealthReportInput {
    pub position_snapshot: PositionSnapshot,
    pub evaluated_at: MarketTime,
    pub position_evaluation: PositionHealthEvaluation,
    pub threshold_profile: DataHealthThresholdProfile,
    pub price_evidence: Option<DataHealthPriceEvidence>,
}

impl DataHealthReport {
    pub fn new(input: DataHealthReportInput) -> DomainResult<Self> {
        if input.position_snapshot.lineage().is_empty() {
            return Err(DomainErrorCode::BrokenLineage);
        }
        if input.position_evaluation.position_snapshot_hash
            != *input.position_snapshot.content_hash()
            || input.position_evaluation.evaluated_at != input.evaluated_at
            || input.position_evaluation.threshold_profile_ref
                != *input.threshold_profile.profile_ref()
            || input.position_evaluation.threshold_profile_hash
                != *input.threshold_profile.content_hash()
        {
            return Err(DomainErrorCode::ContentHashMismatch);
        }
        let mut issues = input.position_evaluation.issues.clone();
        let (data_snapshot_id, data_snapshot_manifest_hash, data_source_ref) =
            append_price_evidence_issues(
                input.price_evidence.as_ref(),
                input.position_snapshot.owner(),
                &input.evaluated_at,
                &input.threshold_profile,
                &mut issues,
            )?;
        issues.sort_by(compare_issues);
        let state = if issues.is_empty() {
            DataHealthState::Healthy
        } else {
            DataHealthState::Warning
        };
        let profile_ref = LineageRef::new(
            input.threshold_profile.profile_ref().id().clone(),
            Some(input.threshold_profile.profile_ref().version()),
            Some(input.threshold_profile.content_hash().clone()),
        )?;
        let mut lineage = input.position_snapshot.lineage().to_vec();
        lineage.push(LineageRef::content_addressed(
            input.position_snapshot.id().clone(),
            input.position_snapshot.content_hash().clone(),
        ));
        if let Some(evidence) = input.price_evidence.as_ref() {
            lineage.extend_from_slice(evidence.lineage());
            lineage.push(LineageRef::content_addressed(
                evidence.data_snapshot_id().clone(),
                evidence.data_snapshot_content_hash().clone(),
            ));
        }
        if !lineage.iter().any(|reference| reference == &profile_ref) {
            lineage.push(profile_ref);
        }
        lineage.sort_by(compare_lineage);
        lineage.dedup();
        let request_fingerprint = data_health_request_fingerprint(
            input.position_snapshot.subject_ref(),
            input.position_snapshot.id(),
            data_snapshot_id.as_ref(),
            &input.evaluated_at,
            &input.threshold_profile,
        );
        let mut report = Self {
            owner: input.position_snapshot.owner().clone(),
            subject_ref: input.position_snapshot.subject_ref().clone(),
            evaluated_at: input.evaluated_at,
            position_snapshot_id: input.position_snapshot.id().clone(),
            position_snapshot_hash: input.position_evaluation.position_snapshot_hash,
            data_snapshot_id,
            data_snapshot_manifest_hash,
            data_source_ref,
            threshold_profile: input.threshold_profile,
            state,
            issues,
            price_evidence_evaluated: input.price_evidence.is_some(),
            position_set_state: input.position_evaluation.position_set_state,
            coverage: input.position_evaluation.coverage,
            request_fingerprint,
            content_hash: ContentHash::digest(b"pending"),
            lineage,
        };
        report.validate_position_pair()?;
        report.content_hash = ContentHash::digest(&report.canonical_bytes());
        Ok(report)
    }

    fn validate_position_pair(&self) -> DomainResult<()> {
        match self.position_set_state {
            PositionSetState::VerifiedEmpty => {
                if self.coverage.imported_position_count() != 0
                    || self.coverage.participating_position_count() != 0
                    || !self
                        .coverage
                        .imported_gross_economic_value_by_unit()
                        .is_empty()
                    || !self
                        .coverage
                        .participating_gross_economic_value_by_unit()
                        .is_empty()
                {
                    return Err(DomainErrorCode::InvalidValue);
                }
            }
            PositionSetState::NonEmpty => {
                if self.coverage.imported_position_count() == 0
                    || self.coverage.participating_position_count() == 0
                {
                    return Err(DomainErrorCode::InvalidValue);
                }
            }
        }
        Ok(())
    }

    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }
    pub fn subject_ref(&self) -> &VersionRef {
        &self.subject_ref
    }
    pub fn evaluated_at(&self) -> &MarketTime {
        &self.evaluated_at
    }
    pub fn position_snapshot_id(&self) -> &Ulid {
        &self.position_snapshot_id
    }
    pub fn position_snapshot_hash(&self) -> &ContentHash {
        &self.position_snapshot_hash
    }
    pub fn data_snapshot_id(&self) -> Option<&Ulid> {
        self.data_snapshot_id.as_ref()
    }
    pub fn data_snapshot_manifest_hash(&self) -> Option<&ContentHash> {
        self.data_snapshot_manifest_hash.as_ref()
    }
    pub fn data_source_ref(&self) -> Option<&VersionRef> {
        self.data_source_ref.as_ref()
    }
    pub fn threshold_profile(&self) -> &DataHealthThresholdProfile {
        &self.threshold_profile
    }
    pub const fn state(&self) -> DataHealthState {
        self.state
    }
    pub fn issues(&self) -> &[DataHealthIssue] {
        &self.issues
    }
    pub const fn price_evidence_evaluated(&self) -> bool {
        self.price_evidence_evaluated
    }
    pub const fn position_set_state(&self) -> PositionSetState {
        self.position_set_state
    }
    pub fn coverage(&self) -> &CoverageDeclaration {
        &self.coverage
    }
    pub fn request_fingerprint(&self) -> &ContentHash {
        &self.request_fingerprint
    }

    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        append(&mut bytes, b"ficant.research.data-health-report.v1");
        append_owner(&mut bytes, &self.owner);
        append_version_ref(&mut bytes, &self.subject_ref);
        append_time(&mut bytes, &self.evaluated_at);
        append(&mut bytes, self.position_snapshot_id.as_str().as_bytes());
        append(&mut bytes, self.position_snapshot_hash.as_bytes());
        append_optional_id(&mut bytes, self.data_snapshot_id.as_ref());
        append_optional_hash(&mut bytes, self.data_snapshot_manifest_hash.as_ref());
        append_optional_version_ref(&mut bytes, self.data_source_ref.as_ref());
        append(&mut bytes, &self.threshold_profile.canonical_bytes());
        append(&mut bytes, &[health_state_code(self.state)]);
        for issue in &self.issues {
            append_issue(&mut bytes, issue);
        }
        append(&mut bytes, &[u8::from(self.price_evidence_evaluated)]);
        append(
            &mut bytes,
            &[position_set_state_code(self.position_set_state)],
        );
        append(&mut bytes, &self.coverage.canonical_bytes());
        append(&mut bytes, self.request_fingerprint.as_bytes());
        for reference in &self.lineage {
            append_lineage(&mut bytes, reference);
        }
        bytes
    }
}

impl ContentAddressed for DataHealthReport {
    fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

impl Lineaged for DataHealthReport {
    fn lineage(&self) -> &[LineageRef] {
        &self.lineage
    }
}

fn profile_canonical_bytes(input: &DataHealthThresholdProfileInput) -> Vec<u8> {
    let mut bytes = Vec::new();
    append(
        &mut bytes,
        b"ficant.research.data-health-threshold-profile.v1",
    );
    append_version_ref(&mut bytes, &input.profile_ref);
    append(
        &mut bytes,
        &input.max_position_snapshot_age_seconds.to_be_bytes(),
    );
    append(
        &mut bytes,
        &input.unknown_accounting_warning_basis_points.to_be_bytes(),
    );
    append(
        &mut bytes,
        &input.max_data_snapshot_age_seconds.to_be_bytes(),
    );
    append(
        &mut bytes,
        &input.model_valuation_warning_basis_points.to_be_bytes(),
    );
    bytes
}

fn data_health_request_fingerprint(
    subject_ref: &VersionRef,
    position_snapshot_id: &Ulid,
    data_snapshot_id: Option<&Ulid>,
    evaluated_at: &MarketTime,
    threshold_profile: &DataHealthThresholdProfile,
) -> ContentHash {
    let mut bytes = Vec::new();
    append(&mut bytes, b"ficant.operation.data-health-report.v1");
    append_version_ref(&mut bytes, subject_ref);
    append(&mut bytes, position_snapshot_id.as_str().as_bytes());
    append_optional_id(&mut bytes, data_snapshot_id);
    append_time(&mut bytes, evaluated_at);
    append(&mut bytes, &threshold_profile.canonical_bytes());
    append(&mut bytes, threshold_profile.content_hash().as_bytes());
    ContentHash::digest(&bytes)
}

fn ratio_reaches_threshold(count: usize, total: usize, threshold: u32) -> DomainResult<bool> {
    if total == 0 {
        return Err(DomainErrorCode::InvalidValue);
    }
    let count = u64::try_from(count).map_err(|_| DomainErrorCode::InvalidValue)?;
    let total = u64::try_from(total).map_err(|_| DomainErrorCode::InvalidValue)?;
    let left = count
        .checked_mul(BASIS_POINTS_DENOMINATOR)
        .ok_or(DomainErrorCode::InvalidValue)?;
    let right = total
        .checked_mul(u64::from(threshold))
        .ok_or(DomainErrorCode::InvalidValue)?;
    Ok(left >= right)
}

fn ratio_basis_points(count: usize, total: usize) -> DomainResult<u32> {
    let count = u64::try_from(count).map_err(|_| DomainErrorCode::InvalidValue)?;
    let total = u64::try_from(total).map_err(|_| DomainErrorCode::InvalidValue)?;
    let value = count
        .checked_mul(BASIS_POINTS_DENOMINATOR)
        .ok_or(DomainErrorCode::InvalidValue)?
        / total;
    u32::try_from(value).map_err(|_| DomainErrorCode::InvalidValue)
}

fn elapsed_seconds(earlier: &MarketTime, later: &MarketTime) -> DomainResult<u64> {
    let seconds = later
        .instant()
        .signed_duration_since(earlier.instant())
        .num_seconds();
    u64::try_from(seconds).map_err(|_| DomainErrorCode::InvalidEffectiveTime)
}

fn append_price_evidence_issues(
    evidence: Option<&DataHealthPriceEvidence>,
    snapshot_owner: &OwnerRef,
    evaluated_at: &MarketTime,
    profile: &DataHealthThresholdProfile,
    issues: &mut Vec<DataHealthIssue>,
) -> DomainResult<(Option<Ulid>, Option<ContentHash>, Option<VersionRef>)> {
    let Some(evidence) = evidence else {
        return Ok((None, None, None));
    };
    if evidence.owner() != snapshot_owner {
        return Err(DomainErrorCode::InvalidValue);
    }
    if evaluated_at.instant() < evidence.visible_at().instant() {
        return Err(DomainErrorCode::InvalidEffectiveTime);
    }
    let age = elapsed_seconds(evidence.as_of(), evaluated_at)?;
    if evidence.source_type().is_none() {
        issues.push(DataHealthIssue::new(
            DataHealthIssueCode::UntypedPriceSource,
            Vec::new(),
            Some(evidence.data_source_ref().clone()),
            evidence.record_count(),
            0,
            0,
        )?);
    }
    if evidence.source_type() == Some(PriceSourceType::ModelValuation)
        && ratio_reaches_threshold(
            usize::try_from(evidence.record_count()).map_err(|_| DomainErrorCode::InvalidValue)?,
            usize::try_from(evidence.record_count()).map_err(|_| DomainErrorCode::InvalidValue)?,
            profile.model_valuation_warning_basis_points,
        )?
    {
        issues.push(DataHealthIssue::new(
            DataHealthIssueCode::ModelValuationShare,
            Vec::new(),
            Some(evidence.data_source_ref().clone()),
            evidence.record_count(),
            10_000,
            0,
        )?);
    }
    if age > profile.max_data_snapshot_age_seconds {
        issues.push(DataHealthIssue::new(
            DataHealthIssueCode::StaleDataSnapshot,
            Vec::new(),
            Some(evidence.data_source_ref().clone()),
            0,
            0,
            age,
        )?);
    }
    Ok((
        Some(evidence.data_snapshot_id().clone()),
        Some(evidence.data_snapshot_manifest_hash().clone()),
        Some(evidence.data_source_ref().clone()),
    ))
}

fn compare_issues(left: &DataHealthIssue, right: &DataHealthIssue) -> Ordering {
    left.code
        .cmp(&right.code)
        .then_with(|| compare_ids(&left.affected_position_ids, &right.affected_position_ids))
        .then_with(|| {
            compare_optional_version_ref(
                left.data_source_ref.as_ref(),
                right.data_source_ref.as_ref(),
            )
        })
}

fn compare_ids(left: &[Ulid], right: &[Ulid]) -> Ordering {
    left.iter()
        .map(Ulid::as_str)
        .cmp(right.iter().map(Ulid::as_str))
}

fn compare_optional_version_ref(left: Option<&VersionRef>, right: Option<&VersionRef>) -> Ordering {
    left.map(|value| (value.id().as_str(), value.version().get()))
        .cmp(&right.map(|value| (value.id().as_str(), value.version().get())))
}

fn compare_lineage(left: &LineageRef, right: &LineageRef) -> Ordering {
    left.object_id()
        .as_str()
        .cmp(right.object_id().as_str())
        .then_with(|| {
            left.version()
                .map(Version::get)
                .cmp(&right.version().map(Version::get))
        })
        .then_with(|| {
            left.content_hash()
                .map(ContentHash::as_bytes)
                .cmp(&right.content_hash().map(ContentHash::as_bytes))
        })
}

fn append_issue(bytes: &mut Vec<u8>, issue: &DataHealthIssue) {
    append(bytes, &[issue_code(issue.code)]);
    for id in &issue.affected_position_ids {
        append(bytes, id.as_str().as_bytes());
    }
    append_optional_version_ref(bytes, issue.data_source_ref.as_ref());
    append(bytes, &issue.record_count.to_be_bytes());
    append(bytes, &issue.ratio_basis_points.to_be_bytes());
    append(bytes, &issue.observed_age_seconds.to_be_bytes());
}

fn append_owner(bytes: &mut Vec<u8>, owner: &OwnerRef) {
    append(bytes, owner.tenant_id().as_str().as_bytes());
    append(bytes, owner.owner_id().as_str().as_bytes());
}

fn append_version_ref(bytes: &mut Vec<u8>, reference: &VersionRef) {
    append(bytes, reference.id().as_str().as_bytes());
    append(bytes, &reference.version().get().to_be_bytes());
}

fn append_optional_version_ref(bytes: &mut Vec<u8>, reference: Option<&VersionRef>) {
    match reference {
        Some(reference) => {
            append(bytes, &[1]);
            append_version_ref(bytes, reference);
        }
        None => append(bytes, &[0]),
    }
}

fn append_optional_id(bytes: &mut Vec<u8>, value: Option<&Ulid>) {
    match value {
        Some(value) => {
            append(bytes, &[1]);
            append(bytes, value.as_str().as_bytes());
        }
        None => append(bytes, &[0]),
    }
}

fn append_optional_hash(bytes: &mut Vec<u8>, value: Option<&ContentHash>) {
    match value {
        Some(value) => {
            append(bytes, &[1]);
            append(bytes, value.as_bytes());
        }
        None => append(bytes, &[0]),
    }
}

fn append_time(bytes: &mut Vec<u8>, value: &MarketTime) {
    append(bytes, &value.instant().timestamp().to_be_bytes());
    append(
        bytes,
        &value.instant().timestamp_subsec_nanos().to_be_bytes(),
    );
    append(bytes, value.market_timezone().as_bytes());
    append(bytes, value.local_trading_date().to_string().as_bytes());
}

fn append_lineage(bytes: &mut Vec<u8>, reference: &LineageRef) {
    append(bytes, reference.object_id().as_str().as_bytes());
    match reference.version() {
        Some(version) => {
            append(bytes, &[1]);
            append(bytes, &version.get().to_be_bytes());
        }
        None => append(bytes, &[0]),
    }
    append_optional_hash(bytes, reference.content_hash());
}

fn append(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
    bytes.extend_from_slice(value);
}

const fn health_state_code(value: DataHealthState) -> u8 {
    match value {
        DataHealthState::Healthy => 1,
        DataHealthState::Warning => 2,
    }
}

const fn position_set_state_code(value: PositionSetState) -> u8 {
    match value {
        PositionSetState::NonEmpty => 1,
        PositionSetState::VerifiedEmpty => 2,
    }
}

const fn issue_code(value: DataHealthIssueCode) -> u8 {
    match value {
        DataHealthIssueCode::EmptyPositions => 1,
        DataHealthIssueCode::UnknownAccountingClassification => 2,
        DataHealthIssueCode::StalePositionSnapshot => 3,
        DataHealthIssueCode::UntypedPriceSource => 4,
        DataHealthIssueCode::ModelValuationShare => 5,
        DataHealthIssueCode::StaleDataSnapshot => 6,
    }
}
