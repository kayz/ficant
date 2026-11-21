//! Strict protobuf <-> domain/application codec and canonical request hashing for
//! `ficant.blotter.v1`.
//!
//! All input enums reject `UNSPECIFIED` and unknown discriminants except where explicitly allowed
//! by the contract (`calendar_requirement` inheritance and `eligibility_class` fail-closed
//! unverified state). All required sub-messages enforce presence and canonical unit bindings.

use std::collections::BTreeSet;

use chrono::{Datelike, NaiveDate, TimeZone, Utc, Weekday};
use ficant_application::ports::ApplicationResult;
use ficant_application::use_cases::blotter::{
    AccountChange, AnalysisUnits, BlotterEventEnvelope, CalendarResolutionMode, CoverageStatus,
    DraftEarmarkRecord, DraftEarmarkStatus, DraftTargetState, EnrichedAccountBlotterSnapshot,
    FullOpeningPositionBase, InstrumentCatalogEntry, MarketQuoteTick, NlpEarmarkProposal,
    PollEventsBatch, PreviewCoveredHedgeMeasures, PreviewCoveredHedgeOutput, PreviewHedgeScope,
    PullMarketQuotesResult, PullTradeFeedBatchResult, QuoteCursor, RateCoverageEligibility,
    RiskAndCoverageSummary, RuleLineage, SnapshotFallbackRequired, SourceTextAnchor,
    UncoveredReason, ValuedInstrumentPositionView,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory, map_domain_error};
use ficant_contracts::ficant::blotter::v1 as pb;
use ficant_contracts::ficant::core::v1 as core_pb;
use ficant_contracts::ficant::market::v1 as market_pb;
use ficant_contracts::ficant::rates::v1 as rates_pb;
use ficant_domain::analytics::{
    BondTaxBasis, CalendarBinding, CalendarRequirement, DECIMAL_SCALE, FixedDecimal,
};
use ficant_domain::blotter::{
    BaseEvidence, BaselineDelta, BucketBalanceView, EarmarkRecord, EarmarkState, FeedHealthState,
    FeedStatusView, FundingGapSummary, FundingPrecision, OpeningInstrumentBalance,
    OpeningPositionBase, PendingMatchReason, PendingMatchRecord, PendingMatchResolutionAction,
    PendingMatchState, QuoteBasis, QuoteTerms, ReconcileAllocationRecord, SeqRange,
    SettlementBucket, TradeFeedRecordAction, TradeFillIngestOutcome, TradeFillOutcomeKind,
    TradeFillRecord, TradeSide,
};
use ficant_domain::market::{
    Bond, BondBusinessDayConvention, BondCouponFrequency, BondDayCountConvention, BondPricingTerms,
    BondTaxAttributes, IncomeTaxStatus, Instrument, InstrumentInput, InstrumentKind,
    MarketRulePack, MarketRulePackInput, PriceSourceType, RulePackContent, ValueAddedTaxStatus,
    VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue as DomainDecimalValue, EffectivePeriod, MarketTime, OwnerRef, Ulid,
    UnitRef, Version, VersionRef,
};
use prost_types::Timestamp;

const DEFAULT_ULID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

// ============================================================================
// Core primitive conversions
// ============================================================================

pub fn parse_non_empty(value: &str) -> ApplicationResult<String> {
    if value.trim().is_empty() || value != value.trim() {
        return Err(invalid());
    }
    Ok(value.to_owned())
}

pub fn parse_ulid(u: Option<&core_pb::Ulid>) -> ApplicationResult<Ulid> {
    let raw = u.ok_or_else(invalid)?;
    Ulid::new(raw.value.clone()).map_err(map_domain_error)
}

#[must_use]
pub fn to_pb_ulid(id: &Ulid) -> core_pb::Ulid {
    core_pb::Ulid {
        value: id.as_str().to_owned(),
    }
}

pub fn parse_sha256(h: Option<&core_pb::Sha256>) -> ApplicationResult<ContentHash> {
    let raw = h.ok_or_else(invalid)?;
    if raw.value.len() != 32 {
        return Err(invalid());
    }
    ContentHash::from_bytes(&raw.value).map_err(map_domain_error)
}

#[must_use]
pub fn to_pb_sha256(hash: &ContentHash) -> core_pb::Sha256 {
    core_pb::Sha256 {
        value: hash.as_bytes().to_vec(),
    }
}

pub fn parse_unit_ref(u: Option<&core_pb::UnitRef>) -> ApplicationResult<UnitRef> {
    let u = u.ok_or_else(invalid)?;
    let id = parse_ulid(u.unit_id.as_ref())?;
    let ver = Version::new(u.version).map_err(map_domain_error)?;
    Ok(UnitRef::new(id, ver))
}

#[must_use]
pub fn to_pb_unit_ref(u: &UnitRef) -> core_pb::UnitRef {
    core_pb::UnitRef {
        unit_id: Some(to_pb_ulid(u.unit_id())),
        version: u.version().get(),
    }
}

pub fn parse_version_ref(v: Option<&core_pb::VersionRef>) -> ApplicationResult<VersionRef> {
    let v = v.ok_or_else(invalid)?;
    let id = parse_ulid(v.id.as_ref())?;
    let ver = Version::new(v.version).map_err(map_domain_error)?;
    Ok(VersionRef::new(id, ver))
}

#[must_use]
pub fn to_pb_version_ref(v: &VersionRef) -> core_pb::VersionRef {
    core_pb::VersionRef {
        id: Some(to_pb_ulid(v.id())),
        version: v.version().get(),
    }
}

pub fn parse_date(s: &str) -> ApplicationResult<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| invalid())
}

pub fn parse_market_time(mt: Option<&core_pb::MarketTime>) -> ApplicationResult<MarketTime> {
    let mt = mt.ok_or_else(invalid)?;
    let ts = mt.instant.as_ref().ok_or_else(invalid)?;
    let nanos = u32::try_from(ts.nanos).map_err(|_| invalid())?;
    let instant = Utc
        .timestamp_opt(ts.seconds, nanos)
        .single()
        .ok_or_else(invalid)?;
    let local_date = parse_date(&mt.local_trading_date)?;
    MarketTime::new(instant, mt.market_timezone.clone(), local_date).map_err(map_domain_error)
}

#[must_use]
pub fn to_pb_market_time(mt: &MarketTime) -> core_pb::MarketTime {
    let nanos = i32::try_from(mt.instant().timestamp_subsec_nanos()).unwrap_or(0);
    core_pb::MarketTime {
        instant: Some(Timestamp {
            seconds: mt.instant().timestamp(),
            nanos,
        }),
        market_timezone: mt.market_timezone().to_owned(),
        local_trading_date: mt.local_trading_date().to_string(),
    }
}

pub fn parse_domain_decimal(
    value: Option<&core_pb::DecimalValue>,
) -> ApplicationResult<DomainDecimalValue> {
    let value = value.ok_or_else(invalid)?;
    let unit = parse_unit_ref(value.unit.as_ref())?;
    let canonical = DomainDecimalValue::new(value.coefficient.clone(), value.scale, unit)
        .map_err(map_domain_error)?;
    if canonical.coefficient() != value.coefficient || canonical.scale() != value.scale {
        return Err(invalid());
    }
    Ok(canonical)
}

#[must_use]
pub fn to_pb_domain_decimal(value: &DomainDecimalValue) -> core_pb::DecimalValue {
    core_pb::DecimalValue {
        coefficient: value.coefficient().to_owned(),
        scale: value.scale(),
        unit: Some(to_pb_unit_ref(value.unit())),
    }
}

/// Canonical unit-bearing fixed-point decimal preserved during pre-transaction request hashing
/// so `UnitRef` changes trigger `HASH_MISMATCH` instead of false idempotent replay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalDecimal {
    pub value: FixedDecimal,
    pub unit: UnitRef,
}

/// Canonical unit-bearing quote basis for pre-transaction request hashing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CanonicalQuoteBasis {
    CleanPricePer100(CanonicalDecimal),
    YieldToMaturity(CanonicalDecimal),
}

/// Canonical unit-bearing `QuoteTerms` for pre-transaction request hashing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalQuoteTerms {
    pub basis: Option<CanonicalQuoteBasis>,
    pub settlement_amount: Option<CanonicalDecimal>,
}

/// Canonical unit-bearing `MarketQuoteTick` for pre-transaction request hashing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalMarketQuoteTick {
    pub tick: MarketQuoteTick,
    pub bid_clean_price_unit: Option<UnitRef>,
    pub ask_clean_price_unit: Option<UnitRef>,
    pub mid_clean_price_unit: Option<UnitRef>,
    pub mid_yield_to_maturity_unit: Option<UnitRef>,
}

/// Canonical unit-bearing `NlpEarmarkProposal` for pre-transaction request hashing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalNlpEarmarkProposal {
    pub parser_source_id: String,
    pub model_version: String,
    pub raw_source_text: String,
    pub anchors: Vec<SourceTextAnchor>,
    pub confidence_score: CanonicalDecimal,
    pub account_id: String,
    pub instrument_code: String,
    pub side: TradeSide,
    pub settlement_bucket: SettlementBucket,
    pub notional: Option<CanonicalDecimal>,
    pub quote_terms: Option<CanonicalQuoteTerms>,
    pub suggested_target_state: DraftTargetState,
}

/// Canonical unit-bearing `TradeFillRecord` for pre-transaction request hashing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalTradeFillRecord {
    pub record: TradeFillRecord,
    pub notional_unit: UnitRef,
    pub quote_terms: Option<CanonicalQuoteTerms>,
}

/// Canonical unit-bearing `BaseEvidence` for pre-transaction request hashing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalBaseEvidence {
    pub evidence: BaseEvidence,
    pub old_notional_unit: UnitRef,
    pub old_signed_cash_unit: UnitRef,
}

/// Canonical unit-bearing `BaselineDelta` for pre-transaction request hashing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalBaselineDelta {
    pub delta_notional: CanonicalDecimal,
    pub delta_cash: CanonicalDecimal,
    pub settlement_bucket: SettlementBucket,
    pub evidence: CanonicalBaseEvidence,
    pub reason: String,
}

pub fn parse_canonical_decimal(
    value: &core_pb::DecimalValue,
) -> ApplicationResult<CanonicalDecimal> {
    let canonical = parse_domain_decimal(Some(value))?;
    let coeff = canonical
        .coefficient()
        .parse::<i128>()
        .map_err(|_| invalid())?;
    if canonical.scale() > DECIMAL_SCALE {
        return Err(invalid());
    }
    let factor = 10_i128
        .checked_pow(DECIMAL_SCALE - canonical.scale())
        .ok_or_else(invalid)?;
    let scaled = coeff.checked_mul(factor).ok_or_else(invalid)?;
    Ok(CanonicalDecimal {
        value: FixedDecimal::from_scaled(scaled),
        unit: canonical.unit().clone(),
    })
}

pub fn parse_fixed_decimal_any_unit(
    value: &core_pb::DecimalValue,
) -> ApplicationResult<FixedDecimal> {
    Ok(parse_canonical_decimal(value)?.value)
}

pub fn parse_fixed_decimal(
    value: Option<&core_pb::DecimalValue>,
    expected_unit: &UnitRef,
) -> ApplicationResult<FixedDecimal> {
    let value = value.ok_or_else(invalid)?;
    let unit = parse_unit_ref(value.unit.as_ref())?;
    if &unit != expected_unit {
        return Err(invalid());
    }
    parse_fixed_decimal_any_unit(value)
}

pub fn parse_opt_fixed_decimal(
    value: Option<&core_pb::DecimalValue>,
    expected_unit: &UnitRef,
) -> ApplicationResult<Option<FixedDecimal>> {
    value
        .map(|v| parse_fixed_decimal(Some(v), expected_unit))
        .transpose()
}

#[allow(clippy::missing_panics_doc)]
#[must_use]
pub fn to_pb_decimal(value: FixedDecimal, unit: &UnitRef) -> core_pb::DecimalValue {
    let canonical =
        DomainDecimalValue::new(value.scaled().to_string(), DECIMAL_SCALE, unit.clone())
            .expect("FixedDecimal fits canonical DecimalValue");
    core_pb::DecimalValue {
        coefficient: canonical.coefficient().to_owned(),
        scale: canonical.scale(),
        unit: Some(to_pb_unit_ref(unit)),
    }
}

// ============================================================================
// AnalysisUnits, Bond, Calendar, MarketRulePack
// ============================================================================

pub fn parse_analysis_units(
    u: Option<&rates_pb::AnalysisUnits>,
) -> ApplicationResult<AnalysisUnits> {
    let u = u.ok_or_else(invalid)?;
    if let Some(years) = u.years.as_ref() {
        let _ = parse_unit_ref(Some(years))?;
    }
    if let Some(years_sq) = u.years_squared.as_ref() {
        let _ = parse_unit_ref(Some(years_sq))?;
    }
    Ok(AnalysisUnits {
        currency_amount: parse_unit_ref(u.currency_amount.as_ref())?,
        price_per_100: parse_unit_ref(u.price_per_100.as_ref())?,
        rate: parse_unit_ref(u.rate.as_ref())?,
        dv01: parse_unit_ref(u.dv01.as_ref())?,
        dv01_per_100: parse_unit_ref(u.dv01_per_100.as_ref())?,
        dimensionless: parse_unit_ref(u.dimensionless.as_ref())?,
        contract_count: parse_unit_ref(u.contract_count.as_ref())?,
    })
}

#[must_use]
pub fn to_pb_analysis_units(u: &AnalysisUnits) -> rates_pb::AnalysisUnits {
    rates_pb::AnalysisUnits {
        currency_amount: Some(to_pb_unit_ref(&u.currency_amount)),
        price_per_100: Some(to_pb_unit_ref(&u.price_per_100)),
        rate: Some(to_pb_unit_ref(&u.rate)),
        years: None,
        years_squared: None,
        dv01_per_100: Some(to_pb_unit_ref(&u.dv01_per_100)),
        dv01: Some(to_pb_unit_ref(&u.dv01)),
        dimensionless: Some(to_pb_unit_ref(&u.dimensionless)),
        contract_count: Some(to_pb_unit_ref(&u.contract_count)),
    }
}

