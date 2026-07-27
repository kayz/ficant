use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use crate::primitives::{DecimalValue, Ulid, VersionRef};
use crate::{DomainErrorCode, DomainResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subject {
    subject_id: Ulid,
    display_name: String,
}

impl Subject {
    pub fn new(subject_id: Ulid, display_name: impl Into<String>) -> DomainResult<Self> {
        let display_name = display_name.into();
        require_text(&display_name)?;
        if display_name.len() > 256 {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            subject_id,
            display_name,
        })
    }

    pub fn id(&self) -> &Ulid {
        &self.subject_id
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessSet {
    market_codes: Vec<String>,
    tool_codes: Vec<String>,
}

impl AccessSet {
    pub fn new<I, J, M, T>(markets: I, tools: J) -> DomainResult<Self>
    where
        I: IntoIterator<Item = M>,
        J: IntoIterator<Item = T>,
        M: Into<String>,
        T: Into<String>,
    {
        Ok(Self {
            market_codes: canonical_codes(markets)?,
            tool_codes: canonical_codes(tools)?,
        })
    }

    pub fn market_codes(&self) -> &[String] {
        &self.market_codes
    }

    pub fn tool_codes(&self) -> &[String] {
        &self.tool_codes
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FundingTier {
    DrAvailable,
    ROnly,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaxTreatment {
    value_added_tax_profile: String,
    income_tax_profile: String,
}

impl TaxTreatment {
    pub fn new(
        value_added_tax_profile: impl Into<String>,
        income_tax_profile: impl Into<String>,
    ) -> DomainResult<Self> {
        let value_added_tax_profile = value_added_tax_profile.into();
        let income_tax_profile = income_tax_profile.into();
        require_text(&value_added_tax_profile)?;
        require_text(&income_tax_profile)?;
        Ok(Self {
            value_added_tax_profile,
            income_tax_profile,
        })
    }

    pub fn value_added_tax_profile(&self) -> &str {
        &self.value_added_tax_profile
    }

    pub fn income_tax_profile(&self) -> &str {
        &self.income_tax_profile
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConstraintSetRef {
    reference: VersionRef,
}

impl ConstraintSetRef {
    pub fn new(reference: VersionRef) -> Self {
        Self { reference }
    }

    pub fn reference(&self) -> &VersionRef {
        &self.reference
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubjectVersion {
    reference: VersionRef,
    access_set: AccessSet,
    funding_tier: FundingTier,
    tax_treatment: TaxTreatment,
    assessment_mechanism: String,
    liability_profile: String,
    constraint_set_ref: Option<ConstraintSetRef>,
}

impl SubjectVersion {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        reference: VersionRef,
        access_set: AccessSet,
        funding_tier: FundingTier,
        tax_treatment: TaxTreatment,
        assessment_mechanism: impl Into<String>,
        liability_profile: impl Into<String>,
        constraint_set_ref: Option<ConstraintSetRef>,
    ) -> DomainResult<Self> {
        let assessment_mechanism = assessment_mechanism.into();
        let liability_profile = liability_profile.into();
        require_text(&assessment_mechanism)?;
        require_text(&liability_profile)?;
        Ok(Self {
            reference,
            access_set,
            funding_tier,
            tax_treatment,
            assessment_mechanism,
            liability_profile,
            constraint_set_ref,
        })
    }

    pub fn reference(&self) -> &VersionRef {
        &self.reference
    }

    pub fn access_set(&self) -> &AccessSet {
        &self.access_set
    }

    pub fn funding_tier(&self) -> FundingTier {
        self.funding_tier
    }

    pub fn tax_treatment(&self) -> &TaxTreatment {
        &self.tax_treatment
    }

    pub fn assessment_mechanism(&self) -> &str {
        &self.assessment_mechanism
    }

    pub fn liability_profile(&self) -> &str {
        &self.liability_profile
    }

    pub fn constraint_set_ref(&self) -> Option<&ConstraintSetRef> {
        self.constraint_set_ref.as_ref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubjectRecord {
    subject: Subject,
    version: SubjectVersion,
}

impl SubjectRecord {
    pub fn new(subject: Subject, version: SubjectVersion) -> DomainResult<Self> {
        if subject.id() != version.reference().id() {
            return Err(DomainErrorCode::VersionConflict);
        }
        Ok(Self { subject, version })
    }

    pub fn subject(&self) -> &Subject {
        &self.subject
    }

    pub fn version(&self) -> &SubjectVersion {
        &self.version
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LimitCeiling {
    limit_code: String,
    ceiling: DecimalValue,
}

impl LimitCeiling {
    pub fn new(limit_code: impl Into<String>, ceiling: DecimalValue) -> DomainResult<Self> {
        let limit_code = limit_code.into();
        require_text(&limit_code)?;
        Ok(Self {
            limit_code,
            ceiling,
        })
    }

    pub fn limit_code(&self) -> &str {
        &self.limit_code
    }

    pub fn ceiling(&self) -> &DecimalValue {
        &self.ceiling
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubjectStateSnapshot {
    snapshot_id: Ulid,
    subject_ref: VersionRef,
    net_capital: DecimalValue,
    limit_ceilings: Vec<LimitCeiling>,
    observed_at: DateTime<Utc>,
    visible_at: DateTime<Utc>,
    market_timezone: String,
}

impl SubjectStateSnapshot {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        snapshot_id: Ulid,
        subject_ref: VersionRef,
        net_capital: DecimalValue,
        limit_ceilings: Vec<LimitCeiling>,
        observed_at: DateTime<Utc>,
        visible_at: DateTime<Utc>,
        market_timezone: impl Into<String>,
    ) -> DomainResult<Self> {
        let market_timezone = market_timezone.into();
        market_timezone
            .parse::<Tz>()
            .map_err(|_| DomainErrorCode::InvalidEffectiveTime)?;
        if observed_at > visible_at {
            return Err(DomainErrorCode::InvalidEffectiveTime);
        }
        let mut codes = std::collections::BTreeSet::new();
        for ceiling in &limit_ceilings {
            if !codes.insert(ceiling.limit_code().to_owned()) {
                return Err(DomainErrorCode::InvalidValue);
            }
        }
        Ok(Self {
            snapshot_id,
            subject_ref,
            net_capital,
            limit_ceilings,
            observed_at,
            visible_at,
            market_timezone,
        })
    }

    pub fn id(&self) -> &Ulid {
        &self.snapshot_id
    }

    pub fn subject_ref(&self) -> &VersionRef {
        &self.subject_ref
    }

    pub fn net_capital(&self) -> &DecimalValue {
        &self.net_capital
    }

    pub fn limit_ceilings(&self) -> &[LimitCeiling] {
        &self.limit_ceilings
    }

    pub fn observed_at(&self) -> DateTime<Utc> {
        self.observed_at
    }

    pub fn visible_at(&self) -> DateTime<Utc> {
        self.visible_at
    }

    pub fn market_timezone(&self) -> &str {
        &self.market_timezone
    }
}

fn require_text(value: &str) -> DomainResult<()> {
    if value.trim().is_empty() || value != value.trim() {
        return Err(DomainErrorCode::InvalidValue);
    }
    Ok(())
}

fn canonical_codes<I, S>(values: I) -> DomainResult<Vec<String>>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut values: Vec<String> = values.into_iter().map(Into::into).collect();
    for value in &values {
        require_text(value)?;
    }
    values.sort();
    values.dedup();
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::{UnitRef, Version};
    use chrono::TimeZone;

    fn id(value: &str) -> Ulid {
        Ulid::new(value).expect("fixture ULID")
    }

    fn unit() -> UnitRef {
        UnitRef::new(id("01J00000000000000000000001"), Version::new(1).unwrap())
    }

    #[test]
    fn subject_record_round_trips_identity_and_version_shape() {
        let subject = Subject::new(id("01J00000000000000000000002"), "R1 subject").unwrap();
        let reference = VersionRef::new(subject.id().clone(), Version::new(1).unwrap());
        let version = SubjectVersion::new(
            reference,
            AccessSet::new(["market-cn"], ["tool-b", "tool-a", "tool-a"]).unwrap(),
            FundingTier::DrAvailable,
            TaxTreatment::new("vat-profile", "income-profile").unwrap(),
            "assessment-v1",
            "liability-v1",
            None,
        )
        .unwrap();
        let record = SubjectRecord::new(subject, version).unwrap();
        assert_eq!(
            record.version().access_set().market_codes(),
            &["market-cn".to_owned()]
        );
        assert_eq!(
            record.version().access_set().tool_codes(),
            &["tool-a".to_owned(), "tool-b".to_owned()]
        );
    }

    #[test]
    fn state_snapshot_rejects_time_inversion_and_duplicate_limits() {
        let subject_ref =
            VersionRef::new(id("01J00000000000000000000003"), Version::new(1).unwrap());
        let decimal = DecimalValue::new("1000", 0, unit()).unwrap();
        let observed = Utc.with_ymd_and_hms(2026, 7, 27, 12, 0, 0).unwrap();
        let visible = Utc.with_ymd_and_hms(2026, 7, 27, 11, 0, 0).unwrap();
        assert_eq!(
            SubjectStateSnapshot::new(
                id("01J00000000000000000000004"),
                subject_ref.clone(),
                decimal.clone(),
                Vec::new(),
                observed,
                visible,
                "Asia/Shanghai",
            ),
            Err(DomainErrorCode::InvalidEffectiveTime)
        );

        let at = Utc.with_ymd_and_hms(2026, 7, 27, 12, 0, 0).unwrap();
        let duplicate = vec![
            LimitCeiling::new("credit", decimal.clone()).unwrap(),
            LimitCeiling::new("credit", decimal).unwrap(),
        ];
        assert_eq!(
            SubjectStateSnapshot::new(
                id("01J00000000000000000000004"),
                subject_ref,
                DecimalValue::new("1", 0, unit()).unwrap(),
                duplicate,
                at,
                at,
                "Asia/Shanghai",
            ),
            Err(DomainErrorCode::InvalidValue)
        );
    }
}
