use chrono::{NaiveDate, Utc};

use crate::primitives::{ContentHash, MarketTime, OwnerRef, Version, VersionRef};
pub use crate::primitives::{DECIMAL_SCALE, FixedDecimal};
use crate::{DomainErrorCode, DomainResult};

pub const INPUT_SCHEMA_ID: &str = "ficant.bond-analytics.input.v1";
pub const RESULT_SCHEMA_ID: &str = "ficant.bond-analytics.result.v1";
pub const ARTIFACT_SCHEMA_ID: &str = "ficant.bond-analytics.arrow.v1";
pub const ARTIFACT_CODEC_ID: &str = "ficant-bond-analytics-arrow/1";
pub const ENGINE_ID: &str = "ficant-fixed-income-native";
pub const ENGINE_VERSION: &str = "0.1.0";
pub const ALGORITHM_ID: &str = "ficant.cgb.fixed-rate.reference";
pub const ALGORITHM_VERSION: u32 = 1;
pub const CONVENTION_PROFILE: &str = "cgb-reference-v1";
pub const MARKET_TIMEZONE: &str = "Asia/Shanghai";
pub const ABI_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AnalyticsMode {
    YieldIn = 1,
    PriceIn = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CalendarRequirement {
    ReferenceReplay = 1,
    ExactMarket = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CalendarResolution {
    Exact = 1,
    ProvisionalWeekendOnly = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum CouponFrequency {
    Annual = 1,
    Semiannual = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum DayCountConvention {
    ActActBondIsma = 1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum BusinessDayConvention {
    Following = 1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnalyticsError {
    InvalidInput,
    AbiMismatch,
    BufferTooSmall,
    NoBracket,
    NotConverged,
    NonFinite,
    CalendarCoverageMissing,
    Internal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnalyticsObjectRef {
    version_ref: VersionRef,
    content_hash: ContentHash,
}

impl AnalyticsObjectRef {
    #[must_use]
    pub fn new(version_ref: VersionRef, content_hash: ContentHash) -> Self {
        Self {
            version_ref,
            content_hash,
        }
    }

    #[must_use]
    pub fn version_ref(&self) -> &VersionRef {
        &self.version_ref
    }

    #[must_use]
    pub fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BondTerms {
    first_issue_date: NaiveDate,
    current_issue_date: NaiveDate,
    maturity_date: NaiveDate,
    frequency: CouponFrequency,
    day_count: DayCountConvention,
    business_day: BusinessDayConvention,
    coupon_rate: FixedDecimal,
    face_amount: FixedDecimal,
    cumulative_issued_amount: FixedDecimal,
    tax_attributes: Option<crate::market::BondTaxAttributes>,
}

impl BondTerms {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        first_issue_date: NaiveDate,
        maturity_date: NaiveDate,
        frequency: CouponFrequency,
        day_count: DayCountConvention,
        business_day: BusinessDayConvention,
        coupon_rate: FixedDecimal,
        face_amount: FixedDecimal,
    ) -> DomainResult<Self> {
        if first_issue_date >= maturity_date
            || !coupon_rate.is_non_negative()
            || !face_amount.is_positive()
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            first_issue_date,
            current_issue_date: first_issue_date,
            maturity_date,
            frequency,
            day_count,
            business_day,
            coupon_rate,
            face_amount,
            cumulative_issued_amount: face_amount,
            tax_attributes: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_issuance(
        first_issue_date: NaiveDate,
        current_issue_date: NaiveDate,
        maturity_date: NaiveDate,
        frequency: CouponFrequency,
        day_count: DayCountConvention,
        business_day: BusinessDayConvention,
        coupon_rate: FixedDecimal,
        face_amount: FixedDecimal,
        cumulative_issued_amount: FixedDecimal,
        tax_attributes: crate::market::BondTaxAttributes,
    ) -> DomainResult<Self> {
        if first_issue_date >= maturity_date
            || current_issue_date < first_issue_date
            || current_issue_date >= maturity_date
            || !coupon_rate.is_non_negative()
            || !face_amount.is_positive()
            || !cumulative_issued_amount.is_positive()
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            first_issue_date,
            current_issue_date,
            maturity_date,
            frequency,
            day_count,
            business_day,
            coupon_rate,
            face_amount,
            cumulative_issued_amount,
            tax_attributes: Some(tax_attributes),
        })
    }

    #[must_use]
    pub const fn first_issue_date(&self) -> NaiveDate {
        self.first_issue_date
    }
    #[must_use]
    pub const fn current_issue_date(&self) -> NaiveDate {
        self.current_issue_date
    }
    #[must_use]
    pub const fn maturity_date(&self) -> NaiveDate {
        self.maturity_date
    }
    #[must_use]
    pub const fn frequency(&self) -> CouponFrequency {
        self.frequency
    }
    #[must_use]
    pub const fn day_count(&self) -> DayCountConvention {
        self.day_count
    }
    #[must_use]
    pub const fn business_day(&self) -> BusinessDayConvention {
        self.business_day
    }
    #[must_use]
    pub const fn coupon_rate(&self) -> FixedDecimal {
        self.coupon_rate
    }
    #[must_use]
    pub const fn face_amount(&self) -> FixedDecimal {
        self.face_amount
    }
    #[must_use]
    pub const fn cumulative_issued_amount(&self) -> FixedDecimal {
        self.cumulative_issued_amount
    }
    #[must_use]
    pub const fn tax_attributes(&self) -> Option<crate::market::BondTaxAttributes> {
        self.tax_attributes
    }

    /// Returns the same issuance facts with a new coupon rate for a parsed external adjustment.
    pub fn with_coupon_rate(&self, coupon_rate: FixedDecimal) -> DomainResult<Self> {
        if !coupon_rate.is_non_negative() {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            coupon_rate,
            ..self.clone()
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarBinding {
    calendar_id: String,
    version: Version,
    content_hash: ContentHash,
    coverage_start: NaiveDate,
    coverage_end: NaiveDate,
    non_business_days: Vec<NaiveDate>,
    work_weekends: Vec<NaiveDate>,
}

impl CalendarBinding {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        calendar_id: impl Into<String>,
        version: Version,
        content_hash: ContentHash,
        coverage_start: NaiveDate,
        coverage_end: NaiveDate,
        non_business_days: Vec<NaiveDate>,
        work_weekends: Vec<NaiveDate>,
    ) -> DomainResult<Self> {
        let calendar_id = calendar_id.into();
        if calendar_id.trim().is_empty()
            || calendar_id != calendar_id.trim()
            || coverage_start > coverage_end
            || !strictly_sorted(&non_business_days)
            || !strictly_sorted(&work_weekends)
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            calendar_id,
            version,
            content_hash,
            coverage_start,
            coverage_end,
            non_business_days,
            work_weekends,
        })
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.calendar_id
    }
    #[must_use]
    pub const fn version(&self) -> Version {
        self.version
    }
    #[must_use]
    pub fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
    #[must_use]
    pub const fn coverage_start(&self) -> NaiveDate {
        self.coverage_start
    }
    #[must_use]
    pub const fn coverage_end(&self) -> NaiveDate {
        self.coverage_end
    }
    #[must_use]
    pub fn non_business_days(&self) -> &[NaiveDate] {
        &self.non_business_days
    }
    #[must_use]
    pub fn work_weekends(&self) -> &[NaiveDate] {
        &self.work_weekends
    }
}

fn strictly_sorted(values: &[NaiveDate]) -> bool {
    values.windows(2).all(|pair| pair[0] < pair[1])
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BondAnalyticsInput {
    owner: OwnerRef,
    bond: AnalyticsObjectRef,
    rule_pack: AnalyticsObjectRef,
    snapshot: AnalyticsObjectRef,
    valuation_at: MarketTime,
    settlement_date: NaiveDate,
    calendar_requirement: CalendarRequirement,
    calendar: CalendarBinding,
    terms: BondTerms,
    mode: AnalyticsMode,
    input_value: FixedDecimal,
}

impl BondAnalyticsInput {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        owner: OwnerRef,
        bond: AnalyticsObjectRef,
        rule_pack: AnalyticsObjectRef,
        snapshot: AnalyticsObjectRef,
        valuation_at: MarketTime,
        settlement_date: NaiveDate,
        calendar_requirement: CalendarRequirement,
        calendar: CalendarBinding,
        terms: BondTerms,
        mode: AnalyticsMode,
        input_value: FixedDecimal,
    ) -> DomainResult<Self> {
        if valuation_at.market_timezone() != MARKET_TIMEZONE
            || settlement_date < terms.first_issue_date()
            || settlement_date >= terms.maturity_date()
            || !input_value.is_positive()
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        if calendar_requirement == CalendarRequirement::ExactMarket
            && (settlement_date < calendar.coverage_start()
                || settlement_date > calendar.coverage_end())
        {
            return Err(DomainErrorCode::InvalidEffectiveTime);
        }
        Ok(Self {
            owner,
            bond,
            rule_pack,
            snapshot,
            valuation_at,
            settlement_date,
            calendar_requirement,
            calendar,
            terms,
            mode,
            input_value,
        })
    }

    #[must_use]
    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }
    #[must_use]
    pub fn bond(&self) -> &AnalyticsObjectRef {
        &self.bond
    }
    #[must_use]
    pub fn rule_pack(&self) -> &AnalyticsObjectRef {
        &self.rule_pack
    }
    #[must_use]
    pub fn snapshot(&self) -> &AnalyticsObjectRef {
        &self.snapshot
    }
    #[must_use]
    pub fn valuation_at(&self) -> &MarketTime {
        &self.valuation_at
    }
    #[must_use]
    pub const fn settlement_date(&self) -> NaiveDate {
        self.settlement_date
    }
    #[must_use]
    pub const fn calendar_requirement(&self) -> CalendarRequirement {
        self.calendar_requirement
    }
    #[must_use]
    pub fn calendar(&self) -> &CalendarBinding {
        &self.calendar
    }
    #[must_use]
    pub fn terms(&self) -> &BondTerms {
        &self.terms
    }
    #[must_use]
    pub const fn mode(&self) -> AnalyticsMode {
        self.mode
    }
    #[must_use]
    pub const fn input_value(&self) -> FixedDecimal {
        self.input_value
    }

    /// Rebuilds the same calculation lineage with adjusted terms and the exact pre-tax price.
    ///
    /// This is used by a resolved external rule to rerun the same native engine without changing
    /// the Bond, market-rule, snapshot, valuation, settlement, or calendar facts.
    pub fn with_terms_and_price_in(
        &self,
        terms: BondTerms,
        clean_price: FixedDecimal,
    ) -> DomainResult<Self> {
        Self::new(
            self.owner.clone(),
            self.bond.clone(),
            self.rule_pack.clone(),
            self.snapshot.clone(),
            self.valuation_at.clone(),
            self.settlement_date,
            self.calendar_requirement,
            self.calendar.clone(),
            terms,
            AnalyticsMode::PriceIn,
            clean_price,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivedCashflow {
    sequence: u32,
    nominal_date: NaiveDate,
    payment_date: NaiveDate,
    coupon: FixedDecimal,
    principal: FixedDecimal,
    total: FixedDecimal,
}

impl DerivedCashflow {
    pub fn new(
        sequence: u32,
        nominal_date: NaiveDate,
        payment_date: NaiveDate,
        coupon: FixedDecimal,
        principal: FixedDecimal,
        total: FixedDecimal,
    ) -> DomainResult<Self> {
        if sequence == 0
            || payment_date < nominal_date
            || !coupon.is_non_negative()
            || !principal.is_non_negative()
            || total != coupon.checked_add(principal)?
            || !total.is_positive()
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            sequence,
            nominal_date,
            payment_date,
            coupon,
            principal,
            total,
        })
    }

    #[must_use]
    pub const fn sequence(&self) -> u32 {
        self.sequence
    }
    #[must_use]
    pub const fn nominal_date(&self) -> NaiveDate {
        self.nominal_date
    }
    #[must_use]
    pub const fn payment_date(&self) -> NaiveDate {
        self.payment_date
    }
    #[must_use]
    pub const fn coupon(&self) -> FixedDecimal {
        self.coupon
    }
    #[must_use]
    pub const fn principal(&self) -> FixedDecimal {
        self.principal
    }
    #[must_use]
    pub const fn total(&self) -> FixedDecimal {
        self.total
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnalyticsMeasures {
    accrued_interest: FixedDecimal,
    clean_price: FixedDecimal,
    dirty_price: FixedDecimal,
    yield_to_maturity: FixedDecimal,
    macaulay_duration: FixedDecimal,
    modified_duration: FixedDecimal,
    convexity: FixedDecimal,
    dv01: FixedDecimal,
}

impl AnalyticsMeasures {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        accrued_interest: FixedDecimal,
        clean_price: FixedDecimal,
        yield_to_maturity: FixedDecimal,
        macaulay_duration: FixedDecimal,
        modified_duration: FixedDecimal,
        convexity: FixedDecimal,
        dv01: FixedDecimal,
    ) -> DomainResult<Self> {
        let dirty_price = clean_price.checked_add(accrued_interest)?;
        if !accrued_interest.is_non_negative()
            || !clean_price.is_positive()
            || !dirty_price.is_positive()
            || !yield_to_maturity.is_positive()
            || !macaulay_duration.is_positive()
            || !modified_duration.is_positive()
            || !convexity.is_positive()
            || !dv01.is_positive()
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            accrued_interest,
            clean_price,
            dirty_price,
            yield_to_maturity,
            macaulay_duration,
            modified_duration,
            convexity,
            dv01,
        })
    }

    #[must_use]
    pub const fn accrued_interest(&self) -> FixedDecimal {
        self.accrued_interest
    }
    #[must_use]
    pub const fn clean_price(&self) -> FixedDecimal {
        self.clean_price
    }
    #[must_use]
    pub const fn dirty_price(&self) -> FixedDecimal {
        self.dirty_price
    }
    #[must_use]
    pub const fn yield_to_maturity(&self) -> FixedDecimal {
        self.yield_to_maturity
    }
    #[must_use]
    pub const fn macaulay_duration(&self) -> FixedDecimal {
        self.macaulay_duration
    }
    #[must_use]
    pub const fn modified_duration(&self) -> FixedDecimal {
        self.modified_duration
    }
    #[must_use]
    pub const fn convexity(&self) -> FixedDecimal {
        self.convexity
    }
    #[must_use]
    pub const fn dv01(&self) -> FixedDecimal {
        self.dv01
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BondAnalyticsResult {
    input: BondAnalyticsInput,
    calendar_resolution: CalendarResolution,
    cashflows: Vec<DerivedCashflow>,
    measures: AnalyticsMeasures,
}

impl BondAnalyticsResult {
    pub fn new(
        input: BondAnalyticsInput,
        calendar_resolution: CalendarResolution,
        cashflows: Vec<DerivedCashflow>,
        measures: AnalyticsMeasures,
    ) -> DomainResult<Self> {
        if cashflows.is_empty()
            || cashflows.iter().enumerate().any(|(index, cashflow)| {
                cashflow.sequence() != u32::try_from(index + 1).unwrap_or(u32::MAX)
                    || (index > 0 && cashflows[index - 1].payment_date() > cashflow.payment_date())
            })
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        if input.calendar_requirement() == CalendarRequirement::ExactMarket
            && calendar_resolution != CalendarResolution::Exact
        {
            return Err(DomainErrorCode::InvalidEffectiveTime);
        }
        Ok(Self {
            input,
            calendar_resolution,
            cashflows,
            measures,
        })
    }

    pub fn validate_against(&self, input: &BondAnalyticsInput) -> DomainResult<()> {
        if &self.input != input {
            return Err(DomainErrorCode::BrokenLineage);
        }
        Ok(())
    }

    #[must_use]
    pub fn input(&self) -> &BondAnalyticsInput {
        &self.input
    }
    #[must_use]
    pub const fn calendar_resolution(&self) -> CalendarResolution {
        self.calendar_resolution
    }
    #[must_use]
    pub fn cashflows(&self) -> &[DerivedCashflow] {
        &self.cashflows
    }
    #[must_use]
    pub fn measures(&self) -> &AnalyticsMeasures {
        &self.measures
    }
    #[must_use]
    pub fn schema_id(&self) -> &'static str {
        RESULT_SCHEMA_ID
    }
    #[must_use]
    pub fn engine_id(&self) -> &'static str {
        ENGINE_ID
    }
    #[must_use]
    pub fn engine_version(&self) -> &'static str {
        ENGINE_VERSION
    }
    #[must_use]
    pub fn algorithm_id(&self) -> &'static str {
        ALGORITHM_ID
    }
    #[must_use]
    pub const fn algorithm_version(&self) -> u32 {
        ALGORITHM_VERSION
    }
    #[must_use]
    pub fn convention_profile(&self) -> &'static str {
        CONVENTION_PROFILE
    }
    #[must_use]
    pub const fn abi_version(&self) -> u32 {
        ABI_VERSION
    }
}

#[must_use]
pub fn utc_micros(value: &MarketTime) -> i64 {
    value.valuation_micros()
}

trait MarketTimeMicros {
    fn valuation_micros(&self) -> i64;
}

impl MarketTimeMicros for MarketTime {
    fn valuation_micros(&self) -> i64 {
        let duration = self
            .instant()
            .signed_duration_since(chrono::DateTime::<Utc>::UNIX_EPOCH);
        duration
            .num_microseconds()
            .expect("validated market time must fit i64 microseconds")
    }
}