pub fn parse_bond(
    bond_pb: &market_pb::Bond,
    instrument_code: &str,
    units: &AnalysisUnits,
) -> ApplicationResult<Bond> {
    let inst_ref = parse_version_ref(bond_pb.instrument.as_ref())?;
    let first_issue_date = parse_date(&bond_pb.first_issue_date)?;
    let current_issue_date = if bond_pb.current_issue_date.is_empty() {
        first_issue_date
    } else {
        parse_date(&bond_pb.current_issue_date)?
    };
    let maturity_date = parse_date(&bond_pb.maturity_date)?;
    let face_value = parse_domain_decimal(bond_pb.face_value.as_ref())?;
    if face_value.unit() != &units.currency_amount && face_value.unit() != &units.price_per_100 {
        return Err(invalid());
    }
    let cumulative_issued_amount = match bond_pb.cumulative_issued_amount.as_ref() {
        Some(v) => parse_domain_decimal(Some(v))?,
        None => face_value.clone(),
    };

    let owner = OwnerRef::new(inst_ref.id().clone(), inst_ref.id().clone());
    let instrument = Instrument::new(InstrumentInput {
        instrument_id: inst_ref.id().clone(),
        version: inst_ref.version(),
        owner,
        kind: InstrumentKind::Bond,
        market: "MARKET".to_owned(),
        symbol: instrument_code.to_owned(),
        currency: face_value.unit().clone(),
        calendar: inst_ref,
    })
    .map_err(map_domain_error)?;

    let mut bond = if let Some(ta) = bond_pb.tax_attributes.as_ref() {
        let vat = match market_pb::ValueAddedTaxStatus::try_from(ta.value_added_tax_status) {
            Ok(market_pb::ValueAddedTaxStatus::Exempt) => ValueAddedTaxStatus::Exempt,
            Ok(market_pb::ValueAddedTaxStatus::Taxable) => ValueAddedTaxStatus::Taxable,
            _ => return Err(invalid()),
        };
        let cit = match market_pb::IncomeTaxStatus::try_from(ta.income_tax_status) {
            Ok(market_pb::IncomeTaxStatus::Exempt) => IncomeTaxStatus::Exempt,
            Ok(market_pb::IncomeTaxStatus::Taxable) => IncomeTaxStatus::Taxable,
            _ => return Err(invalid()),
        };
        Bond::with_issuance(
            &instrument,
            first_issue_date,
            current_issue_date,
            maturity_date,
            cumulative_issued_amount,
            BondTaxAttributes::new(vat, cit),
            face_value,
        )
        .map_err(map_domain_error)?
    } else {
        Bond::new(&instrument, first_issue_date, maturity_date, face_value)
            .map_err(map_domain_error)?
    };

    if bond_pb.coupon_rate.is_some()
        || bond_pb.coupon_frequency != market_pb::BondCouponFrequency::Unspecified as i32
        || bond_pb.day_count != market_pb::BondDayCountConvention::Unspecified as i32
        || bond_pb.business_day != market_pb::BondBusinessDayConvention::Unspecified as i32
    {
        let coupon_rate = parse_domain_decimal(bond_pb.coupon_rate.as_ref())?;
        if coupon_rate.unit() != &units.rate {
            return Err(invalid());
        }
        let frequency = match market_pb::BondCouponFrequency::try_from(bond_pb.coupon_frequency) {
            Ok(market_pb::BondCouponFrequency::Annual) => BondCouponFrequency::Annual,
            Ok(market_pb::BondCouponFrequency::Semiannual) => BondCouponFrequency::Semiannual,
            _ => return Err(invalid()),
        };
        let day_count = match market_pb::BondDayCountConvention::try_from(bond_pb.day_count) {
            Ok(market_pb::BondDayCountConvention::ActActBondIsma) => {
                BondDayCountConvention::ActActBondIsma
            }
            _ => return Err(invalid()),
        };
        let business_day =
            match market_pb::BondBusinessDayConvention::try_from(bond_pb.business_day) {
                Ok(market_pb::BondBusinessDayConvention::Following) => {
                    BondBusinessDayConvention::Following
                }
                _ => return Err(invalid()),
            };
        let terms = BondPricingTerms::new(coupon_rate, frequency, day_count, business_day)
            .map_err(map_domain_error)?;
        bond = bond.with_pricing_terms(terms).map_err(map_domain_error)?;
    }

    Ok(bond)
}

#[must_use]
pub fn to_pb_bond(bond: &Bond) -> market_pb::Bond {
    let (coupon_rate, coupon_frequency, day_count, business_day) = match bond.pricing_terms() {
        Some(pt) => (
            Some(to_pb_domain_decimal(pt.coupon_rate())),
            match pt.frequency() {
                BondCouponFrequency::Annual => market_pb::BondCouponFrequency::Annual as i32,
                BondCouponFrequency::Semiannual => {
                    market_pb::BondCouponFrequency::Semiannual as i32
                }
            },
            match pt.day_count() {
                BondDayCountConvention::ActActBondIsma => {
                    market_pb::BondDayCountConvention::ActActBondIsma as i32
                }
            },
            match pt.business_day() {
                BondBusinessDayConvention::Following => {
                    market_pb::BondBusinessDayConvention::Following as i32
                }
            },
        ),
        None => (
            None,
            market_pb::BondCouponFrequency::Unspecified as i32,
            market_pb::BondDayCountConvention::Unspecified as i32,
            market_pb::BondBusinessDayConvention::Unspecified as i32,
        ),
    };
    let tax_attributes = bond
        .tax_attributes()
        .map(|ta| market_pb::BondTaxAttributes {
            value_added_tax_status: match ta.value_added_tax_status() {
                ValueAddedTaxStatus::Exempt => market_pb::ValueAddedTaxStatus::Exempt as i32,
                ValueAddedTaxStatus::Taxable => market_pb::ValueAddedTaxStatus::Taxable as i32,
            },
            income_tax_status: match ta.income_tax_status() {
                IncomeTaxStatus::Exempt => market_pb::IncomeTaxStatus::Exempt as i32,
                IncomeTaxStatus::Taxable => market_pb::IncomeTaxStatus::Taxable as i32,
            },
        });
    market_pb::Bond {
        instrument: Some(to_pb_version_ref(bond.instrument())),
        first_issue_date: bond.first_issue_date().to_string(),
        current_issue_date: bond.current_issue_date().to_string(),
        maturity_date: bond.maturity_date().to_string(),
        face_value: Some(to_pb_domain_decimal(bond.face_value())),
        cumulative_issued_amount: Some(to_pb_domain_decimal(bond.cumulative_issued_amount())),
        tax_attributes,
        coupon_rate,
        coupon_frequency,
        day_count,
        business_day,
    }
}

pub fn parse_calendar_binding(cal: &market_pb::Calendar) -> ApplicationResult<CalendarBinding> {
    if cal.sessions.is_empty() {
        return Err(invalid());
    }
    let id = match cal.calendar_id.as_ref() {
        Some(u) => Ulid::new(u.value.clone())
            .map_err(map_domain_error)?
            .as_str()
            .to_owned(),
        None => DEFAULT_ULID.to_owned(),
    };
    let version = Version::new(cal.version.max(1)).map_err(map_domain_error)?;
    let mut holidays = Vec::new();
    let mut work_weekends = Vec::new();
    let mut min_date: Option<NaiveDate> = None;
    let mut max_date: Option<NaiveDate> = None;

    for session in &cal.sessions {
        let d = parse_date(&session.local_date)?;
        min_date = Some(min_date.map_or(d, |m| m.min(d)));
        max_date = Some(max_date.map_or(d, |m| m.max(d)));
        let is_weekend = matches!(d.weekday(), Weekday::Sat | Weekday::Sun);
        if session.closed && !is_weekend {
            holidays.push(d);
        } else if !session.closed && is_weekend {
            work_weekends.push(d);
        }
    }
    holidays.sort();
    holidays.dedup();
    work_weekends.sort();
    work_weekends.dedup();

    let (Some(cov_start), Some(cov_end)) = (min_date, max_date) else {
        return Err(invalid());
    };
    let mut hash_buf = Vec::new();
    append_str(&mut hash_buf, &id);
    append_u64(&mut hash_buf, version.get());
    append_str(&mut hash_buf, &cov_start.to_string());
    append_str(&mut hash_buf, &cov_end.to_string());
    for h in &holidays {
        append_str(&mut hash_buf, &h.to_string());
    }
    for w in &work_weekends {
        append_str(&mut hash_buf, &w.to_string());
    }
    CalendarBinding::new(
        id,
        version,
        ContentHash::digest(&hash_buf),
        cov_start,
        cov_end,
        holidays,
        work_weekends,
    )
    .map_err(map_domain_error)
}

#[must_use]
pub fn to_pb_calendar(cal: &CalendarBinding) -> market_pb::Calendar {
    let holidays: BTreeSet<NaiveDate> = cal.non_business_days().iter().copied().collect();
    let work_weekends: BTreeSet<NaiveDate> = cal.work_weekends().iter().copied().collect();
    let mut dates = BTreeSet::new();
    dates.insert(cal.coverage_start());
    dates.insert(cal.coverage_end());
    for &d in &holidays {
        dates.insert(d);
    }
    for &d in &work_weekends {
        dates.insert(d);
    }

    let sessions = dates
        .into_iter()
        .map(|d| {
            let is_weekend = matches!(d.weekday(), Weekday::Sat | Weekday::Sun);
            let closed = if holidays.contains(&d) {
                true
            } else if work_weekends.contains(&d) {
                false
            } else {
                is_weekend
            };
            market_pb::CalendarSession {
                local_date: d.to_string(),
                open_local_time: if closed {
                    String::new()
                } else {
                    "09:00:00".to_owned()
                },
                close_local_time: if closed {
                    String::new()
                } else {
                    "17:00:00".to_owned()
                },
                closed,
            }
        })
        .collect();

    market_pb::Calendar {
        calendar_id: Some(core_pb::Ulid {
            value: cal.id().to_owned(),
        }),
        version: cal.version().get(),
        owner: None,
        market: String::new(),
        market_timezone: ficant_domain::analytics::MARKET_TIMEZONE.to_owned(),
        effective_from: None,
        effective_to: None,
        sessions,
    }
}

#[allow(clippy::missing_panics_doc)]
pub fn parse_strict_market_rule_pack(
    pack: Option<&market_pb::MarketRulePack>,
) -> ApplicationResult<MarketRulePack> {
    let p = pack.ok_or_else(invalid)?;
    let id = parse_ulid(p.rule_pack_id.as_ref())?;
    let version = Version::new(p.version).map_err(map_domain_error)?;
    let owner = p
        .owner
        .as_ref()
        .map(|o| -> ApplicationResult<OwnerRef> {
            Ok(OwnerRef::new(
                parse_ulid(o.tenant_id.as_ref())?,
                parse_ulid(o.owner_id.as_ref())?,
            ))
        })
        .transpose()?
        .unwrap_or_else(|| {
            let fallback = Ulid::new(DEFAULT_ULID).expect("valid default ULID");
            OwnerRef::new(fallback.clone(), fallback)
        });
    let from = parse_market_time(p.effective_from.as_ref())?;
    let to = parse_market_time(p.effective_to.as_ref())?;
    let effective = EffectivePeriod::new(from, to).map_err(map_domain_error)?;
    let status = match market_pb::VerificationStatus::try_from(p.verification_status) {
        Ok(market_pb::VerificationStatus::Verified) => VerificationStatus::Verified,
        Ok(market_pb::VerificationStatus::Unverified) => VerificationStatus::Unverified,
        Ok(market_pb::VerificationStatus::Rejected) => VerificationStatus::Rejected,
        _ => return Err(invalid()),
    };
    let content_any = p.content.as_ref().ok_or_else(invalid)?;
    let content = RulePackContent::new(content_any.type_url.clone(), content_any.value.clone())
        .map_err(map_domain_error)?;
    let hash = parse_sha256(p.content_hash.as_ref())?;

    MarketRulePack::new_with_content(
        MarketRulePackInput {
            rule_pack_id: id,
            version,
            owner,
            market: p.market.clone(),
            rule_type: p.rule_type.clone(),
            source: p.source.clone(),
            effective,
            verification_status: status,
            content_hash: hash,
        },
        content,
    )
    .map_err(map_domain_error)
}

// ============================================================================
// Enum conversions
// ============================================================================

pub fn parse_side(raw: i32) -> ApplicationResult<TradeSide> {
    match pb::TradeSide::try_from(raw) {
        Ok(pb::TradeSide::Buy) => Ok(TradeSide::Buy),
        Ok(pb::TradeSide::Sell) => Ok(TradeSide::Sell),
        _ => Err(invalid()),
    }
}

#[must_use]
pub const fn to_pb_side(side: TradeSide) -> i32 {
    match side {
        TradeSide::Buy => pb::TradeSide::Buy as i32,
        TradeSide::Sell => pb::TradeSide::Sell as i32,
    }
}

pub fn parse_bucket(raw: i32) -> ApplicationResult<SettlementBucket> {
    match pb::SettlementBucket::try_from(raw) {
        Ok(pb::SettlementBucket::TPlus0) => Ok(SettlementBucket::TPlus0),
        Ok(pb::SettlementBucket::TPlus1) => Ok(SettlementBucket::TPlus1),
        _ => Err(invalid()),
    }
}

#[must_use]
pub const fn to_pb_bucket(bucket: SettlementBucket) -> i32 {
    match bucket {
        SettlementBucket::TPlus0 => pb::SettlementBucket::TPlus0 as i32,
        SettlementBucket::TPlus1 => pb::SettlementBucket::TPlus1 as i32,
    }
}

pub fn parse_trade_action(raw: i32) -> ApplicationResult<TradeFeedRecordAction> {
    match pb::TradeFeedRecordAction::try_from(raw) {
        Ok(pb::TradeFeedRecordAction::NewFill) => Ok(TradeFeedRecordAction::NewFill),
        Ok(pb::TradeFeedRecordAction::Amend) => Ok(TradeFeedRecordAction::Amend),
        Ok(pb::TradeFeedRecordAction::CancelReversal) => Ok(TradeFeedRecordAction::CancelReversal),
        _ => Err(invalid()),
    }
}

#[must_use]
pub const fn to_pb_trade_action(action: TradeFeedRecordAction) -> i32 {
    match action {
        TradeFeedRecordAction::NewFill => pb::TradeFeedRecordAction::NewFill as i32,
        TradeFeedRecordAction::Amend => pb::TradeFeedRecordAction::Amend as i32,
        TradeFeedRecordAction::CancelReversal => pb::TradeFeedRecordAction::CancelReversal as i32,
    }
}

pub fn parse_resolution_action(raw: i32) -> ApplicationResult<PendingMatchResolutionAction> {
    match pb::PendingMatchResolutionAction::try_from(raw) {
        Ok(pb::PendingMatchResolutionAction::MatchFull) => {
            Ok(PendingMatchResolutionAction::MatchFull)
        }
        Ok(pb::PendingMatchResolutionAction::SplitOffset) => {
            Ok(PendingMatchResolutionAction::SplitOffset)
        }
        Ok(pb::PendingMatchResolutionAction::AcceptOrphan) => {
            Ok(PendingMatchResolutionAction::AcceptOrphan)
        }
        Ok(pb::PendingMatchResolutionAction::ReleaseReject) => {
            Ok(PendingMatchResolutionAction::ReleaseReject)
        }
        Ok(pb::PendingMatchResolutionAction::ApplyBaselineDelta) => {
            Ok(PendingMatchResolutionAction::ApplyBaselineDelta)
        }
        _ => Err(invalid()),
    }
}

pub fn parse_eligibility_optional(raw: i32) -> ApplicationResult<Option<RateCoverageEligibility>> {
    match pb::RateCoverageEligibility::try_from(raw) {
        Ok(pb::RateCoverageEligibility::Unspecified) => Ok(None),
        Ok(pb::RateCoverageEligibility::SovereignOrPolicyFixed) => {
            Ok(Some(RateCoverageEligibility::SovereignOrPolicyFixed))
        }
        Ok(pb::RateCoverageEligibility::CreditOrSpreadProduct) => {
            Ok(Some(RateCoverageEligibility::CreditOrSpreadProduct))
        }
        Ok(pb::RateCoverageEligibility::FloatingOrIndexed) => {
            Ok(Some(RateCoverageEligibility::FloatingOrIndexed))
        }
        Ok(pb::RateCoverageEligibility::EmbeddedOptionOrSinking) => {
            Ok(Some(RateCoverageEligibility::EmbeddedOptionOrSinking))
        }
        Err(_) => Err(invalid()),
    }
}

#[must_use]
pub const fn to_pb_eligibility(v: Option<RateCoverageEligibility>) -> i32 {
    match v {
        None => pb::RateCoverageEligibility::Unspecified as i32,
        Some(RateCoverageEligibility::SovereignOrPolicyFixed) => {
            pb::RateCoverageEligibility::SovereignOrPolicyFixed as i32
        }
        Some(RateCoverageEligibility::CreditOrSpreadProduct) => {
            pb::RateCoverageEligibility::CreditOrSpreadProduct as i32
        }
        Some(RateCoverageEligibility::FloatingOrIndexed) => {
            pb::RateCoverageEligibility::FloatingOrIndexed as i32
        }
        Some(RateCoverageEligibility::EmbeddedOptionOrSinking) => {
            pb::RateCoverageEligibility::EmbeddedOptionOrSinking as i32
        }
    }
}

pub fn parse_calendar_req_optional(raw: i32) -> ApplicationResult<Option<CalendarRequirement>> {
    match rates_pb::CalendarRequirement::try_from(raw) {
        Ok(rates_pb::CalendarRequirement::Unspecified) => Ok(None),
        Ok(rates_pb::CalendarRequirement::ReferenceReplay) => {
            Ok(Some(CalendarRequirement::ReferenceReplay))
        }
        Ok(rates_pb::CalendarRequirement::ExactMarket) => {
            Ok(Some(CalendarRequirement::ExactMarket))
        }
        Err(_) => Err(invalid()),
    }
}

#[must_use]
pub const fn to_pb_calendar_req(v: Option<CalendarRequirement>) -> i32 {
    match v {
        None => rates_pb::CalendarRequirement::Unspecified as i32,
        Some(CalendarRequirement::ReferenceReplay) => {
            rates_pb::CalendarRequirement::ReferenceReplay as i32
        }
        Some(CalendarRequirement::ExactMarket) => rates_pb::CalendarRequirement::ExactMarket as i32,
    }
}

pub fn parse_price_source_type(raw: i32) -> ApplicationResult<PriceSourceType> {
    match market_pb::PriceSourceType::try_from(raw) {
        Ok(market_pb::PriceSourceType::RealTrade) => Ok(PriceSourceType::RealTrade),
        Ok(market_pb::PriceSourceType::ActiveQuote) => Ok(PriceSourceType::ActiveQuote),
        Ok(market_pb::PriceSourceType::ModelValuation) => Ok(PriceSourceType::ModelValuation),
        Ok(market_pb::PriceSourceType::CurveInterpolation) => {
            Ok(PriceSourceType::CurveInterpolation)
        }
        _ => Err(invalid()),
    }
}

#[must_use]
pub const fn to_pb_price_source_type(v: PriceSourceType) -> i32 {
    match v {
        PriceSourceType::RealTrade => market_pb::PriceSourceType::RealTrade as i32,
        PriceSourceType::ActiveQuote => market_pb::PriceSourceType::ActiveQuote as i32,
        PriceSourceType::ModelValuation => market_pb::PriceSourceType::ModelValuation as i32,
        PriceSourceType::CurveInterpolation => {
            market_pb::PriceSourceType::CurveInterpolation as i32
        }
    }
}

pub fn parse_draft_target_state(raw: i32) -> ApplicationResult<DraftTargetState> {
    match pb::DraftTargetState::try_from(raw) {
        Ok(pb::DraftTargetState::Hold) => Ok(DraftTargetState::Hold),
        Ok(pb::DraftTargetState::Committed) => Ok(DraftTargetState::Committed),
        _ => Err(invalid()),
    }
}

#[must_use]
pub const fn to_pb_draft_target_state(v: DraftTargetState) -> i32 {
    match v {
        DraftTargetState::Hold => pb::DraftTargetState::Hold as i32,
        DraftTargetState::Committed => pb::DraftTargetState::Committed as i32,
    }
}

#[must_use]
pub const fn to_pb_draft_status(v: DraftEarmarkStatus) -> i32 {
    match v {
        DraftEarmarkStatus::PendingConfirmation => {
            pb::DraftEarmarkStatus::PendingConfirmation as i32
        }
        DraftEarmarkStatus::Confirmed => pb::DraftEarmarkStatus::Confirmed as i32,
        DraftEarmarkStatus::Discarded => pb::DraftEarmarkStatus::Discarded as i32,
    }
}

pub fn parse_preview_hedge_scope(raw: i32) -> ApplicationResult<PreviewHedgeScope> {
    match pb::PreviewHedgeScope::try_from(raw) {
        Ok(pb::PreviewHedgeScope::WholeAccount) => Ok(PreviewHedgeScope::WholeAccount),
        Ok(pb::PreviewHedgeScope::CoveredSubsetOnly) => Ok(PreviewHedgeScope::CoveredSubsetOnly),
        _ => Err(invalid()),
    }
}

#[must_use]
pub const fn to_pb_preview_hedge_scope(v: PreviewHedgeScope) -> i32 {
    match v {
        PreviewHedgeScope::WholeAccount => pb::PreviewHedgeScope::WholeAccount as i32,
        PreviewHedgeScope::CoveredSubsetOnly => pb::PreviewHedgeScope::CoveredSubsetOnly as i32,
    }
}

#[must_use]
pub const fn to_pb_earmark_state(s: EarmarkState) -> i32 {
    match s {
        EarmarkState::Hold => pb::EarmarkState::Hold as i32,
        EarmarkState::Committed => pb::EarmarkState::Committed as i32,
        EarmarkState::Expired => pb::EarmarkState::Expired as i32,
        EarmarkState::Released => pb::EarmarkState::Released as i32,
        EarmarkState::Cancelled => pb::EarmarkState::Cancelled as i32,
        EarmarkState::Reconciled => pb::EarmarkState::Reconciled as i32,
    }
}

#[must_use]
pub const fn to_pb_funding_precision(p: FundingPrecision) -> i32 {
    match p {
        FundingPrecision::ExactSettlementAmount => {
            pb::FundingPrecision::ExactSettlementAmount as i32
        }
        FundingPrecision::ExactDirtyPrice => pb::FundingPrecision::ExactDirtyPrice as i32,
        FundingPrecision::CleanPriceEstimate => pb::FundingPrecision::CleanPriceEstimate as i32,
        FundingPrecision::UnpricedYieldOnly => pb::FundingPrecision::UnpricedYieldOnly as i32,
    }
}

#[must_use]
pub const fn to_pb_pending_state(s: PendingMatchState) -> i32 {
    match s {
        PendingMatchState::Open => pb::PendingMatchState::Open as i32,
        PendingMatchState::ResolvedMatched => pb::PendingMatchState::ResolvedMatched as i32,
        PendingMatchState::ResolvedOrphanAccepted => {
            pb::PendingMatchState::ResolvedOrphanAccepted as i32
        }
        PendingMatchState::Released => pb::PendingMatchState::Released as i32,
        PendingMatchState::ReversedByFeed => pb::PendingMatchState::ReversedByFeed as i32,
        PendingMatchState::ResolvedBaselineDelta => {
            pb::PendingMatchState::ResolvedBaselineDelta as i32
        }
    }
}

#[must_use]
pub const fn to_pb_pending_reason(r: PendingMatchReason) -> i32 {
    match r {
        PendingMatchReason::AmbiguousCandidates => {
            pb::PendingMatchReason::AmbiguousCandidates as i32
        }
        PendingMatchReason::NotionalMismatchOrSplit => {
            pb::PendingMatchReason::NotionalMismatchOrSplit as i32
        }
        PendingMatchReason::PriceOutOfTolerance => {
            pb::PendingMatchReason::PriceOutOfTolerance as i32
        }
        PendingMatchReason::OrphanNoCommitment => pb::PendingMatchReason::OrphanNoCommitment as i32,
        PendingMatchReason::BurstRepeat => pb::PendingMatchReason::BurstRepeat as i32,
        PendingMatchReason::ExistingPendingOnKey => {
            pb::PendingMatchReason::ExistingPendingOnKey as i32
        }
        PendingMatchReason::AmendmentMismatch => pb::PendingMatchReason::AmendmentMismatch as i32,
        PendingMatchReason::BaselineRevisionManual => {
            pb::PendingMatchReason::BaselineRevisionManual as i32
        }
        PendingMatchReason::IncomparableBasis => pb::PendingMatchReason::IncomparableBasis as i32,
        PendingMatchReason::RevisionHold => pb::PendingMatchReason::RevisionHold as i32,
        PendingMatchReason::GapHold => pb::PendingMatchReason::GapHold as i32,
    }
}

#[must_use]
pub const fn to_pb_feed_health(h: FeedHealthState) -> i32 {
    match h {
        FeedHealthState::Healthy => pb::FeedHealthState::Healthy as i32,
        FeedHealthState::Stale => pb::FeedHealthState::Stale as i32,
        FeedHealthState::GapDetected => pb::FeedHealthState::GapDetected as i32,
        FeedHealthState::PausedByOperator => pb::FeedHealthState::PausedByOperator as i32,
    }
}

#[must_use]
pub const fn to_pb_coverage_status(c: CoverageStatus) -> i32 {
    match c {
        CoverageStatus::CoveredPreTax => pb::CoverageStatus::CoveredPreTax as i32,
        CoverageStatus::Uncovered => pb::CoverageStatus::Uncovered as i32,
        CoverageStatus::InactiveZeroPosition => pb::CoverageStatus::InactiveZeroPosition as i32,
    }
}

#[must_use]
pub const fn to_pb_uncovered_reason(r: Option<UncoveredReason>) -> i32 {
    match r {
        None => pb::UncoveredReason::Unspecified as i32,
        Some(UncoveredReason::InstrumentSpecMissing) => {
            pb::UncoveredReason::InstrumentSpecMissing as i32
        }
        Some(UncoveredReason::UnsupportedCouponStructure) => {
            pb::UncoveredReason::UnsupportedCouponStructure as i32
        }
        Some(UncoveredReason::EmbeddedOptionOrCallable) => {
            pb::UncoveredReason::EmbeddedOptionOrCallable as i32
        }
        Some(UncoveredReason::UnsupportedConvention) => {
            pb::UncoveredReason::UnsupportedConvention as i32
        }
        Some(UncoveredReason::MissingMarketQuote) => pb::UncoveredReason::MissingMarketQuote as i32,
        Some(UncoveredReason::CalendarCoverageMissing) => {
            pb::UncoveredReason::CalendarCoverageMissing as i32
        }
        Some(UncoveredReason::KernelPricingFailed) => {
            pb::UncoveredReason::KernelPricingFailed as i32
        }
        Some(UncoveredReason::EligibilityUnverified) => {
            pb::UncoveredReason::EligibilityUnverified as i32
        }
        Some(UncoveredReason::CreditOrNonRatesIssuer) => {
            pb::UncoveredReason::CreditOrNonRatesIssuer as i32
        }
        Some(UncoveredReason::StaleQuote) => pb::UncoveredReason::StaleQuote as i32,
    }
}

#[must_use]
pub const fn to_pb_calendar_resolution(m: CalendarResolutionMode) -> i32 {
    match m {
        CalendarResolutionMode::Unspecified => pb::CalendarResolutionMode::Unspecified as i32,
        CalendarResolutionMode::Exact => pb::CalendarResolutionMode::Exact as i32,
        CalendarResolutionMode::ProvisionalWeekendBeyondCoverage => {
            pb::CalendarResolutionMode::ProvisionalWeekendBeyondCoverage as i32
        }
    }
}

#[must_use]
pub const fn to_pb_tax_basis(b: BondTaxBasis) -> i32 {
    match b {
        BondTaxBasis::PreTax => rates_pb::BondTaxBasis::PreTax as i32,
        BondTaxBasis::TaxAdjusted => rates_pb::BondTaxBasis::TaxAdjusted as i32,
    }
}

#[must_use]
pub const fn to_pb_outcome_kind(k: TradeFillOutcomeKind) -> i32 {
    match k {
        TradeFillOutcomeKind::PerItemDeduplicated => {
            pb::TradeFillOutcomeKind::PerItemDeduplicated as i32
        }
        TradeFillOutcomeKind::PreWatermarkIgnored => {
            pb::TradeFillOutcomeKind::PreWatermarkIgnored as i32
        }
        TradeFillOutcomeKind::AutoMatched => pb::TradeFillOutcomeKind::AutoMatched as i32,
        TradeFillOutcomeKind::PendingMatch => pb::TradeFillOutcomeKind::PendingMatch as i32,
        TradeFillOutcomeKind::BufferedGap => pb::TradeFillOutcomeKind::BufferedGap as i32,
        TradeFillOutcomeKind::BufferedRevision => pb::TradeFillOutcomeKind::BufferedRevision as i32,
        TradeFillOutcomeKind::Reversed => pb::TradeFillOutcomeKind::Reversed as i32,
        TradeFillOutcomeKind::BaselineReview => pb::TradeFillOutcomeKind::BaselineReview as i32,
        TradeFillOutcomeKind::SupersededInBatch => {
            pb::TradeFillOutcomeKind::SupersededInBatch as i32
        }
    }
}

// ============================================================================
// Compound message conversions
// ============================================================================

pub fn parse_quote_terms(
    qt: Option<&pb::QuoteTerms>,
    units: &AnalysisUnits,
) -> ApplicationResult<QuoteTerms> {
    let qt = qt.ok_or_else(invalid)?;
    let basis = match qt.price_or_yield.as_ref() {
        Some(pb::quote_terms::PriceOrYield::CleanPricePer100(p)) => Some(
            QuoteBasis::CleanPricePer100(parse_fixed_decimal(Some(p), &units.price_per_100)?),
        ),
        Some(pb::quote_terms::PriceOrYield::YieldToMaturity(y)) => Some(
            QuoteBasis::YieldToMaturity(parse_fixed_decimal(Some(y), &units.rate)?),
        ),
        None => None,
    };
    let settlement_amount =
        parse_opt_fixed_decimal(qt.settlement_amount.as_ref(), &units.currency_amount)?;
    let out = QuoteTerms {
        basis,
        settlement_amount,
    };
    out.validate()
        .map_err(|e| ficant_application::map_blotter_error(&e))?;
    Ok(out)
}

pub fn parse_canonical_quote_terms(
    qt: Option<&pb::QuoteTerms>,
) -> ApplicationResult<CanonicalQuoteTerms> {
    let qt = qt.ok_or_else(invalid)?;
    let basis = match qt.price_or_yield.as_ref() {
        Some(pb::quote_terms::PriceOrYield::CleanPricePer100(p)) => Some(
            CanonicalQuoteBasis::CleanPricePer100(parse_canonical_decimal(p)?),
        ),
        Some(pb::quote_terms::PriceOrYield::YieldToMaturity(y)) => Some(
            CanonicalQuoteBasis::YieldToMaturity(parse_canonical_decimal(y)?),
        ),
        None => None,
    };
    let settlement_amount = qt
        .settlement_amount
        .as_ref()
        .map(parse_canonical_decimal)
        .transpose()?;
    let domain_check = QuoteTerms {
        basis: basis.as_ref().map(|b| match b {
            CanonicalQuoteBasis::CleanPricePer100(p) => QuoteBasis::CleanPricePer100(p.value),
            CanonicalQuoteBasis::YieldToMaturity(y) => QuoteBasis::YieldToMaturity(y.value),
        }),
        settlement_amount: settlement_amount.as_ref().map(|a| a.value),
    };
    domain_check
        .validate()
        .map_err(|e| ficant_application::map_blotter_error(&e))?;
    Ok(CanonicalQuoteTerms {
        basis,
        settlement_amount,
    })
}

pub fn parse_quote_terms_any_unit(qt: Option<&pb::QuoteTerms>) -> ApplicationResult<QuoteTerms> {
    let canonical = parse_canonical_quote_terms(qt)?;
    Ok(QuoteTerms {
        basis: canonical.basis.map(|b| match b {
            CanonicalQuoteBasis::CleanPricePer100(p) => QuoteBasis::CleanPricePer100(p.value),
            CanonicalQuoteBasis::YieldToMaturity(y) => QuoteBasis::YieldToMaturity(y.value),
        }),
        settlement_amount: canonical.settlement_amount.map(|a| a.value),
    })
}

pub fn parse_canonical_market_quote_tick(
    tick: &pb::MarketQuoteTick,
) -> ApplicationResult<CanonicalMarketQuoteTick> {
    let bid = tick
        .bid_clean_price
        .as_ref()
        .map(parse_canonical_decimal)
        .transpose()?;
    let ask = tick
        .ask_clean_price
        .as_ref()
        .map(parse_canonical_decimal)
        .transpose()?;
    let mid = tick
        .mid_clean_price
        .as_ref()
        .map(parse_canonical_decimal)
        .transpose()?;
    let ytm = tick
        .mid_yield_to_maturity
        .as_ref()
        .map(parse_canonical_decimal)
        .transpose()?;
    let parsed = MarketQuoteTick {
        source_id: parse_non_empty(&tick.source_id)?,
        quote_seq: tick.quote_seq,
        instrument_code: parse_non_empty(&tick.instrument_code)?,
        source_type: parse_price_source_type(tick.source_type)?,
        bid_clean_price: bid.as_ref().map(|v| v.value),
        ask_clean_price: ask.as_ref().map(|v| v.value),
        mid_clean_price: mid.as_ref().map(|v| v.value),
        mid_yield_to_maturity: ytm.as_ref().map(|v| v.value),
        observed_at: parse_market_time(tick.observed_at.as_ref())?,
        received_at: parse_market_time(tick.received_at.as_ref())?,
    };
    parsed.validate()?;
    Ok(CanonicalMarketQuoteTick {
        tick: parsed,
        bid_clean_price_unit: bid.map(|v| v.unit),
        ask_clean_price_unit: ask.map(|v| v.unit),
        mid_clean_price_unit: mid.map(|v| v.unit),
        mid_yield_to_maturity_unit: ytm.map(|v| v.unit),
    })
}

pub fn parse_canonical_nlp_proposal(
    prop: Option<&pb::NlpEarmarkProposal>,
) -> ApplicationResult<CanonicalNlpEarmarkProposal> {
    let p = prop.ok_or_else(invalid)?;
    let anchors = p
        .anchors
        .iter()
        .map(|a| SourceTextAnchor {
            field_name: a.field_name.clone(),
            byte_start: a.byte_start,
            byte_end: a.byte_end,
            span_text: a.span_text.clone(),
        })
        .collect();
    let confidence_score =
        parse_canonical_decimal(p.confidence_score.as_ref().ok_or_else(invalid)?)?;
    let notional = p
        .notional
        .as_ref()
        .map(parse_canonical_decimal)
        .transpose()?;
    let quote_terms = p
        .quote_terms
        .as_ref()
        .map(|qt| parse_canonical_quote_terms(Some(qt)))
        .transpose()?;
    Ok(CanonicalNlpEarmarkProposal {
        parser_source_id: parse_non_empty(&p.parser_source_id)?,
        model_version: parse_non_empty(&p.model_version)?,
        raw_source_text: p.raw_source_text.clone(),
        anchors,
        confidence_score,
        account_id: parse_non_empty(&p.account_id)?,
        instrument_code: parse_non_empty(&p.instrument_code)?,
        side: parse_side(p.side)?,
        settlement_bucket: parse_bucket(p.settlement_bucket)?,
        notional,
        quote_terms,
        suggested_target_state: parse_draft_target_state(p.suggested_target_state)?,
    })
}

pub fn parse_canonical_trade_fill_record(
    fill: &pb::TradeFillRecord,
) -> ApplicationResult<CanonicalTradeFillRecord> {
    let notional = parse_canonical_decimal(fill.notional.as_ref().ok_or_else(invalid)?)?;
    let quote_terms = fill
        .quote_terms
        .as_ref()
        .map(|qt| parse_canonical_quote_terms(Some(qt)))
        .transpose()?;
    let domain_quote_terms = quote_terms.as_ref().map(|qt| QuoteTerms {
        basis: qt.basis.as_ref().map(|b| match b {
            CanonicalQuoteBasis::CleanPricePer100(p) => QuoteBasis::CleanPricePer100(p.value),
            CanonicalQuoteBasis::YieldToMaturity(y) => QuoteBasis::YieldToMaturity(y.value),
        }),
        settlement_amount: qt.settlement_amount.as_ref().map(|a| a.value),
    });
    let record = TradeFillRecord {
        source_id: parse_non_empty(&fill.source_id)?,
        external_trade_id: parse_non_empty(&fill.external_trade_id)?,
        source_revision: fill.source_revision,
        feed_seq: fill.feed_seq,
        action: parse_trade_action(fill.action)?,
        account_id: parse_non_empty(&fill.account_id)?,
        instrument_code: parse_non_empty(&fill.instrument_code)?,
        side: parse_side(fill.side)?,
        settlement_date: parse_date(&fill.settlement_date)?,
        notional: notional.value,
        quote_terms: domain_quote_terms,
        executed_at: parse_market_time(fill.executed_at.as_ref())?,
        batch_first_feed_seq: fill.batch_first_feed_seq,
        batch_last_feed_seq: fill.batch_last_feed_seq,
        exact_dirty_price_input: None,
    };
    Ok(CanonicalTradeFillRecord {
        record,
        notional_unit: notional.unit,
        quote_terms,
    })
}

pub fn parse_canonical_baseline_delta(
    delta: Option<&pb::BaselineDelta>,
) -> ApplicationResult<CanonicalBaselineDelta> {
    let d = delta.ok_or_else(invalid)?;
    let ev = d.evidence.as_ref().ok_or_else(invalid)?;
    let delta_notional = parse_canonical_decimal(d.delta_notional.as_ref().ok_or_else(invalid)?)?;
    let delta_cash = parse_canonical_decimal(d.delta_cash.as_ref().ok_or_else(invalid)?)?;
    let old_notional = parse_canonical_decimal(ev.old_notional.as_ref().ok_or_else(invalid)?)?;
    let old_signed_cash =
        parse_canonical_decimal(ev.old_signed_cash.as_ref().ok_or_else(invalid)?)?;
    let evidence = BaseEvidence {
        source_id: parse_non_empty(&ev.source_id)?,
        external_trade_id: parse_non_empty(&ev.external_trade_id)?,
        original_feed_seq: ev.original_feed_seq,
        original_executed_at: parse_market_time(ev.original_executed_at.as_ref())?,
        old_notional: old_notional.value,
        old_signed_cash: old_signed_cash.value,
        settlement_bucket: parse_bucket(ev.settlement_bucket)?,
        evidence_ref: parse_non_empty(&ev.evidence_ref)?,
        evidence_hash: parse_sha256(ev.evidence_hash.as_ref())?,
        instrument_code: parse_non_empty(&ev.instrument_code)?,
        side: parse_side(ev.side)?,
        original_revision: ev.original_revision,
    };
    Ok(CanonicalBaselineDelta {
        delta_notional,
        delta_cash,
        settlement_bucket: parse_bucket(d.settlement_bucket)?,
        evidence: CanonicalBaseEvidence {
            evidence,
            old_notional_unit: old_notional.unit,
            old_signed_cash_unit: old_signed_cash.unit,
        },
        reason: parse_non_empty(&d.reason)?,
    })
}

#[must_use]
pub fn to_pb_quote_terms(qt: &QuoteTerms, units: &AnalysisUnits) -> pb::QuoteTerms {
    let price_or_yield = match qt.basis {
        Some(QuoteBasis::CleanPricePer100(p)) => Some(
            pb::quote_terms::PriceOrYield::CleanPricePer100(to_pb_decimal(p, &units.price_per_100)),
        ),
        Some(QuoteBasis::YieldToMaturity(y)) => Some(
            pb::quote_terms::PriceOrYield::YieldToMaturity(to_pb_decimal(y, &units.rate)),
        ),
        None => None,
    };
    pb::QuoteTerms {
        price_or_yield,
        settlement_amount: qt
            .settlement_amount
            .map(|a| to_pb_decimal(a, &units.currency_amount)),
    }
}

pub fn parse_base_evidence(
    ev: Option<&pb::BaseEvidence>,
    units: &AnalysisUnits,
) -> ApplicationResult<BaseEvidence> {
    let ev = ev.ok_or_else(invalid)?;
    Ok(BaseEvidence {
        source_id: parse_non_empty(&ev.source_id)?,
        external_trade_id: parse_non_empty(&ev.external_trade_id)?,
        original_feed_seq: ev.original_feed_seq,
        original_executed_at: parse_market_time(ev.original_executed_at.as_ref())?,
        old_notional: parse_fixed_decimal(ev.old_notional.as_ref(), &units.currency_amount)?,
        old_signed_cash: parse_fixed_decimal(ev.old_signed_cash.as_ref(), &units.currency_amount)?,
        settlement_bucket: parse_bucket(ev.settlement_bucket)?,
        evidence_ref: parse_non_empty(&ev.evidence_ref)?,
        evidence_hash: parse_sha256(ev.evidence_hash.as_ref())?,
        instrument_code: parse_non_empty(&ev.instrument_code)?,
        side: parse_side(ev.side)?,
        original_revision: ev.original_revision,
    })
}

#[must_use]
pub fn to_pb_base_evidence(ev: &BaseEvidence, units: &AnalysisUnits) -> pb::BaseEvidence {
    pb::BaseEvidence {
        source_id: ev.source_id.clone(),
        external_trade_id: ev.external_trade_id.clone(),
        original_feed_seq: ev.original_feed_seq,
        original_executed_at: Some(to_pb_market_time(&ev.original_executed_at)),
        old_notional: Some(to_pb_decimal(ev.old_notional, &units.currency_amount)),
        old_signed_cash: Some(to_pb_decimal(ev.old_signed_cash, &units.currency_amount)),
        settlement_bucket: to_pb_bucket(ev.settlement_bucket),
        evidence_ref: ev.evidence_ref.clone(),
        evidence_hash: Some(to_pb_sha256(&ev.evidence_hash)),
        instrument_code: ev.instrument_code.clone(),
        side: to_pb_side(ev.side),
        original_revision: ev.original_revision,
    }
}

pub fn parse_baseline_delta(
    delta: Option<&pb::BaselineDelta>,
    units: &AnalysisUnits,
) -> ApplicationResult<BaselineDelta> {
    let delta = delta.ok_or_else(invalid)?;
    Ok(BaselineDelta {
        delta_notional: parse_fixed_decimal(delta.delta_notional.as_ref(), &units.currency_amount)?,
        delta_cash: parse_fixed_decimal(delta.delta_cash.as_ref(), &units.currency_amount)?,
        settlement_bucket: parse_bucket(delta.settlement_bucket)?,
        evidence: parse_base_evidence(delta.evidence.as_ref(), units)?,
        reason: parse_non_empty(&delta.reason)?,
    })
}

#[must_use]
pub fn to_pb_baseline_delta(delta: &BaselineDelta, units: &AnalysisUnits) -> pb::BaselineDelta {
    pb::BaselineDelta {
        delta_notional: Some(to_pb_decimal(delta.delta_notional, &units.currency_amount)),
        delta_cash: Some(to_pb_decimal(delta.delta_cash, &units.currency_amount)),
        settlement_bucket: to_pb_bucket(delta.settlement_bucket),
        evidence: Some(to_pb_base_evidence(&delta.evidence, units)),
        reason: delta.reason.clone(),
    }
}

pub fn parse_full_opening_position_base(
    base_pb: Option<&pb::OpeningPositionBase>,
) -> ApplicationResult<FullOpeningPositionBase> {
    let b = base_pb.ok_or_else(invalid)?;
    let units = parse_analysis_units(b.units.as_ref())?;
    let account_id = parse_non_empty(&b.account_id)?;
    let base_id = parse_ulid(b.base_id.as_ref())?;
    let trading_date = parse_date(&b.trading_date)?;
    let next_settlement_date = parse_date(&b.next_settlement_date)?;
    let effective_from = parse_market_time(b.effective_from.as_ref())?;
    let watermark_time = parse_market_time(b.watermark_time.as_ref())?;
    let source_id = parse_non_empty(&b.source_id)?;
    let opening_cash_t0 = parse_fixed_decimal(b.opening_cash_t0.as_ref(), &units.currency_amount)?;
    let opening_net_cash_delta_t1 =
        parse_fixed_decimal(b.opening_net_cash_delta_t1.as_ref(), &units.currency_amount)?;

    let mut balances = Vec::with_capacity(b.balances.len());
    for bal in &b.balances {
        balances.push(OpeningInstrumentBalance {
            instrument_code: parse_non_empty(&bal.instrument_code)?,
            opening_economic_notional: parse_fixed_decimal(
                bal.opening_economic_notional.as_ref(),
                &units.currency_amount,
            )?,
            opening_available_notional: parse_fixed_decimal(
                bal.opening_available_notional.as_ref(),
                &units.currency_amount,
            )?,
            opening_net_t0_notional: parse_fixed_decimal(
                bal.opening_net_t0_notional.as_ref(),
                &units.currency_amount,
            )?,
            opening_net_t1_notional: parse_fixed_decimal(
                bal.opening_net_t1_notional.as_ref(),
                &units.currency_amount,
            )?,
        });
    }

    let mut instruments = Vec::with_capacity(b.instruments.len());
    let mut instrument_codes = Vec::with_capacity(b.instruments.len());
    for entry in &b.instruments {
        let code = parse_non_empty(&entry.instrument_code)?;
        let bond = entry
            .bond
            .as_ref()
            .map(|bond_pb| parse_bond(bond_pb, &code, &units))
            .transpose()?;
        let eligibility_class = parse_eligibility_optional(entry.eligibility_class)?;
        let calendar_requirement = parse_calendar_req_optional(entry.calendar_requirement)?;
        instrument_codes.push(code.clone());
        instruments.push(InstrumentCatalogEntry {
            instrument_code: code,
            bond,
            eligibility_class,
            calendar_requirement,
        });
    }

    let calendar = b
        .calendar
        .as_ref()
        .map(parse_calendar_binding)
        .transpose()?;
    let default_calendar_requirement = parse_calendar_req_optional(b.default_calendar_requirement)?;

    let mut baseline_evidence = Vec::with_capacity(b.baseline_evidence.len());
    for ev in &b.baseline_evidence {
        baseline_evidence.push(parse_base_evidence(Some(ev), &units)?);
    }

    Ok(FullOpeningPositionBase {
        core: OpeningPositionBase {
            account_id,
            base_id,
            trading_date,
            next_settlement_date,
            effective_from,
            watermark_time,
            watermark_source_seq: b.watermark_source_seq,
            source_id,
            opening_cash_t0,
            opening_net_cash_delta_t1,
            balances,
            instrument_codes,
            baseline_evidence,
            source_start_after_feed_seq: b.source_start_after_feed_seq,
        },
        instruments,
        calendar,
        default_calendar_requirement,
        units,
    })
}

#[must_use]
pub fn to_pb_opening_position_base(base: &FullOpeningPositionBase) -> pb::OpeningPositionBase {
    let units = &base.units;
    pb::OpeningPositionBase {
        account_id: base.core.account_id.clone(),
        base_id: Some(to_pb_ulid(&base.core.base_id)),
        trading_date: base.core.trading_date.to_string(),
        next_settlement_date: base.core.next_settlement_date.to_string(),
        effective_from: Some(to_pb_market_time(&base.core.effective_from)),
        watermark_time: Some(to_pb_market_time(&base.core.watermark_time)),
        watermark_source_seq: base.core.watermark_source_seq,
        source_id: base.core.source_id.clone(),
        opening_cash_t0: Some(to_pb_decimal(
            base.core.opening_cash_t0,
            &units.currency_amount,
        )),
        opening_net_cash_delta_t1: Some(to_pb_decimal(
            base.core.opening_net_cash_delta_t1,
            &units.currency_amount,
        )),
        balances: base
            .core
            .balances
            .iter()
            .map(|bal| pb::OpeningInstrumentBalance {
                instrument_code: bal.instrument_code.clone(),
                opening_economic_notional: Some(to_pb_decimal(
                    bal.opening_economic_notional,
                    &units.currency_amount,
                )),
                opening_available_notional: Some(to_pb_decimal(
                    bal.opening_available_notional,
                    &units.currency_amount,
                )),
                opening_net_t0_notional: Some(to_pb_decimal(
                    bal.opening_net_t0_notional,
                    &units.currency_amount,
                )),
                opening_net_t1_notional: Some(to_pb_decimal(
                    bal.opening_net_t1_notional,
                    &units.currency_amount,
                )),
            })
            .collect(),
        instruments: base
            .instruments
            .iter()
            .map(|inst| pb::InstrumentCatalogEntry {
                instrument_code: inst.instrument_code.clone(),
                bond: inst.bond.as_ref().map(to_pb_bond),
                eligibility_class: to_pb_eligibility(inst.eligibility_class),
                calendar_requirement: to_pb_calendar_req(inst.calendar_requirement),
            })
            .collect(),
        calendar: base.calendar.as_ref().map(to_pb_calendar),
        default_calendar_requirement: to_pb_calendar_req(base.default_calendar_requirement),
        units: Some(to_pb_analysis_units(units)),
        baseline_evidence: base
            .core
            .baseline_evidence
            .iter()
            .map(|ev| to_pb_base_evidence(ev, units))
            .collect(),
        source_start_after_feed_seq: base.core.source_start_after_feed_seq,
    }
}

pub fn parse_market_quote_tick(
    tick: &pb::MarketQuoteTick,
    units: &AnalysisUnits,
) -> ApplicationResult<MarketQuoteTick> {
    let parsed = MarketQuoteTick {
        source_id: parse_non_empty(&tick.source_id)?,
        quote_seq: tick.quote_seq,
        instrument_code: parse_non_empty(&tick.instrument_code)?,
        source_type: parse_price_source_type(tick.source_type)?,
        bid_clean_price: parse_opt_fixed_decimal(
            tick.bid_clean_price.as_ref(),
            &units.price_per_100,
        )?,
        ask_clean_price: parse_opt_fixed_decimal(
            tick.ask_clean_price.as_ref(),
            &units.price_per_100,
        )?,
        mid_clean_price: parse_opt_fixed_decimal(
            tick.mid_clean_price.as_ref(),
            &units.price_per_100,
        )?,
        mid_yield_to_maturity: parse_opt_fixed_decimal(
            tick.mid_yield_to_maturity.as_ref(),
            &units.rate,
        )?,
        observed_at: parse_market_time(tick.observed_at.as_ref())?,
        received_at: parse_market_time(tick.received_at.as_ref())?,
    };
    parsed.validate()?;
    Ok(parsed)
}

#[must_use]
pub fn to_pb_market_quote_tick(
    tick: &MarketQuoteTick,
    units: &AnalysisUnits,
) -> pb::MarketQuoteTick {
    pb::MarketQuoteTick {
        source_id: tick.source_id.clone(),
        quote_seq: tick.quote_seq,
        instrument_code: tick.instrument_code.clone(),
        source_type: to_pb_price_source_type(tick.source_type),
        bid_clean_price: tick
            .bid_clean_price
            .map(|v| to_pb_decimal(v, &units.price_per_100)),
        ask_clean_price: tick
            .ask_clean_price
            .map(|v| to_pb_decimal(v, &units.price_per_100)),
        mid_clean_price: tick
            .mid_clean_price
            .map(|v| to_pb_decimal(v, &units.price_per_100)),
        mid_yield_to_maturity: tick
            .mid_yield_to_maturity
            .map(|v| to_pb_decimal(v, &units.rate)),
        observed_at: Some(to_pb_market_time(&tick.observed_at)),
        received_at: Some(to_pb_market_time(&tick.received_at)),
    }
}

pub fn parse_trade_fill_record(
    fill: &pb::TradeFillRecord,
    units: Option<&AnalysisUnits>,
) -> ApplicationResult<TradeFillRecord> {
    let notional = match units {
        Some(u) => parse_fixed_decimal(fill.notional.as_ref(), &u.currency_amount)?,
        None => parse_fixed_decimal_any_unit(fill.notional.as_ref().ok_or_else(invalid)?)?,
    };
    let quote_terms = match (&fill.quote_terms, units) {
        (Some(qt), Some(u)) => Some(parse_quote_terms(Some(qt), u)?),
        (Some(qt), None) => Some(parse_quote_terms_any_unit(Some(qt))?),
        (None, _) => None,
    };
    Ok(TradeFillRecord {
        source_id: parse_non_empty(&fill.source_id)?,
        external_trade_id: parse_non_empty(&fill.external_trade_id)?,
        source_revision: fill.source_revision,
        feed_seq: fill.feed_seq,
        action: parse_trade_action(fill.action)?,
        account_id: parse_non_empty(&fill.account_id)?,
        instrument_code: parse_non_empty(&fill.instrument_code)?,
        side: parse_side(fill.side)?,
        settlement_date: parse_date(&fill.settlement_date)?,
        notional,
        quote_terms,
        executed_at: parse_market_time(fill.executed_at.as_ref())?,
        batch_first_feed_seq: fill.batch_first_feed_seq,
        batch_last_feed_seq: fill.batch_last_feed_seq,
        exact_dirty_price_input: None,
    })
}

#[must_use]
pub fn to_pb_trade_fill_record(
    fill: &TradeFillRecord,
    units: &AnalysisUnits,
) -> pb::TradeFillRecord {
    pb::TradeFillRecord {
        source_id: fill.source_id.clone(),
        external_trade_id: fill.external_trade_id.clone(),
        source_revision: fill.source_revision,
        feed_seq: fill.feed_seq,
        action: to_pb_trade_action(fill.action),
        account_id: fill.account_id.clone(),
        instrument_code: fill.instrument_code.clone(),
        side: to_pb_side(fill.side),
        settlement_date: fill.settlement_date.to_string(),
        notional: Some(to_pb_decimal(fill.notional, &units.currency_amount)),
        quote_terms: fill
            .quote_terms
            .as_ref()
            .map(|qt| to_pb_quote_terms(qt, units)),
        executed_at: Some(to_pb_market_time(&fill.executed_at)),
        batch_first_feed_seq: fill.batch_first_feed_seq,
        batch_last_feed_seq: fill.batch_last_feed_seq,
    }
}

pub fn parse_nlp_proposal(
    prop: Option<&pb::NlpEarmarkProposal>,
    units: &AnalysisUnits,
) -> ApplicationResult<NlpEarmarkProposal> {
    let p = prop.ok_or_else(invalid)?;
    let anchors = p
        .anchors
        .iter()
        .map(|a| SourceTextAnchor {
            field_name: a.field_name.clone(),
            byte_start: a.byte_start,
            byte_end: a.byte_end,
            span_text: a.span_text.clone(),
        })
        .collect();
    let notional = parse_opt_fixed_decimal(p.notional.as_ref(), &units.currency_amount)?;
    let quote_terms = p
        .quote_terms
        .as_ref()
        .map(|qt| parse_quote_terms(Some(qt), units))
        .transpose()?;
    Ok(NlpEarmarkProposal {
        parser_source_id: parse_non_empty(&p.parser_source_id)?,
        model_version: parse_non_empty(&p.model_version)?,
        raw_source_text: p.raw_source_text.clone(),
        anchors,
        confidence_score: parse_fixed_decimal(p.confidence_score.as_ref(), &units.dimensionless)?,
        account_id: parse_non_empty(&p.account_id)?,
        instrument_code: parse_non_empty(&p.instrument_code)?,
        side: parse_side(p.side)?,
        settlement_bucket: parse_bucket(p.settlement_bucket)?,
        notional,
        quote_terms,
        suggested_target_state: parse_draft_target_state(p.suggested_target_state)?,
    })
}

#[must_use]
pub fn to_pb_nlp_proposal(
    prop: &NlpEarmarkProposal,
    units: &AnalysisUnits,
) -> pb::NlpEarmarkProposal {
    pb::NlpEarmarkProposal {
        parser_source_id: prop.parser_source_id.clone(),
        model_version: prop.model_version.clone(),
        raw_source_text: prop.raw_source_text.clone(),
        anchors: prop.anchors.iter().map(to_pb_anchor).collect(),
        confidence_score: Some(to_pb_decimal(prop.confidence_score, &units.dimensionless)),
        account_id: prop.account_id.clone(),
        instrument_code: prop.instrument_code.clone(),
        side: to_pb_side(prop.side),
        settlement_bucket: to_pb_bucket(prop.settlement_bucket),
        notional: prop
            .notional
            .map(|n| to_pb_decimal(n, &units.currency_amount)),
        quote_terms: prop
            .quote_terms
            .as_ref()
            .map(|qt| to_pb_quote_terms(qt, units)),
        suggested_target_state: to_pb_draft_target_state(prop.suggested_target_state),
    }
}

#[must_use]
pub fn to_pb_anchor(a: &SourceTextAnchor) -> pb::SourceTextAnchor {
    pb::SourceTextAnchor {
        field_name: a.field_name.clone(),
        byte_start: a.byte_start,
        byte_end: a.byte_end,
        span_text: a.span_text.clone(),
    }
}

#[must_use]
pub fn to_pb_rule_lineage(r: &RuleLineage) -> pb::RuleLineage {
    pb::RuleLineage {
        pack_id: Some(to_pb_ulid(&r.pack_id)),
        version: r.version,
        source: r.source.clone(),
        content_hash: Some(to_pb_sha256(&r.content_hash)),
        parsed_constraints_hash: Some(to_pb_sha256(&r.parsed_constraints_hash)),
    }
}

#[must_use]
pub fn to_pb_draft_record(d: &DraftEarmarkRecord, units: &AnalysisUnits) -> pb::DraftEarmarkRecord {
    pb::DraftEarmarkRecord {
        draft_id: Some(to_pb_ulid(&d.draft_id)),
        draft_revision: d.draft_revision,
        account_id: d.account_id.clone(),
        instrument_code: d.instrument_code.clone(),
        side: to_pb_side(d.side),
        settlement_bucket: to_pb_bucket(d.settlement_bucket),
        notional: Some(to_pb_decimal(d.notional, &units.currency_amount)),
        quote_terms: Some(to_pb_quote_terms(&d.quote_terms, units)),
        target_state: to_pb_draft_target_state(d.target_state),
        status: to_pb_draft_status(d.status),
        raw_source_text: d.raw_source_text.clone(),
        anchors: d.anchors.iter().map(to_pb_anchor).collect(),
        confidence_score: Some(to_pb_decimal(d.confidence_score, &units.dimensionless)),
        parser_source_id: d.parser_source_id.clone(),
        preview_coverage_status: to_pb_coverage_status(d.preview_coverage_status),
        preview_delta_pre_tax_dv01: d
            .preview_delta_pre_tax_dv01
            .map(|v| to_pb_decimal(v, &units.dv01)),
        preview_uncovered_reason: to_pb_uncovered_reason(d.preview_uncovered_reason),
        preview_delta_uncovered_notional: d
            .preview_delta_uncovered_notional
            .map(|v| to_pb_decimal(v, &units.currency_amount)),
        preview_delta_cash_t0: d
            .preview_delta_cash_t0
            .map(|v| to_pb_decimal(v, &units.currency_amount)),
        preview_delta_cash_t1: d
            .preview_delta_cash_t1
            .map(|v| to_pb_decimal(v, &units.currency_amount)),
        preview_at_seq: d.preview_at_seq,
        preview_quote_seq: d.preview_quote_seq,
        confirmation_token: Some(to_pb_sha256(&d.confirmation_token)),
        rule_lineage: Some(to_pb_rule_lineage(&d.rule_lineage)),
        preview_quote_source_id: d.preview_quote_source_id.clone(),
        context_hash: Some(to_pb_sha256(&d.context_hash)),
        confirmed_by_trader_id: d.confirmed_by_trader_id.clone(),
        valuation_settlement_date: d.valuation_settlement_date.to_string(),
    }
}

#[must_use]
pub fn to_pb_bucket_balance(b: &BucketBalanceView, units: &AnalysisUnits) -> pb::BucketBalanceView {
    pb::BucketBalanceView {
        settlement_bucket: to_pb_bucket(b.settlement_bucket),
        settlement_date: b.settlement_date.to_string(),
        available_notional: Some(to_pb_decimal(b.available_notional, &units.currency_amount)),
        hold_buy_notional: Some(to_pb_decimal(b.hold_buy_notional, &units.currency_amount)),
        hold_sell_notional: Some(to_pb_decimal(b.hold_sell_notional, &units.currency_amount)),
        committed_buy_unreconciled_notional: Some(to_pb_decimal(
            b.committed_buy_unreconciled_notional,
            &units.currency_amount,
        )),
        committed_sell_unreconciled_notional: Some(to_pb_decimal(
            b.committed_sell_unreconciled_notional,
            &units.currency_amount,
        )),
        reconciled_buy_notional: Some(to_pb_decimal(
            b.reconciled_buy_notional,
            &units.currency_amount,
        )),
        reconciled_sell_notional: Some(to_pb_decimal(
            b.reconciled_sell_notional,
            &units.currency_amount,
        )),
        pending_match_buy_unresolved_notional: Some(to_pb_decimal(
            b.pending_match_buy_unresolved_notional,
            &units.currency_amount,
        )),
        pending_match_sell_unresolved_notional: Some(to_pb_decimal(
            b.pending_match_sell_unresolved_notional,
            &units.currency_amount,
        )),
        sell_ambiguity_locked: b.sell_ambiguity_locked,
    }
}

#[must_use]
pub fn to_pb_position_view(
    v: &ValuedInstrumentPositionView,
    units: &AnalysisUnits,
) -> pb::InstrumentPositionView {
    pb::InstrumentPositionView {
        account_id: v.position.account_id.clone(),
        instrument_code: v.position.instrument_code.clone(),
        position_version: v.position.position_version,
        economic_net_notional: Some(to_pb_decimal(
            v.position.economic_net_notional,
            &units.currency_amount,
        )),
        available_t0_notional: Some(to_pb_decimal(
            v.position.available_t0_notional,
            &units.currency_amount,
        )),
        available_t1_notional: Some(to_pb_decimal(
            v.position.available_t1_notional,
            &units.currency_amount,
        )),
        bucket_t0: Some(to_pb_bucket_balance(&v.position.bucket_t0, units)),
        bucket_t1: Some(to_pb_bucket_balance(&v.position.bucket_t1, units)),
        coverage_status: to_pb_coverage_status(v.coverage_status),
        tax_basis: to_pb_tax_basis(v.tax_basis),
        pre_tax_dv01: v.pre_tax_dv01.map(|d| to_pb_decimal(d, &units.dv01)),
        valuation_clean_price_per_100: v
            .valuation_clean_price_per_100
            .map(|p| to_pb_decimal(p, &units.price_per_100)),
        valuation_yield_to_maturity: v
            .valuation_yield_to_maturity
            .map(|y| to_pb_decimal(y, &units.rate)),
        calendar_resolution: to_pb_calendar_resolution(v.calendar_resolution),
        uncovered_reason: to_pb_uncovered_reason(v.uncovered_reason),
        uncovered_detail: v.uncovered_detail.clone(),
        valuation_settlement_date: v.valuation_settlement_date.to_string(),
        active_quote_source_id: v.active_quote_source_id.clone(),
        active_quote_seq: v.active_quote_seq,
    }
}

#[must_use]
pub fn to_pb_earmark_record(e: &EarmarkRecord, units: &AnalysisUnits) -> pb::EarmarkRecord {
    pb::EarmarkRecord {
        earmark_id: Some(to_pb_ulid(&e.earmark_id)),
        earmark_revision: e.earmark_revision,
        account_id: e.account_id.clone(),
        instrument_code: e.instrument_code.clone(),
        side: to_pb_side(e.side),
        settlement_bucket: to_pb_bucket(e.settlement_bucket),
        settlement_date: e.settlement_date.to_string(),
        notional: Some(to_pb_decimal(e.notional, &units.currency_amount)),
        unreconciled_notional: Some(to_pb_decimal(
            e.unreconciled_notional,
            &units.currency_amount,
        )),
        quote_terms: e
            .quote_terms
            .as_ref()
            .map(|qt| to_pb_quote_terms(qt, units)),
        locked_settlement_cash: e
            .locked_settlement_cash
            .map(|c| to_pb_decimal(c, &units.currency_amount)),
        unreconciled_settlement_cash: e
            .unreconciled_settlement_cash
            .map(|c| to_pb_decimal(c, &units.currency_amount)),
        funding_precision: to_pb_funding_precision(e.funding_precision),
        state: to_pb_earmark_state(e.state),
        created_by: e.created_by.clone(),
        created_at: Some(to_pb_market_time(&e.created_at)),
        expires_at: e.expires_at.as_ref().map(to_pb_market_time),
        committed_at: e.committed_at.as_ref().map(to_pb_market_time),
        overdue_alert: e.overdue_alert,
        pre_close_alert: e.pre_close_alert,
        reopened_by_reversal: e.reopened_by_reversal,
        note: e.note.clone(),
        created_seq: e.created_seq,
        updated_seq: e.updated_seq,
        confirmed_by_trader_id: e.confirmed_by_trader_id.clone(),
        cancel_actor_id: e.cancel_actor_id.clone(),
        cancel_reason: e.cancel_reason.clone(),
    }
}

#[must_use]
pub fn to_pb_pending_match(
    pm: &PendingMatchRecord,
    units: &AnalysisUnits,
) -> pb::PendingMatchRecord {
    pb::PendingMatchRecord {
        pending_match_id: Some(to_pb_ulid(&pm.pending_match_id)),
        pending_revision: pm.pending_revision,
        account_id: pm.account_id.clone(),
        instrument_code: pm.instrument_code.clone(),
        side: to_pb_side(pm.side),
        settlement_bucket: to_pb_bucket(pm.settlement_bucket),
        settlement_date: pm.settlement_date.to_string(),
        source_id: pm.source_id.clone(),
        external_trade_id: pm.external_trade_id.clone(),
        source_revision: pm.source_revision,
        original_notional: Some(to_pb_decimal(pm.original_notional, &units.currency_amount)),
        unresolved_notional: Some(to_pb_decimal(
            pm.unresolved_notional,
            &units.currency_amount,
        )),
        prior_baseline_notional: pm
            .prior_baseline_notional
            .map(|n| to_pb_decimal(n, &units.currency_amount)),
        quote_terms: pm
            .quote_terms
            .as_ref()
            .map(|qt| to_pb_quote_terms(qt, units)),
        executed_at: Some(to_pb_market_time(&pm.executed_at)),
        state: to_pb_pending_state(pm.state),
        reason: to_pb_pending_reason(pm.reason),
        candidate_earmark_ids: pm.candidate_earmark_ids.iter().map(to_pb_ulid).collect(),
        created_seq: pm.created_seq,
        updated_seq: pm.updated_seq,
        unresolved_signed_cash: pm
            .unresolved_signed_cash
            .map(|c| to_pb_decimal(c, &units.currency_amount)),
        evidence: pm
            .evidence
            .as_ref()
            .map(|ev| to_pb_base_evidence(ev, units)),
        applied_delta: pm
            .applied_delta
            .as_ref()
            .map(|d| to_pb_baseline_delta(d, units)),
        resolution_actor_id: pm.resolution_actor_id.clone(),
        resolution_reason: pm.resolution_reason.clone(),
    }
}

#[must_use]
pub fn to_pb_allocation_view(
    a: &ReconcileAllocationRecord,
    units: &AnalysisUnits,
) -> pb::ReconcileAllocationView {
    pb::ReconcileAllocationView {
        allocation_id: Some(to_pb_ulid(&a.allocation_id)),
        source_id: a.source_id.clone(),
        external_trade_id: a.external_trade_id.clone(),
        source_revision: a.source_revision,
        earmark_id: a.earmark_id.as_ref().map(to_pb_ulid),
        pending_match_id: a.pending_match_id.as_ref().map(to_pb_ulid),
        allocated_notional: Some(to_pb_decimal(a.allocated_notional, &units.currency_amount)),
        allocated_fill_cash: a
            .allocated_fill_cash
            .map(|c| to_pb_decimal(c, &units.currency_amount)),
        reversed_by_revision: a.reversed_by_revision,
        released_earmark_cash: a
            .released_earmark_cash
            .map(|c| to_pb_decimal(c, &units.currency_amount)),
        baseline_delta: a
            .baseline_delta
            .as_ref()
            .map(|d| to_pb_baseline_delta(d, units)),
        actor_id: a.actor_id.clone(),
    }
}

#[must_use]
pub fn to_pb_risk_summary(
    r: &RiskAndCoverageSummary,
    units: &AnalysisUnits,
) -> pb::RiskAndCoverageSummary {
    pb::RiskAndCoverageSummary {
        covered_instrument_count: r.covered_instrument_count,
        uncovered_instrument_count: r.uncovered_instrument_count,
        covered_tax_basis: to_pb_tax_basis(r.covered_tax_basis),
        covered_net_pre_tax_dv01: Some(to_pb_decimal(r.covered_net_pre_tax_dv01, &units.dv01)),
        covered_gross_notional: Some(to_pb_decimal(
            r.covered_gross_notional,
            &units.currency_amount,
        )),
        uncovered_gross_notional: Some(to_pb_decimal(
            r.uncovered_gross_notional,
            &units.currency_amount,
        )),
        uncovered_net_notional: Some(to_pb_decimal(
            r.uncovered_net_notional,
            &units.currency_amount,
        )),
        whole_account_hedge_allowed: r.whole_account_hedge_allowed,
    }
}

#[must_use]
pub fn to_pb_funding_summary(
    f: &FundingGapSummary,
    units: &AnalysisUnits,
) -> pb::FundingGapSummary {
    pb::FundingGapSummary {
        opening_cash_t0: Some(to_pb_decimal(f.opening_cash_t0, &units.currency_amount)),
        opening_net_cash_delta_t1: Some(to_pb_decimal(
            f.opening_net_cash_delta_t1,
            &units.currency_amount,
        )),
        net_settlement_flow_t0: Some(to_pb_decimal(
            f.net_settlement_flow_t0,
            &units.currency_amount,
        )),
        net_settlement_flow_t1: Some(to_pb_decimal(
            f.net_settlement_flow_t1,
            &units.currency_amount,
        )),
        projected_cash_t0: Some(to_pb_decimal(f.projected_cash_t0, &units.currency_amount)),
        projected_cash_t1: Some(to_pb_decimal(f.projected_cash_t1, &units.currency_amount)),
        funding_gap_t0: Some(to_pb_decimal(f.funding_gap_t0, &units.currency_amount)),
        funding_gap_t1: Some(to_pb_decimal(f.funding_gap_t1, &units.currency_amount)),
        clean_price_estimated_funding_count: f.clean_price_estimated_funding_count,
        unpriced_funding_earmark_count: f.unpriced_funding_earmark_count,
        funding_is_complete_exact: f.funding_is_complete_exact,
        pending_match_cash_t0: Some(to_pb_decimal(
            f.pending_match_cash_t0,
            &units.currency_amount,
        )),
        pending_match_cash_t1: Some(to_pb_decimal(
            f.pending_match_cash_t1,
            &units.currency_amount,
        )),
        pending_match_count: f.pending_match_count,
        pending_unknown_cash_count: f.pending_unknown_cash_count,
        feed_incomplete: f.feed_incomplete,
    }
}

#[must_use]
pub fn to_pb_seq_range(r: SeqRange) -> pb::SeqRange {
    pb::SeqRange {
        first: r.first,
        last: r.last,
    }
}

#[must_use]
pub fn to_pb_feed_status(f: &FeedStatusView) -> pb::FeedStatusView {
    pb::FeedStatusView {
        source_id: f.source_id.clone(),
        feed_status_revision: f.feed_status_revision,
        health_state: to_pb_feed_health(f.health_state),
        earmarks_paused: f.earmarks_paused,
        paused_by_gap: f.paused_by_gap,
        paused_by_staleness: f.paused_by_staleness,
        paused_by_operator: f.paused_by_operator,
        last_heartbeat_at: f.last_heartbeat_at.as_ref().map(to_pb_market_time),
        contiguous_feed_seq: f.contiguous_feed_seq,
        max_seen_feed_seq: f.max_seen_feed_seq,
        missing_feed_ranges: f
            .missing_feed_ranges
            .iter()
            .copied()
            .map(to_pb_seq_range)
            .collect(),
        open_pending_match_count: f.open_pending_match_count,
        status_reason: f.status_reason.clone(),
    }
}

#[must_use]
pub fn to_pb_snapshot(snap: &EnrichedAccountBlotterSnapshot) -> pb::AccountBlotterSnapshot {
    let units = &snap.opening_base.units;
    pb::AccountBlotterSnapshot {
        snapshot_seq: snap.snapshot_seq,
        account_id: snap.account_id.clone(),
        account_version: snap.account_version,
        opening_base: Some(to_pb_opening_position_base(&snap.opening_base)),
        risk_summary: Some(to_pb_risk_summary(&snap.risk_summary, units)),
        funding_summary: Some(to_pb_funding_summary(&snap.funding_summary, units)),
        feed_statuses: snap.feed_statuses.iter().map(to_pb_feed_status).collect(),
        positions: snap
            .positions
            .iter()
            .map(|p| to_pb_position_view(p, units))
            .collect(),
        active_earmarks: snap
            .active_earmarks
            .iter()
            .map(|e| to_pb_earmark_record(e, units))
            .collect(),
        open_pending_matches: snap
            .open_pending_matches
            .iter()
            .map(|pm| to_pb_pending_match(pm, units))
            .collect(),
        pending_drafts: snap
            .pending_drafts
            .iter()
            .map(|d| to_pb_draft_record(d, units))
            .collect(),
        allowed_hedge_product_codes: snap.allowed_hedge_product_codes.clone(),
    }
}

#[must_use]
pub fn to_pb_account_change(change: &AccountChange) -> pb::AccountChange {
    let units = &change.replace_snapshot.opening_base.units;
    pb::AccountChange {
        account_id: change.account_id.clone(),
        replace_snapshot: Some(to_pb_snapshot(&change.replace_snapshot)),
        changed_allocations: change
            .changed_allocations
            .iter()
            .map(|a| to_pb_allocation_view(a, units))
            .collect(),
    }
}

#[must_use]
pub fn to_pb_event_envelope(env: &BlotterEventEnvelope) -> pb::BlotterEventEnvelope {
    pb::BlotterEventEnvelope {
        seq: env.seq,
        occurred_at: Some(to_pb_market_time(&env.occurred_at)),
        changes: env.changes.iter().map(to_pb_account_change).collect(),
        event_type: env.event_type.clone(),
    }
}

#[must_use]
pub fn to_pb_snapshot_fallback(fb: &SnapshotFallbackRequired) -> pb::SnapshotFallbackRequired {
    pb::SnapshotFallbackRequired {
        requested_after_seq: fb.requested_after_seq,
        oldest_retained_seq: fb.oldest_retained_seq,
        current_head_seq: fb.current_head_seq,
        reason: fb.reason.clone(),
    }
}

#[must_use]
pub fn to_pb_poll_events_batch(batch: &PollEventsBatch) -> pb::PollEventsBatch {
    pb::PollEventsBatch {
        events: batch.events.iter().map(to_pb_event_envelope).collect(),
        scanned_through_seq: batch.scanned_through_seq,
        current_head_seq: batch.current_head_seq,
        has_more: batch.has_more,
    }
}

#[must_use]
pub fn to_pb_trade_fill_outcome(o: &TradeFillIngestOutcome) -> pb::TradeFillIngestOutcome {
    pb::TradeFillIngestOutcome {
        source_id: o.source_id.clone(),
        external_trade_id: o.external_trade_id.clone(),
        source_revision: o.source_revision,
        feed_seq: o.feed_seq,
        kind: to_pb_outcome_kind(o.kind),
        pending_match_id: o.pending_match_id.as_ref().map(to_pb_ulid),
        allocation_ids: o.allocation_ids.iter().map(to_pb_ulid).collect(),
        effective_revision: o.effective_revision,
    }
}

#[must_use]
pub fn to_pb_hedge_measures(
    m: &PreviewCoveredHedgeMeasures,
    units: &AnalysisUnits,
) -> rates_pb::FuturesHedgeMeasures {
    rates_pb::FuturesHedgeMeasures {
        futures_contract_dv01: Some(to_pb_decimal(m.futures_contract_dv01, &units.dv01)),
        raw_contracts: Some(to_pb_decimal(m.raw_contracts, &units.contract_count)),
        recommended_contracts: m.recommended_contracts,
        residual_dv01: Some(to_pb_decimal(m.residual_dv01, &units.dv01)),
        hedge_effectiveness: Some(to_pb_decimal(m.hedge_effectiveness, &units.dimensionless)),
        contract_notional: Some(to_pb_decimal(m.contract_notional, &units.currency_amount)),
    }
}

#[must_use]
pub fn to_pb_preview_covered_hedge_result(
    out: &PreviewCoveredHedgeOutput,
    units: &AnalysisUnits,
) -> pb::PreviewCoveredHedgeResult {
    pb::PreviewCoveredHedgeResult {
        evaluated_at_seq: out.evaluated_at_seq,
        scope: to_pb_preview_hedge_scope(out.scope),
        product_code: out.product_code.clone(),
        contract_code: out.contract_code.clone(),
        zero_net_exposure: out.zero_net_exposure,
        covered_tax_basis: to_pb_tax_basis(out.covered_tax_basis),
        covered_instrument_count: out.covered_instrument_count,
        uncovered_instrument_count: out.uncovered_instrument_count,
        covered_net_pre_tax_dv01: Some(to_pb_decimal(out.covered_net_pre_tax_dv01, &units.dv01)),
        excluded_uncovered_gross_notional: Some(to_pb_decimal(
            out.excluded_uncovered_gross_notional,
            &units.currency_amount,
        )),
        measures: Some(to_pb_hedge_measures(&out.measures, units)),
    }
}

pub fn parse_quote_cursor(c: Option<&pb::QuoteCursor>) -> ApplicationResult<QuoteCursor> {
    let c = c.ok_or_else(invalid)?;
    Ok(QuoteCursor {
        source_id: parse_non_empty(&c.source_id)?,
        instrument_code: parse_non_empty(&c.instrument_code)?,
        after_quote_seq: c.after_quote_seq,
    })
}

#[must_use]
pub fn to_pb_quote_cursor(c: &QuoteCursor) -> pb::QuoteCursor {
    pb::QuoteCursor {
        source_id: c.source_id.clone(),
        instrument_code: c.instrument_code.clone(),
        after_quote_seq: c.after_quote_seq,
    }
}

#[must_use]
pub fn to_pb_pull_trade_feed_result(
    res: &PullTradeFeedBatchResult,
    units: &AnalysisUnits,
) -> pb::PullTradeFeedBatchResult {
    pb::PullTradeFeedBatchResult {
        fills: res
            .fills
            .iter()
            .map(|f| to_pb_trade_fill_record(f, units))
            .collect(),
        scanned_through_feed_seq: res.scanned_through_feed_seq,
        has_more: res.has_more,
    }
}

#[must_use]
pub fn to_pb_pull_market_quotes_result(
    res: &PullMarketQuotesResult,
    units: &AnalysisUnits,
) -> pb::PullMarketQuotesResult {
    pb::PullMarketQuotesResult {
        ticks: res
            .ticks
            .iter()
            .map(|t| to_pb_market_quote_tick(t, units))
            .collect(),
        next_cursor: Some(to_pb_quote_cursor(&res.next_cursor)),
        has_more: res.has_more,
    }
}

// ============================================================================
// Canonical Request Hashing (fixed field order, length-prefixed, presence/oneof)
// ============================================================================

#[must_use]
pub fn hash_import_opening_base(
    base: &FullOpeningPositionBase,
    expected_account_version: u64,
) -> ContentHash {
    let mut buf = Vec::new();
    append_str(&mut buf, "ficant.blotter.req.ImportOpeningBase.v1");
    append_u64(&mut buf, expected_account_version);
    append_bytes(&mut buf, base.core.canonical_hash().as_bytes());
    append_units(&mut buf, &base.units);
    append_opt_u8(
        &mut buf,
        base.default_calendar_requirement.map(|r| match r {
            CalendarRequirement::ReferenceReplay => 1,
            CalendarRequirement::ExactMarket => 2,
        }),
    );
    match &base.calendar {
        Some(cal) => {
            append_u8(&mut buf, 1);
            append_str(&mut buf, cal.id());
            append_u64(&mut buf, cal.version().get());
            append_bytes(&mut buf, cal.content_hash().as_bytes());
            append_str(&mut buf, &cal.coverage_start().to_string());
            append_str(&mut buf, &cal.coverage_end().to_string());
        }
        None => append_u8(&mut buf, 0),
    }
    let mut sorted_insts = base.instruments.clone();
    sorted_insts.sort_by(|a, b| a.instrument_code.cmp(&b.instrument_code));
    append_u64(
        &mut buf,
        u64::try_from(sorted_insts.len()).unwrap_or(u64::MAX),
    );
    for inst in &sorted_insts {
        append_str(&mut buf, &inst.instrument_code);
        append_opt_u8(
            &mut buf,
            inst.eligibility_class.map(|e| match e {
                RateCoverageEligibility::SovereignOrPolicyFixed => 1,
                RateCoverageEligibility::CreditOrSpreadProduct => 2,
                RateCoverageEligibility::FloatingOrIndexed => 3,
                RateCoverageEligibility::EmbeddedOptionOrSinking => 4,
            }),
        );
        append_opt_u8(
            &mut buf,
            inst.calendar_requirement.map(|r| match r {
                CalendarRequirement::ReferenceReplay => 1,
                CalendarRequirement::ExactMarket => 2,
            }),
        );
        match &inst.bond {
            Some(bond) => {
                append_u8(&mut buf, 1);
                append_bond(&mut buf, bond);
            }
            None => append_u8(&mut buf, 0),
        }
    }
    ContentHash::digest(&buf)
}

fn append_bond(buf: &mut Vec<u8>, bond: &Bond) {
    append_str(buf, bond.instrument().id().as_str());
    append_u64(buf, bond.instrument().version().get());
    append_str(buf, &bond.first_issue_date().to_string());
    append_str(buf, &bond.current_issue_date().to_string());
    append_str(buf, &bond.maturity_date().to_string());
    append_domain_decimal(buf, bond.face_value());
    append_domain_decimal(buf, bond.cumulative_issued_amount());
    match bond.tax_attributes() {
        Some(ta) => {
            append_u8(buf, 1);
            append_u8(
                buf,
                match ta.value_added_tax_status() {
                    ValueAddedTaxStatus::Exempt => 1,
                    ValueAddedTaxStatus::Taxable => 2,
                },
            );
            append_u8(
                buf,
                match ta.income_tax_status() {
                    IncomeTaxStatus::Exempt => 1,
                    IncomeTaxStatus::Taxable => 2,
                },
            );
        }
        None => append_u8(buf, 0),
    }
    match bond.pricing_terms() {
        Some(pt) => {
            append_u8(buf, 1);
            append_domain_decimal(buf, pt.coupon_rate());
            append_u8(
                buf,
                match pt.frequency() {
                    BondCouponFrequency::Annual => 1,
                    BondCouponFrequency::Semiannual => 2,
                },
            );
            append_u8(
                buf,
                match pt.day_count() {
                    BondDayCountConvention::ActActBondIsma => 1,
                },
            );
            append_u8(
                buf,
                match pt.business_day() {
                    BondBusinessDayConvention::Following => 1,
                },
            );
        }
        None => append_u8(buf, 0),
    }
}

#[must_use]
pub fn hash_ingest_market_quotes(
    account_id: &str,
    source_id: &str,
    ticks: &[CanonicalMarketQuoteTick],
) -> ContentHash {
    let mut buf = Vec::new();
    append_str(&mut buf, "ficant.blotter.req.IngestMarketQuotes.v1");
    append_str(&mut buf, account_id);
    append_str(&mut buf, source_id);
    let mut keyed: Vec<(&CanonicalMarketQuoteTick, ContentHash)> = ticks
        .iter()
        .map(|t| (t, canonical_market_quote_tick_hash(t)))
        .collect();
    keyed.sort_by(|(a, ha), (b, hb)| {
        (
            &a.tick.source_id,
            &a.tick.instrument_code,
            a.tick.quote_seq,
            ha.as_bytes(),
        )
            .cmp(&(
                &b.tick.source_id,
                &b.tick.instrument_code,
                b.tick.quote_seq,
                hb.as_bytes(),
            ))
    });
    append_u64(&mut buf, u64::try_from(keyed.len()).unwrap_or(u64::MAX));
    for (_, tick_hash) in keyed {
        append_bytes(&mut buf, tick_hash.as_bytes());
    }
    ContentHash::digest(&buf)
}

fn canonical_market_quote_tick_hash(t: &CanonicalMarketQuoteTick) -> ContentHash {
    let mut buf = Vec::new();
    append_bytes(&mut buf, t.tick.canonical_payload_hash().as_bytes());
    append_opt_unit_ref(&mut buf, t.bid_clean_price_unit.as_ref());
    append_opt_unit_ref(&mut buf, t.ask_clean_price_unit.as_ref());
    append_opt_unit_ref(&mut buf, t.mid_clean_price_unit.as_ref());
    append_opt_unit_ref(&mut buf, t.mid_yield_to_maturity_unit.as_ref());
    ContentHash::digest(&buf)
}

#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn hash_create_hold(
    account_id: &str,
    expected_position_version: u64,
    instrument_code: &str,
    side: TradeSide,
    bucket: SettlementBucket,
    notional: &CanonicalDecimal,
    quote_terms: Option<&CanonicalQuoteTerms>,
    ttl_seconds: u32,
    note: &str,
) -> ContentHash {
    let mut buf = Vec::new();
    append_str(&mut buf, "ficant.blotter.req.CreateHold.v1");
    append_str(&mut buf, account_id);
    append_u64(&mut buf, expected_position_version);
    append_str(&mut buf, instrument_code);
    append_u8(&mut buf, side_tag(side));
    append_u8(&mut buf, bucket_tag(bucket));
    append_canonical_decimal(&mut buf, notional);
    append_opt_canonical_quote_terms(&mut buf, quote_terms);
    append_u32(&mut buf, ttl_seconds);
    append_str(&mut buf, note);
    ContentHash::digest(&buf)
}

#[must_use]
pub fn hash_extend_hold(
    account_id: &str,
    earmark_id: &Ulid,
    expected_earmark_revision: u64,
    ttl_seconds: u32,
) -> ContentHash {
    let mut buf = Vec::new();
    append_str(&mut buf, "ficant.blotter.req.ExtendHold.v1");
    append_str(&mut buf, account_id);
    append_str(&mut buf, earmark_id.as_str());
    append_u64(&mut buf, expected_earmark_revision);
    append_u32(&mut buf, ttl_seconds);
    ContentHash::digest(&buf)
}

#[must_use]
pub fn hash_confirm_hold(
    account_id: &str,
    earmark_id: &Ulid,
    expected_earmark_revision: u64,
    expected_position_version: u64,
    quote_terms: &CanonicalQuoteTerms,
) -> ContentHash {
    let mut buf = Vec::new();
    append_str(&mut buf, "ficant.blotter.req.ConfirmHold.v1");
    append_str(&mut buf, account_id);
    append_str(&mut buf, earmark_id.as_str());
    append_u64(&mut buf, expected_earmark_revision);
    append_u64(&mut buf, expected_position_version);
    append_opt_canonical_quote_terms(&mut buf, Some(quote_terms));
    ContentHash::digest(&buf)
}

#[must_use]
pub fn hash_release_hold(
    account_id: &str,
    earmark_id: &Ulid,
    expected_earmark_revision: u64,
    reason: &str,
) -> ContentHash {
    let mut buf = Vec::new();
    append_str(&mut buf, "ficant.blotter.req.ReleaseHold.v1");
    append_str(&mut buf, account_id);
    append_str(&mut buf, earmark_id.as_str());
    append_u64(&mut buf, expected_earmark_revision);
    append_str(&mut buf, reason);
    ContentHash::digest(&buf)
}

#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn hash_create_committed(
    account_id: &str,
    expected_position_version: u64,
    instrument_code: &str,
    side: TradeSide,
    bucket: SettlementBucket,
    notional: &CanonicalDecimal,
    quote_terms: &CanonicalQuoteTerms,
    note: &str,
) -> ContentHash {
    let mut buf = Vec::new();
    append_str(&mut buf, "ficant.blotter.req.CreateCommitted.v1");
    append_str(&mut buf, account_id);
    append_u64(&mut buf, expected_position_version);
    append_str(&mut buf, instrument_code);
    append_u8(&mut buf, side_tag(side));
    append_u8(&mut buf, bucket_tag(bucket));
    append_canonical_decimal(&mut buf, notional);
    append_opt_canonical_quote_terms(&mut buf, Some(quote_terms));
    append_str(&mut buf, note);
    ContentHash::digest(&buf)
}

#[must_use]
pub fn hash_cancel_committed(
    account_id: &str,
    earmark_id: &Ulid,
    expected_earmark_revision: u64,
    reason: &str,
) -> ContentHash {
    let mut buf = Vec::new();
    append_str(&mut buf, "ficant.blotter.req.CancelCommitted.v1");
    append_str(&mut buf, account_id);
    append_str(&mut buf, earmark_id.as_str());
    append_u64(&mut buf, expected_earmark_revision);
    append_str(&mut buf, reason);
    ContentHash::digest(&buf)
}

#[must_use]
pub fn hash_submit_draft(account_id: &str, proposal: &CanonicalNlpEarmarkProposal) -> ContentHash {
    let mut buf = Vec::new();
    append_str(&mut buf, "ficant.blotter.req.SubmitDraftEarmark.v1");
    append_str(&mut buf, account_id);
    append_str(&mut buf, &proposal.parser_source_id);
    append_str(&mut buf, &proposal.model_version);
    append_str(&mut buf, &proposal.raw_source_text);
    append_u64(
        &mut buf,
        u64::try_from(proposal.anchors.len()).unwrap_or(u64::MAX),
    );
    for a in &proposal.anchors {
        append_str(&mut buf, &a.field_name);
        append_u32(&mut buf, a.byte_start);
        append_u32(&mut buf, a.byte_end);
        append_str(&mut buf, &a.span_text);
    }
    append_canonical_decimal(&mut buf, &proposal.confidence_score);
    append_str(&mut buf, &proposal.account_id);
    append_str(&mut buf, &proposal.instrument_code);
    append_u8(&mut buf, side_tag(proposal.side));
    append_u8(&mut buf, bucket_tag(proposal.settlement_bucket));
    append_opt_canonical_decimal(&mut buf, proposal.notional.as_ref());
    append_opt_canonical_quote_terms(&mut buf, proposal.quote_terms.as_ref());
    append_u8(
        &mut buf,
        match proposal.suggested_target_state {
            DraftTargetState::Hold => 1,
            DraftTargetState::Committed => 2,
        },
    );
    ContentHash::digest(&buf)
}

#[must_use]
pub fn hash_confirm_draft(
    account_id: &str,
    draft_id: &Ulid,
    expected_draft_revision: u64,
    expected_position_version: u64,
    confirmation_token: &ContentHash,
    acknowledge_uncovered_dv01: bool,
) -> ContentHash {
    let mut buf = Vec::new();
    append_str(&mut buf, "ficant.blotter.req.ConfirmDraftEarmark.v1");
    append_str(&mut buf, account_id);
    append_str(&mut buf, draft_id.as_str());
    append_u64(&mut buf, expected_draft_revision);
    append_u64(&mut buf, expected_position_version);
    append_bytes(&mut buf, confirmation_token.as_bytes());
    append_u8(&mut buf, u8::from(acknowledge_uncovered_dv01));
    ContentHash::digest(&buf)
}

/// Canonical trade-feed batch hash sorted by `(feed_seq, external_trade_id, source_revision)`
/// while preserving multiplicity (duplicate count) and all `DecimalValue` `UnitRef` bindings.
#[must_use]
pub fn hash_ingest_trade_feed(
    source_id: &str,
    fills: &[CanonicalTradeFillRecord],
    heartbeat_at: Option<&MarketTime>,
) -> ContentHash {
    let mut buf = Vec::new();
    append_str(&mut buf, "ficant.blotter.req.IngestTradeFeed.v1");
    append_str(&mut buf, source_id);
    match heartbeat_at {
        Some(mt) => {
            append_u8(&mut buf, 1);
            append_market_time(&mut buf, mt);
        }
        None => append_u8(&mut buf, 0),
    }
    let mut keyed: Vec<(&CanonicalTradeFillRecord, ContentHash)> = fills
        .iter()
        .map(|f| (f, canonical_trade_fill_hash(f)))
        .collect();
    keyed.sort_by(|(a, ha), (b, hb)| {
        (
            a.record.feed_seq,
            &a.record.external_trade_id,
            a.record.source_revision,
            ha.as_bytes(),
        )
            .cmp(&(
                b.record.feed_seq,
                &b.record.external_trade_id,
                b.record.source_revision,
                hb.as_bytes(),
            ))
    });
    append_u64(&mut buf, u64::try_from(keyed.len()).unwrap_or(u64::MAX));
    for (_, fill_hash) in keyed {
        append_bytes(&mut buf, fill_hash.as_bytes());
    }
    ContentHash::digest(&buf)
}

fn canonical_trade_fill_hash(f: &CanonicalTradeFillRecord) -> ContentHash {
    let mut buf = Vec::new();
    append_bytes(&mut buf, f.record.payload_hash().as_bytes());
    append_unit_ref(&mut buf, &f.notional_unit);
    append_opt_canonical_quote_terms(&mut buf, f.quote_terms.as_ref());
    ContentHash::digest(&buf)
}

#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn hash_resolve_pending_match(
    account_id: &str,
    pending_match_id: &Ulid,
    expected_pending_revision: u64,
    expected_position_version: u64,
    action: PendingMatchResolutionAction,
    expected_earmark_revision: Option<u64>,
    target_earmark_id: Option<&Ulid>,
    offset_notional: Option<&CanonicalDecimal>,
    reason: &str,
    baseline_delta: Option<&CanonicalBaselineDelta>,
) -> ContentHash {
    let mut buf = Vec::new();
    append_str(&mut buf, "ficant.blotter.req.ResolvePendingMatch.v1");
    append_str(&mut buf, account_id);
    append_str(&mut buf, pending_match_id.as_str());
    append_u64(&mut buf, expected_pending_revision);
    append_u64(&mut buf, expected_position_version);
    append_u8(
        &mut buf,
        match action {
            PendingMatchResolutionAction::MatchFull => 1,
            PendingMatchResolutionAction::SplitOffset => 2,
            PendingMatchResolutionAction::AcceptOrphan => 3,
            PendingMatchResolutionAction::ReleaseReject => 4,
            PendingMatchResolutionAction::ApplyBaselineDelta => 5,
        },
    );
    match expected_earmark_revision {
        Some(v) => {
            append_u8(&mut buf, 1);
            append_u64(&mut buf, v);
        }
        None => append_u8(&mut buf, 0),
    }
    match target_earmark_id {
        Some(id) => {
            append_u8(&mut buf, 1);
            append_str(&mut buf, id.as_str());
        }
        None => append_u8(&mut buf, 0),
    }
    append_opt_canonical_decimal(&mut buf, offset_notional);
    append_str(&mut buf, reason);
    match baseline_delta {
        Some(d) => {
            append_u8(&mut buf, 1);
            append_canonical_decimal(&mut buf, &d.delta_notional);
            append_canonical_decimal(&mut buf, &d.delta_cash);
            append_u8(&mut buf, bucket_tag(d.settlement_bucket));
            append_bytes(&mut buf, d.evidence.evidence.canonical_hash().as_bytes());
            append_unit_ref(&mut buf, &d.evidence.old_notional_unit);
            append_unit_ref(&mut buf, &d.evidence.old_signed_cash_unit);
            append_str(&mut buf, &d.reason);
        }
        None => append_u8(&mut buf, 0),
    }
    ContentHash::digest(&buf)
}

#[must_use]
pub fn hash_sweep_lifecycle(account_id: Option<&str>, source_id: Option<&str>) -> ContentHash {
    let mut buf = Vec::new();
    append_str(&mut buf, "ficant.blotter.req.SweepLifecycle.v1");
    match account_id {
        Some(a) => {
            append_u8(&mut buf, 1);
            append_str(&mut buf, a);
        }
        None => append_u8(&mut buf, 0),
    }
    match source_id {
        Some(s) => {
            append_u8(&mut buf, 1);
            append_str(&mut buf, s);
        }
        None => append_u8(&mut buf, 0),
    }
    ContentHash::digest(&buf)
}

#[must_use]
pub fn hash_set_feed_control(
    source_id: &str,
    expected_feed_status_revision: u64,
    paused_by_operator: bool,
    force_clear_gap_pause: bool,
    reason: &str,
) -> ContentHash {
    let mut buf = Vec::new();
    append_str(&mut buf, "ficant.blotter.req.SetFeedControl.v1");
    append_str(&mut buf, source_id);
    append_u64(&mut buf, expected_feed_status_revision);
    append_u8(&mut buf, u8::from(paused_by_operator));
    append_u8(&mut buf, u8::from(force_clear_gap_pause));
    append_str(&mut buf, reason);
    ContentHash::digest(&buf)
}

// ============================================================================
// Low-level canonical encoding helpers
// ============================================================================

const fn side_tag(side: TradeSide) -> u8 {
    match side {
        TradeSide::Buy => 1,
        TradeSide::Sell => 2,
    }
}

const fn bucket_tag(bucket: SettlementBucket) -> u8 {
    match bucket {
        SettlementBucket::TPlus0 => 1,
        SettlementBucket::TPlus1 => 2,
    }
}

fn append_u8(buf: &mut Vec<u8>, v: u8) {
    buf.push(v);
}

fn append_opt_u8(buf: &mut Vec<u8>, v: Option<u8>) {
    match v {
        Some(x) => {
            buf.push(1);
            buf.push(x);
        }
        None => buf.push(0),
    }
}

fn append_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_be_bytes());
}

fn append_u64(buf: &mut Vec<u8>, v: u64) {
    buf.extend_from_slice(&v.to_be_bytes());
}

fn append_fixed(buf: &mut Vec<u8>, v: FixedDecimal) {
    buf.extend_from_slice(&v.scaled().to_be_bytes());
}

fn append_domain_decimal(buf: &mut Vec<u8>, v: &DomainDecimalValue) {
    append_str(buf, v.coefficient());
    append_u32(buf, v.scale());
    append_unit_ref(buf, v.unit());
}

fn append_canonical_decimal(buf: &mut Vec<u8>, v: &CanonicalDecimal) {
    append_fixed(buf, v.value);
    append_unit_ref(buf, &v.unit);
}

fn append_opt_canonical_decimal(buf: &mut Vec<u8>, v: Option<&CanonicalDecimal>) {
    match v {
        Some(x) => {
            append_u8(buf, 1);
            append_canonical_decimal(buf, x);
        }
        None => append_u8(buf, 0),
    }
}

fn append_bytes(buf: &mut Vec<u8>, bytes: &[u8]) {
    append_u64(buf, u64::try_from(bytes.len()).unwrap_or(u64::MAX));
    buf.extend_from_slice(bytes);
}

fn append_str(buf: &mut Vec<u8>, s: &str) {
    append_bytes(buf, s.as_bytes());
}

fn append_unit_ref(buf: &mut Vec<u8>, u: &UnitRef) {
    append_str(buf, u.unit_id().as_str());
    append_u64(buf, u.version().get());
}

fn append_opt_unit_ref(buf: &mut Vec<u8>, u: Option<&UnitRef>) {
    match u {
        Some(unit) => {
            append_u8(buf, 1);
            append_unit_ref(buf, unit);
        }
        None => append_u8(buf, 0),
    }
}

fn append_units(buf: &mut Vec<u8>, u: &AnalysisUnits) {
    append_unit_ref(buf, &u.currency_amount);
    append_unit_ref(buf, &u.price_per_100);
    append_unit_ref(buf, &u.rate);
    append_unit_ref(buf, &u.dv01);
    append_unit_ref(buf, &u.dv01_per_100);
    append_unit_ref(buf, &u.dimensionless);
    append_unit_ref(buf, &u.contract_count);
}

fn append_market_time(buf: &mut Vec<u8>, mt: &MarketTime) {
    buf.extend_from_slice(&mt.instant().timestamp_micros().to_be_bytes());
    append_str(buf, mt.market_timezone());
    append_str(buf, &mt.local_trading_date().to_string());
}

fn append_opt_canonical_quote_terms(buf: &mut Vec<u8>, qt: Option<&CanonicalQuoteTerms>) {
    match qt {
        Some(q) => {
            append_u8(buf, 1);
            match &q.basis {
                Some(CanonicalQuoteBasis::CleanPricePer100(p)) => {
                    append_u8(buf, 1);
                    append_canonical_decimal(buf, p);
                }
                Some(CanonicalQuoteBasis::YieldToMaturity(y)) => {
                    append_u8(buf, 2);
                    append_canonical_decimal(buf, y);
                }
                None => append_u8(buf, 0),
            }
            append_opt_canonical_decimal(buf, q.settlement_amount.as_ref());
        }
        None => append_u8(buf, 0),
    }
}

const fn invalid() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}
