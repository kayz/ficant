use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use ficant_application::use_cases::blotter::{
    AccountChange, AccountDeskPolicy, AnalysisUnits, BlotterEventEnvelope, CalendarResolutionMode,
    CoverageStatus, DraftEarmarkRecord, DraftEarmarkStatus, DraftTargetState,
    EnrichedAccountBlotterSnapshot, FullOpeningPositionBase, InstrumentCatalogEntry,
    MarketQuoteTick, RateCoverageEligibility, RiskAndCoverageSummary, RuleLineage,
    SourceTextAnchor, StoredMarketQuoteTick, UncoveredReason, ValuedInstrumentPositionView,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory, ports::ApplicationResult};
use ficant_domain::analytics::{BondTaxBasis, CalendarBinding, CalendarRequirement};
use ficant_domain::blotter::{
    AllocationPool, AllocationSegment, BaseEvidence, BaselineDelta, BlotterConfig,
    BucketBalanceView, EarmarkRecord, EarmarkState, ExactDirtyPriceCashInput, FeedHealthState,
    FeedStatusView, FundingGapSummary, FundingPrecision, InstrumentPositionView,
    OpeningInstrumentBalance, OpeningPositionBase, PendingMatchReason, PendingMatchRecord,
    PendingMatchState, QuoteBasis, QuoteTerms, ReconcileAllocationRecord, SeqRange,
    SettlementBucket, SourceBatchRecord, TradeFeedRecordAction, TradeFillRecord,
    TradeRevisionLifecycle, TradeSide,
};
use ficant_domain::market::{
    Bond, BondBusinessDayConvention, BondCouponFrequency, BondDayCountConvention, BondPricingTerms,
    BondTaxAttributes, IncomeTaxStatus, Instrument, InstrumentInput, InstrumentKind,
    MarketRulePack, MarketRulePackInput, PriceSourceType, RulePackContent, ValueAddedTaxStatus,
    VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, DECIMAL_SCALE, DecimalValue, EffectivePeriod, FixedDecimal, MarketTime, OwnerRef,
    Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::{ContentAddressed, VersionedDefinition};
use serde::{Deserialize, Serialize};

const FIXED_SCALE_FACTOR: u128 = 1_000_000_000_000;

/// Formats a `FixedDecimal` as canonical scaled decimal TEXT (`12` fractional digits, no floats).
#[must_use]
pub fn format_fixed_decimal(value: FixedDecimal) -> String {
    let raw = value.scaled();
    let negative = raw < 0;
    let mag = raw.unsigned_abs();
    let integer = mag / FIXED_SCALE_FACTOR;
    let fractional = mag % FIXED_SCALE_FACTOR;
    if negative {
        format!("-{integer}.{fractional:012}")
    } else {
        format!("{integer}.{fractional:012}")
    }
}

/// Parses canonical scaled decimal TEXT (or up to 12 fractional digits) into `FixedDecimal`
/// without floating-point arithmetic.
pub fn parse_fixed_decimal(text: &str) -> ApplicationResult<FixedDecimal> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed != text {
        return Err(validation_failed());
    }
    let (negative, rest) = if let Some(stripped) = trimmed.strip_prefix('-') {
        (true, stripped)
    } else if let Some(stripped) = trimmed.strip_prefix('+') {
        (false, stripped)
    } else {
        (false, trimmed)
    };
    if rest.is_empty() {
        return Err(validation_failed());
    }

    let (int_part, frac_part) = match rest.split_once('.') {
        Some((i, f)) => {
            if f.contains('.') || i.is_empty() || f.is_empty() {
                return Err(validation_failed());
            }
            (i, f)
        }
        None => (rest, ""),
    };

    if !int_part.bytes().all(|b| b.is_ascii_digit())
        || !frac_part.bytes().all(|b| b.is_ascii_digit())
        || frac_part.len() > DECIMAL_SCALE as usize
    {
        return Err(validation_failed());
    }

    let integer = int_part.parse::<u128>().map_err(|_| validation_failed())?;
    let mut frac_scaled = 0_u128;
    for byte in frac_part.bytes() {
        frac_scaled = frac_scaled
            .checked_mul(10)
            .and_then(|v| v.checked_add(u128::from(byte - b'0')))
            .ok_or_else(validation_failed)?;
    }
    for _ in frac_part.len()..(DECIMAL_SCALE as usize) {
        frac_scaled = frac_scaled.checked_mul(10).ok_or_else(validation_failed)?;
    }

    let mag = integer
        .checked_mul(FIXED_SCALE_FACTOR)
        .and_then(|v| v.checked_add(frac_scaled))
        .ok_or_else(validation_failed)?;

    if !negative {
        let scaled = i128::try_from(mag).map_err(|_| validation_failed())?;
        return Ok(FixedDecimal::from_scaled(scaled));
    }
    if mag == 0 {
        return Ok(FixedDecimal::ZERO);
    }
    if mag == i128::MIN.unsigned_abs() {
        return Ok(FixedDecimal::from_scaled(i128::MIN));
    }
    let scaled = i128::try_from(mag)
        .ok()
        .and_then(i128::checked_neg)
        .ok_or_else(validation_failed)?;
    Ok(FixedDecimal::from_scaled(scaled))
}

/// Converts a `u64` sequence number into `SQLite`'s non-negative signed 64-bit `INTEGER`.
pub fn u64_to_sqlite_i64(value: u64) -> ApplicationResult<i64> {
    i64::try_from(value).map_err(|_| validation_failed())
}

/// Converts a non-negative `SQLite` `INTEGER` back to `u64`.
pub fn sqlite_i64_to_u64(value: i64) -> ApplicationResult<u64> {
    u64::try_from(value).map_err(|_| storage_unavailable())
}

pub fn content_hash_from_blob(bytes: &[u8]) -> ApplicationResult<ContentHash> {
    ContentHash::from_bytes(bytes).map_err(|_| storage_unavailable())
}

#[must_use]
pub fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[usize::from(b >> 4)] as char);
        out.push(HEX[usize::from(b & 0x0f)] as char);
    }
    out
}

pub fn decode_hex(text: &str) -> ApplicationResult<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return Err(storage_unavailable());
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let hi = hex_nibble(pair[0])?;
        let lo = hex_nibble(pair[1])?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

fn hex_nibble(b: u8) -> ApplicationResult<u8> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(storage_unavailable()),
    }
}

pub fn serialize_json<T: Serialize>(value: &T) -> ApplicationResult<String> {
    serde_json::to_string(value).map_err(|_| storage_unavailable())
}

pub fn deserialize_json<T: for<'de> Deserialize<'de>>(text: &str) -> ApplicationResult<T> {
    serde_json::from_str(text).map_err(|_| storage_unavailable())
}

// ============================================================================
// Enum string tags for normalized SQLite columns
// ============================================================================

#[must_use]
pub const fn side_to_str(side: TradeSide) -> &'static str {
    match side {
        TradeSide::Buy => "BUY",
        TradeSide::Sell => "SELL",
    }
}

#[must_use]
pub const fn bucket_to_str(bucket: SettlementBucket) -> &'static str {
    match bucket {
        SettlementBucket::TPlus0 => "T_PLUS_0",
        SettlementBucket::TPlus1 => "T_PLUS_1",
    }
}

#[must_use]
pub const fn earmark_state_to_str(state: EarmarkState) -> &'static str {
    match state {
        EarmarkState::Hold => "HOLD",
        EarmarkState::Committed => "COMMITTED",
        EarmarkState::Expired => "EXPIRED",
        EarmarkState::Released => "RELEASED",
        EarmarkState::Cancelled => "CANCELLED",
        EarmarkState::Reconciled => "RECONCILED",
    }
}

#[must_use]
pub const fn funding_precision_to_str(precision: FundingPrecision) -> &'static str {
    match precision {
        FundingPrecision::ExactSettlementAmount => "EXACT_SETTLEMENT_AMOUNT",
        FundingPrecision::ExactDirtyPrice => "EXACT_DIRTY_PRICE",
        FundingPrecision::CleanPriceEstimate => "CLEAN_PRICE_ESTIMATE",
        FundingPrecision::UnpricedYieldOnly => "UNPRICED_YIELD_ONLY",
    }
}

#[must_use]
pub const fn draft_status_to_str(status: DraftEarmarkStatus) -> &'static str {
    match status {
        DraftEarmarkStatus::PendingConfirmation => "PENDING_CONFIRMATION",
        DraftEarmarkStatus::Confirmed => "CONFIRMED",
        DraftEarmarkStatus::Discarded => "DISCARDED",
    }
}

#[must_use]
pub const fn pending_state_to_str(state: PendingMatchState) -> &'static str {
    match state {
        PendingMatchState::Open => "OPEN",
        PendingMatchState::ResolvedMatched => "RESOLVED_MATCHED",
        PendingMatchState::ResolvedOrphanAccepted => "RESOLVED_ORPHAN_ACCEPTED",
        PendingMatchState::Released => "RELEASED",
        PendingMatchState::ReversedByFeed => "REVERSED_BY_FEED",
        PendingMatchState::ResolvedBaselineDelta => "RESOLVED_BASELINE_DELTA",
    }
}

#[must_use]
pub const fn pending_reason_to_str(reason: PendingMatchReason) -> &'static str {
    match reason {
        PendingMatchReason::AmbiguousCandidates => "AMBIGUOUS_CANDIDATES",
        PendingMatchReason::NotionalMismatchOrSplit => "NOTIONAL_MISMATCH_OR_SPLIT",
        PendingMatchReason::PriceOutOfTolerance => "PRICE_OUT_OF_TOLERANCE",
        PendingMatchReason::OrphanNoCommitment => "ORPHAN_NO_COMMITMENT",
        PendingMatchReason::BurstRepeat => "BURST_REPEAT",
        PendingMatchReason::ExistingPendingOnKey => "EXISTING_PENDING_ON_KEY",
        PendingMatchReason::AmendmentMismatch => "AMENDMENT_MISMATCH",
        PendingMatchReason::BaselineRevisionManual => "BASELINE_REVISION_MANUAL",
        PendingMatchReason::IncomparableBasis => "INCOMPARABLE_BASIS",
        PendingMatchReason::RevisionHold => "REVISION_HOLD",
        PendingMatchReason::GapHold => "GAP_HOLD",
    }
}

#[must_use]
pub const fn trade_lifecycle_to_str(lifecycle: TradeRevisionLifecycle) -> &'static str {
    match lifecycle {
        TradeRevisionLifecycle::BufferedGap => "BUFFERED_GAP",
        TradeRevisionLifecycle::RevisionHold => "REVISION_HOLD",
        TradeRevisionLifecycle::PreWatermarkIgnored => "PRE_WATERMARK_IGNORED",
        TradeRevisionLifecycle::SupersededInBatch => "SUPERSEDED_IN_BATCH",
        TradeRevisionLifecycle::Applied => "APPLIED",
        TradeRevisionLifecycle::BaselineReview => "BASELINE_REVIEW",
        TradeRevisionLifecycle::Reversed => "REVERSED",
    }
}

pub fn trade_lifecycle_from_str(text: &str) -> ApplicationResult<TradeRevisionLifecycle> {
    match text {
        "BUFFERED_GAP" => Ok(TradeRevisionLifecycle::BufferedGap),
        "REVISION_HOLD" => Ok(TradeRevisionLifecycle::RevisionHold),
        "PRE_WATERMARK_IGNORED" => Ok(TradeRevisionLifecycle::PreWatermarkIgnored),
        "SUPERSEDED_IN_BATCH" => Ok(TradeRevisionLifecycle::SupersededInBatch),
        "APPLIED" => Ok(TradeRevisionLifecycle::Applied),
        "BASELINE_REVIEW" => Ok(TradeRevisionLifecycle::BaselineReview),
        "REVERSED" => Ok(TradeRevisionLifecycle::Reversed),
        _ => Err(storage_unavailable()),
    }
}

// ============================================================================
// Core primitive DTOs
// ============================================================================

#[derive(Serialize, Deserialize)]
pub struct BlotterConfigDto {
    pub clean_price_tolerance: String,
    pub yield_tolerance: String,
    pub committed_overdue_seconds: u32,
    pub pre_close_alert_time: String,
    pub feed_staleness_seconds: Option<u32>,
}

pub fn config_to_json(config: &BlotterConfig) -> ApplicationResult<String> {
    let dto = BlotterConfigDto {
        clean_price_tolerance: format_fixed_decimal(config.clean_price_tolerance),
        yield_tolerance: format_fixed_decimal(config.yield_tolerance),
        committed_overdue_seconds: config.committed_overdue_seconds,
        pre_close_alert_time: config
            .pre_close_alert_time
            .format("%H:%M:%S%.f")
            .to_string(),
        feed_staleness_seconds: config.feed_staleness_seconds,
    };
    serialize_json(&dto)
}

pub fn config_from_json(text: &str) -> ApplicationResult<BlotterConfig> {
    let dto: BlotterConfigDto = deserialize_json(text)?;
    let pre_close_alert_time = NaiveTime::parse_from_str(&dto.pre_close_alert_time, "%H:%M:%S%.f")
        .map_err(|_| storage_unavailable())?;
    let config = BlotterConfig {
        clean_price_tolerance: parse_fixed_decimal(&dto.clean_price_tolerance)
            .map_err(|_| storage_unavailable())?,
        yield_tolerance: parse_fixed_decimal(&dto.yield_tolerance)
            .map_err(|_| storage_unavailable())?,
        committed_overdue_seconds: dto.committed_overdue_seconds,
        pre_close_alert_time,
        feed_staleness_seconds: dto.feed_staleness_seconds,
    };
    config.validate().map_err(|_| storage_unavailable())?;
    Ok(config)
}

#[derive(Serialize, Deserialize)]
pub struct MarketTimeDto {
    pub instant_secs: i64,
    pub instant_nanos: u32,
    pub market_timezone: String,
    pub local_trading_date: String,
}

impl MarketTimeDto {
    #[must_use]
    pub fn from_domain(value: &MarketTime) -> Self {
        let dt = value.instant();
        Self {
            instant_secs: dt.timestamp(),
            instant_nanos: dt.timestamp_subsec_nanos(),
            market_timezone: value.market_timezone().to_owned(),
            local_trading_date: value.local_trading_date().to_string(),
        }
    }

    pub fn into_domain(self) -> ApplicationResult<MarketTime> {
        let dt = DateTime::<Utc>::from_timestamp(self.instant_secs, self.instant_nanos)
            .ok_or_else(storage_unavailable)?;
        let date = parse_naive_date(&self.local_trading_date)?;
        MarketTime::new(dt, self.market_timezone, date).map_err(|_| storage_unavailable())
    }
}

#[derive(Serialize, Deserialize)]
pub struct UnitRefDto {
    pub unit_id: String,
    pub version: u64,
}

impl UnitRefDto {
    #[must_use]
    pub fn from_domain(value: &UnitRef) -> Self {
        Self {
            unit_id: value.unit_id().as_str().to_owned(),
            version: value.version().get(),
        }
    }

    pub fn into_domain(self) -> ApplicationResult<UnitRef> {
        Ok(UnitRef::new(
            parse_ulid(&self.unit_id)?,
            parse_version(self.version)?,
        ))
    }
}

#[derive(Serialize, Deserialize)]
pub struct VersionRefDto {
    pub id: String,
    pub version: u64,
}

impl VersionRefDto {
    #[must_use]
    pub fn from_domain(value: &VersionRef) -> Self {
        Self {
            id: value.id().as_str().to_owned(),
            version: value.version().get(),
        }
    }

    pub fn into_domain(self) -> ApplicationResult<VersionRef> {
        Ok(VersionRef::new(
            parse_ulid(&self.id)?,
            parse_version(self.version)?,
        ))
    }
}

#[derive(Serialize, Deserialize)]
pub struct OwnerRefDto {
    pub tenant_id: String,
    pub owner_id: String,
}

impl OwnerRefDto {
    #[must_use]
    pub fn from_domain(value: &OwnerRef) -> Self {
        Self {
            tenant_id: value.tenant_id().as_str().to_owned(),
            owner_id: value.owner_id().as_str().to_owned(),
        }
    }

    pub fn into_domain(self) -> ApplicationResult<OwnerRef> {
        Ok(OwnerRef::new(
            parse_ulid(&self.tenant_id)?,
            parse_ulid(&self.owner_id)?,
        ))
    }
}

#[derive(Serialize, Deserialize)]
pub struct DecimalValueDto {
    pub coefficient: String,
    pub scale: u32,
    pub unit: UnitRefDto,
}

impl DecimalValueDto {
    #[must_use]
    pub fn from_domain(value: &DecimalValue) -> Self {
        Self {
            coefficient: value.coefficient().to_owned(),
            scale: value.scale(),
            unit: UnitRefDto::from_domain(value.unit()),
        }
    }

    pub fn into_domain(self) -> ApplicationResult<DecimalValue> {
        DecimalValue::new(self.coefficient, self.scale, self.unit.into_domain()?)
            .map_err(|_| storage_unavailable())
    }
}

#[derive(Serialize, Deserialize)]
pub struct AnalysisUnitsDto {
    pub currency_amount: UnitRefDto,
    pub price_per_100: UnitRefDto,
    pub rate: UnitRefDto,
    pub dv01: UnitRefDto,
    pub dv01_per_100: UnitRefDto,
    pub dimensionless: UnitRefDto,
    pub contract_count: UnitRefDto,
}

impl AnalysisUnitsDto {
    #[must_use]
    pub fn from_app(value: &AnalysisUnits) -> Self {
        Self {
            currency_amount: UnitRefDto::from_domain(&value.currency_amount),
            price_per_100: UnitRefDto::from_domain(&value.price_per_100),
            rate: UnitRefDto::from_domain(&value.rate),
            dv01: UnitRefDto::from_domain(&value.dv01),
            dv01_per_100: UnitRefDto::from_domain(&value.dv01_per_100),
            dimensionless: UnitRefDto::from_domain(&value.dimensionless),
            contract_count: UnitRefDto::from_domain(&value.contract_count),
        }
    }

    pub fn into_app(self) -> ApplicationResult<AnalysisUnits> {
        Ok(AnalysisUnits {
            currency_amount: self.currency_amount.into_domain()?,
            price_per_100: self.price_per_100.into_domain()?,
            rate: self.rate.into_domain()?,
            dv01: self.dv01.into_domain()?,
            dv01_per_100: self.dv01_per_100.into_domain()?,
            dimensionless: self.dimensionless.into_domain()?,
            contract_count: self.contract_count.into_domain()?,
        })
    }
}

// ============================================================================
// Market objects: Bond, CalendarBinding, MarketRulePack
// ============================================================================

#[derive(Serialize, Deserialize)]
pub struct BondTaxAttributesDto {
    pub vat: String,
    pub income_tax: String,
}

#[derive(Serialize, Deserialize)]
pub struct BondPricingTermsDto {
    pub coupon_rate: DecimalValueDto,
    pub frequency: String,
    pub day_count: String,
    pub business_day: String,
}

#[derive(Serialize, Deserialize)]
pub struct BondDto {
    pub instrument: VersionRefDto,
    pub first_issue_date: String,
    pub current_issue_date: String,
    pub maturity_date: String,
    pub cumulative_issued_amount: DecimalValueDto,
    pub tax_attributes: Option<BondTaxAttributesDto>,
    pub face_value: DecimalValueDto,
    pub pricing_terms: Option<BondPricingTermsDto>,
}

impl BondDto {
    #[must_use]
    pub fn from_domain(bond: &Bond) -> Self {
        Self {
            instrument: VersionRefDto::from_domain(bond.instrument()),
            first_issue_date: bond.first_issue_date().to_string(),
            current_issue_date: bond.current_issue_date().to_string(),
            maturity_date: bond.maturity_date().to_string(),
            cumulative_issued_amount: DecimalValueDto::from_domain(bond.cumulative_issued_amount()),
            tax_attributes: bond.tax_attributes().map(|ta| BondTaxAttributesDto {
                vat: match ta.value_added_tax_status() {
                    ValueAddedTaxStatus::Exempt => "EXEMPT".to_owned(),
                    ValueAddedTaxStatus::Taxable => "TAXABLE".to_owned(),
                },
                income_tax: match ta.income_tax_status() {
                    IncomeTaxStatus::Exempt => "EXEMPT".to_owned(),
                    IncomeTaxStatus::Taxable => "TAXABLE".to_owned(),
                },
            }),
            face_value: DecimalValueDto::from_domain(bond.face_value()),
            pricing_terms: bond.pricing_terms().map(|pt| BondPricingTermsDto {
                coupon_rate: DecimalValueDto::from_domain(pt.coupon_rate()),
                frequency: match pt.frequency() {
                    BondCouponFrequency::Annual => "ANNUAL".to_owned(),
                    BondCouponFrequency::Semiannual => "SEMIANNUAL".to_owned(),
                },
                day_count: match pt.day_count() {
                    BondDayCountConvention::ActActBondIsma => "ACT_ACT_BOND_ISMA".to_owned(),
                },
                business_day: match pt.business_day() {
                    BondBusinessDayConvention::Following => "FOLLOWING".to_owned(),
                },
            }),
        }
    }

    pub fn into_domain(self) -> ApplicationResult<Bond> {
        let inst_ref = self.instrument.into_domain()?;
        let face_value = self.face_value.into_domain()?;
        let cumulative_issued_amount = self.cumulative_issued_amount.into_domain()?;
        let first_issue_date = parse_naive_date(&self.first_issue_date)?;
        let current_issue_date = parse_naive_date(&self.current_issue_date)?;
        let maturity_date = parse_naive_date(&self.maturity_date)?;

        // Reconstruct ephemeral Instrument shell carrying exact VersionRef for Bond constructor.
        let placeholder_owner = OwnerRef::new(inst_ref.id().clone(), inst_ref.id().clone());
        let instrument = Instrument::new(InstrumentInput {
            instrument_id: inst_ref.id().clone(),
            version: inst_ref.version(),
            owner: placeholder_owner,
            kind: InstrumentKind::Bond,
            market: "GENERIC".to_owned(),
            symbol: "BOND".to_owned(),
            currency: face_value.unit().clone(),
            calendar: inst_ref,
        })
        .map_err(|_| storage_unavailable())?;

        let mut bond = if let Some(ta) = self.tax_attributes {
            let vat = match ta.vat.as_str() {
                "EXEMPT" => ValueAddedTaxStatus::Exempt,
                "TAXABLE" => ValueAddedTaxStatus::Taxable,
                _ => return Err(storage_unavailable()),
            };
            let income = match ta.income_tax.as_str() {
                "EXEMPT" => IncomeTaxStatus::Exempt,
                "TAXABLE" => IncomeTaxStatus::Taxable,
                _ => return Err(storage_unavailable()),
            };
            Bond::with_issuance(
                &instrument,
                first_issue_date,
                current_issue_date,
                maturity_date,
                cumulative_issued_amount,
                BondTaxAttributes::new(vat, income),
                face_value,
            )
            .map_err(|_| storage_unavailable())?
        } else {
            Bond::new(&instrument, first_issue_date, maturity_date, face_value)
                .map_err(|_| storage_unavailable())?
        };

        if let Some(pt) = self.pricing_terms {
            let frequency = match pt.frequency.as_str() {
                "ANNUAL" => BondCouponFrequency::Annual,
                "SEMIANNUAL" => BondCouponFrequency::Semiannual,
                _ => return Err(storage_unavailable()),
            };
            let day_count = match pt.day_count.as_str() {
                "ACT_ACT_BOND_ISMA" => BondDayCountConvention::ActActBondIsma,
                _ => return Err(storage_unavailable()),
            };
            let business_day = match pt.business_day.as_str() {
                "FOLLOWING" => BondBusinessDayConvention::Following,
                _ => return Err(storage_unavailable()),
            };
            let terms = BondPricingTerms::new(
                pt.coupon_rate.into_domain()?,
                frequency,
                day_count,
                business_day,
            )
            .map_err(|_| storage_unavailable())?;
            bond = bond
                .with_pricing_terms(terms)
                .map_err(|_| storage_unavailable())?;
        }

        Ok(bond)
    }
}

#[derive(Serialize, Deserialize)]
pub struct CalendarBindingDto {
    pub calendar_id: String,
    pub version: u64,
    pub content_hash_hex: String,
    pub coverage_start: String,
    pub coverage_end: String,
    pub non_business_days: Vec<String>,
    pub work_weekends: Vec<String>,
}

impl CalendarBindingDto {
    #[must_use]
    pub fn from_domain(cal: &CalendarBinding) -> Self {
        Self {
            calendar_id: cal.id().to_owned(),
            version: cal.version().get(),
            content_hash_hex: encode_hex(cal.content_hash().as_bytes()),
            coverage_start: cal.coverage_start().to_string(),
            coverage_end: cal.coverage_end().to_string(),
            non_business_days: cal
                .non_business_days()
                .iter()
                .map(ToString::to_string)
                .collect(),
            work_weekends: cal
                .work_weekends()
                .iter()
                .map(ToString::to_string)
                .collect(),
        }
    }

    pub fn into_domain(self) -> ApplicationResult<CalendarBinding> {
        let non_business_days = self
            .non_business_days
            .iter()
            .map(|d| parse_naive_date(d))
            .collect::<ApplicationResult<Vec<_>>>()?;
        let work_weekends = self
            .work_weekends
            .iter()
            .map(|d| parse_naive_date(d))
            .collect::<ApplicationResult<Vec<_>>>()?;
        CalendarBinding::new(
            self.calendar_id,
            parse_version(self.version)?,
            parse_content_hash_hex(&self.content_hash_hex)?,
            parse_naive_date(&self.coverage_start)?,
            parse_naive_date(&self.coverage_end)?,
            non_business_days,
            work_weekends,
        )
        .map_err(|_| storage_unavailable())
    }
}

#[derive(Serialize, Deserialize)]
pub struct MarketRulePackDto {
    pub rule_pack_id: String,
    pub version: u64,
    pub owner: OwnerRefDto,
    pub market: String,
    pub rule_type: String,
    pub source: String,
    pub effective_from: MarketTimeDto,
    pub effective_to: MarketTimeDto,
    pub verification_status: String,
    pub content_hash_hex: String,
    pub content_type_url: Option<String>,
    pub content_value_hex: Option<String>,
}

impl MarketRulePackDto {
    #[must_use]
    pub fn from_domain(pack: &MarketRulePack) -> Self {
        let (content_type_url, content_value_hex) = match pack.content() {
            Some(c) => (Some(c.type_url().to_owned()), Some(encode_hex(c.value()))),
            None => (None, None),
        };
        Self {
            rule_pack_id: pack.identity().to_owned(),
            version: pack.version(),
            owner: OwnerRefDto::from_domain(pack.owner()),
            market: pack.market().to_owned(),
            rule_type: pack.rule_type().to_owned(),
            source: pack.source().to_owned(),
            effective_from: MarketTimeDto::from_domain(pack.effective().from()),
            effective_to: MarketTimeDto::from_domain(pack.effective().to()),
            verification_status: match pack.verification_status() {
                VerificationStatus::Unverified => "UNVERIFIED".to_owned(),
                VerificationStatus::Verified => "VERIFIED".to_owned(),
                VerificationStatus::Rejected => "REJECTED".to_owned(),
            },
            content_hash_hex: encode_hex(pack.content_hash().as_bytes()),
            content_type_url,
            content_value_hex,
        }
    }

    pub fn into_domain(self) -> ApplicationResult<MarketRulePack> {
        let verification_status = match self.verification_status.as_str() {
            "UNVERIFIED" => VerificationStatus::Unverified,
            "VERIFIED" => VerificationStatus::Verified,
            "REJECTED" => VerificationStatus::Rejected,
            _ => return Err(storage_unavailable()),
        };
        let effective = EffectivePeriod::new(
            self.effective_from.into_domain()?,
            self.effective_to.into_domain()?,
        )
        .map_err(|_| storage_unavailable())?;
        let input = MarketRulePackInput {
            rule_pack_id: parse_ulid(&self.rule_pack_id)?,
            version: parse_version(self.version)?,
            owner: self.owner.into_domain()?,
            market: self.market,
            rule_type: self.rule_type,
            source: self.source,
            effective,
            verification_status,
            content_hash: parse_content_hash_hex(&self.content_hash_hex)?,
        };
        match (self.content_type_url, self.content_value_hex) {
            (Some(type_url), Some(val_hex)) => {
                let value = decode_hex(&val_hex)?;
                let content =
                    RulePackContent::new(type_url, value).map_err(|_| storage_unavailable())?;
                MarketRulePack::new_with_content(input, content).map_err(|_| storage_unavailable())
            }
            (None, None) => MarketRulePack::new(input).map_err(|_| storage_unavailable()),
            _ => Err(storage_unavailable()),
        }
    }
}

// ============================================================================
// Blotter domain & application DTOs
// ============================================================================

#[derive(Serialize, Deserialize)]
pub struct QuoteTermsDto {
    pub basis_kind: Option<String>,
    pub basis_value: Option<String>,
    pub settlement_amount: Option<String>,
}

impl QuoteTermsDto {
    #[must_use]
    pub fn from_domain(value: &QuoteTerms) -> Self {
        let (basis_kind, basis_value) = match value.basis {
            Some(QuoteBasis::CleanPricePer100(p)) => {
                (Some("CLEAN".to_owned()), Some(format_fixed_decimal(p)))
            }
            Some(QuoteBasis::YieldToMaturity(y)) => {
                (Some("YTM".to_owned()), Some(format_fixed_decimal(y)))
            }
            None => (None, None),
        };
        Self {
            basis_kind,
            basis_value,
            settlement_amount: value.settlement_amount.map(format_fixed_decimal),
        }
    }

    pub fn into_domain(self) -> ApplicationResult<QuoteTerms> {
        let basis = match (self.basis_kind.as_deref(), self.basis_value.as_deref()) {
            (Some("CLEAN"), Some(v)) => Some(QuoteBasis::CleanPricePer100(
                parse_fixed_decimal_storage(v)?,
            )),
            (Some("YTM"), Some(v)) => {
                Some(QuoteBasis::YieldToMaturity(parse_fixed_decimal_storage(v)?))
            }
            (None, None) => None,
            _ => return Err(storage_unavailable()),
        };
        let settlement_amount = self
            .settlement_amount
            .as_deref()
            .map(parse_fixed_decimal_storage)
            .transpose()?;
        Ok(QuoteTerms {
            basis,
            settlement_amount,
        })
    }
}

#[derive(Serialize, Deserialize)]
pub struct ExactDirtyPriceCashInputDto {
    pub dirty_price: String,
    pub face_value: String,
}

impl ExactDirtyPriceCashInputDto {
    #[must_use]
    pub fn from_domain(v: ExactDirtyPriceCashInput) -> Self {
        Self {
            dirty_price: format_fixed_decimal(v.dirty_price),
            face_value: format_fixed_decimal(v.face_value),
        }
    }

    pub fn into_domain(self) -> ApplicationResult<ExactDirtyPriceCashInput> {
        Ok(ExactDirtyPriceCashInput {
            dirty_price: parse_fixed_decimal_storage(&self.dirty_price)?,
            face_value: parse_fixed_decimal_storage(&self.face_value)?,
        })
    }
}

#[derive(Serialize, Deserialize)]
pub struct AllocationSegmentDto {
    pub segment_index: u32,
    pub base_notional: String,
    pub base_signed_cash: String,
    pub cumulative_allocated_notional: String,
    pub cumulative_allocated_cash_abs: String,
}

impl AllocationSegmentDto {
    #[must_use]
    pub fn from_domain(seg: &AllocationSegment) -> Self {
        Self {
            segment_index: seg.segment_index,
            base_notional: format_fixed_decimal(seg.base_notional),
            base_signed_cash: format_fixed_decimal(seg.base_signed_cash),
            cumulative_allocated_notional: format_fixed_decimal(seg.cumulative_allocated_notional),
            cumulative_allocated_cash_abs: seg.cumulative_allocated_cash_abs.to_string(),
        }
    }

    pub fn into_domain(self) -> ApplicationResult<AllocationSegment> {
        Ok(AllocationSegment {
            segment_index: self.segment_index,
            base_notional: parse_fixed_decimal_storage(&self.base_notional)?,
            base_signed_cash: parse_fixed_decimal_storage(&self.base_signed_cash)?,
            cumulative_allocated_notional: parse_fixed_decimal_storage(
                &self.cumulative_allocated_notional,
            )?,
            cumulative_allocated_cash_abs: self
                .cumulative_allocated_cash_abs
                .parse::<u128>()
                .map_err(|_| storage_unavailable())?,
        })
    }
}

#[derive(Serialize, Deserialize)]
pub struct AllocationPoolDto {
    pub active_segment: Option<AllocationSegmentDto>,
    pub segment_history: Vec<AllocationSegmentDto>,
}

impl AllocationPoolDto {
    #[must_use]
    pub fn from_domain(pool: &AllocationPool) -> Self {
        Self {
            active_segment: pool
                .active_segment
                .as_ref()
                .map(AllocationSegmentDto::from_domain),
            segment_history: pool
                .segment_history
                .iter()
                .map(AllocationSegmentDto::from_domain)
                .collect(),
        }
    }

    pub fn into_domain(self) -> ApplicationResult<AllocationPool> {
        Ok(AllocationPool {
            active_segment: self
                .active_segment
                .map(AllocationSegmentDto::into_domain)
                .transpose()?,
            segment_history: self
                .segment_history
                .into_iter()
                .map(AllocationSegmentDto::into_domain)
                .collect::<ApplicationResult<Vec<_>>>()?,
        })
    }
}

#[derive(Serialize, Deserialize)]
pub struct OpeningInstrumentBalanceDto {
    pub instrument_code: String,
    pub opening_economic_notional: String,
    pub opening_available_notional: String,
    pub opening_net_t0_notional: String,
    pub opening_net_t1_notional: String,
}

impl OpeningInstrumentBalanceDto {
    #[must_use]
    pub fn from_domain(b: &OpeningInstrumentBalance) -> Self {
        Self {
            instrument_code: b.instrument_code.clone(),
            opening_economic_notional: format_fixed_decimal(b.opening_economic_notional),
            opening_available_notional: format_fixed_decimal(b.opening_available_notional),
            opening_net_t0_notional: format_fixed_decimal(b.opening_net_t0_notional),
            opening_net_t1_notional: format_fixed_decimal(b.opening_net_t1_notional),
        }
    }

    pub fn into_domain(self) -> ApplicationResult<OpeningInstrumentBalance> {
        Ok(OpeningInstrumentBalance {
            instrument_code: self.instrument_code,
            opening_economic_notional: parse_fixed_decimal_storage(
                &self.opening_economic_notional,
            )?,
            opening_available_notional: parse_fixed_decimal_storage(
                &self.opening_available_notional,
            )?,
            opening_net_t0_notional: parse_fixed_decimal_storage(&self.opening_net_t0_notional)?,
            opening_net_t1_notional: parse_fixed_decimal_storage(&self.opening_net_t1_notional)?,
        })
    }
}

#[derive(Serialize, Deserialize)]
pub struct BaseEvidenceDto {
    pub source_id: String,
    pub external_trade_id: String,
    pub original_feed_seq: u64,
    pub original_executed_at: MarketTimeDto,
    pub old_notional: String,
    pub old_signed_cash: String,
    pub settlement_bucket: String,
    pub evidence_ref: String,
    pub evidence_hash_hex: String,
    pub instrument_code: String,
    pub side: String,
    pub original_revision: u64,
}

impl BaseEvidenceDto {
    #[must_use]
    pub fn from_domain(e: &BaseEvidence) -> Self {
        Self {
            source_id: e.source_id.clone(),
            external_trade_id: e.external_trade_id.clone(),
            original_feed_seq: e.original_feed_seq,
            original_executed_at: MarketTimeDto::from_domain(&e.original_executed_at),
            old_notional: format_fixed_decimal(e.old_notional),
            old_signed_cash: format_fixed_decimal(e.old_signed_cash),
            settlement_bucket: bucket_to_str(e.settlement_bucket).to_owned(),
            evidence_ref: e.evidence_ref.clone(),
            evidence_hash_hex: encode_hex(e.evidence_hash.as_bytes()),
            instrument_code: e.instrument_code.clone(),
            side: side_to_str(e.side).to_owned(),
            original_revision: e.original_revision,
        }
    }

    pub fn into_domain(self) -> ApplicationResult<BaseEvidence> {
        Ok(BaseEvidence {
            source_id: self.source_id,
            external_trade_id: self.external_trade_id,
            original_feed_seq: self.original_feed_seq,
            original_executed_at: self.original_executed_at.into_domain()?,
            old_notional: parse_fixed_decimal_storage(&self.old_notional)?,
            old_signed_cash: parse_fixed_decimal_storage(&self.old_signed_cash)?,
            settlement_bucket: parse_bucket(&self.settlement_bucket)?,
            evidence_ref: self.evidence_ref,
            evidence_hash: parse_content_hash_hex(&self.evidence_hash_hex)?,
            instrument_code: self.instrument_code,
            side: parse_side(&self.side)?,
            original_revision: self.original_revision,
        })
    }
}

#[derive(Serialize, Deserialize)]
pub struct BaselineDeltaDto {
    pub delta_notional: String,
    pub delta_cash: String,
    pub settlement_bucket: String,
    pub evidence: BaseEvidenceDto,
    pub reason: String,
}

impl BaselineDeltaDto {
    #[must_use]
    pub fn from_domain(d: &BaselineDelta) -> Self {
        Self {
            delta_notional: format_fixed_decimal(d.delta_notional),
            delta_cash: format_fixed_decimal(d.delta_cash),
            settlement_bucket: bucket_to_str(d.settlement_bucket).to_owned(),
            evidence: BaseEvidenceDto::from_domain(&d.evidence),
            reason: d.reason.clone(),
        }
    }

    pub fn into_domain(self) -> ApplicationResult<BaselineDelta> {
        Ok(BaselineDelta {
            delta_notional: parse_fixed_decimal_storage(&self.delta_notional)?,
            delta_cash: parse_fixed_decimal_storage(&self.delta_cash)?,
            settlement_bucket: parse_bucket(&self.settlement_bucket)?,
            evidence: self.evidence.into_domain()?,
            reason: self.reason,
        })
    }
}

#[derive(Serialize, Deserialize)]
pub struct OpeningBaseFullImageDto {
    pub account_id: String,
    pub base_id: String,
    pub trading_date: String,
    pub next_settlement_date: String,
    pub effective_from: MarketTimeDto,
    pub watermark_time: MarketTimeDto,
    pub watermark_source_seq: Option<u64>,
    pub source_id: String,
    pub opening_cash_t0: String,
    pub opening_net_cash_delta_t1: String,
    pub balances: Vec<OpeningInstrumentBalanceDto>,
    pub instrument_codes: Vec<String>,
    pub baseline_evidence: Vec<BaseEvidenceDto>,
    pub source_start_after_feed_seq: u64,
    pub calendar: Option<CalendarBindingDto>,
    pub default_calendar_requirement: Option<String>,
    pub units: AnalysisUnitsDto,
}

impl OpeningBaseFullImageDto {
    #[must_use]
    pub fn from_parts(
        core: &OpeningPositionBase,
        calendar: Option<&CalendarBinding>,
        default_calendar_requirement: Option<CalendarRequirement>,
        units: &AnalysisUnits,
    ) -> Self {
        Self {
            account_id: core.account_id.clone(),
            base_id: core.base_id.as_str().to_owned(),
            trading_date: core.trading_date.to_string(),
            next_settlement_date: core.next_settlement_date.to_string(),
            effective_from: MarketTimeDto::from_domain(&core.effective_from),
            watermark_time: MarketTimeDto::from_domain(&core.watermark_time),
            watermark_source_seq: core.watermark_source_seq,
            source_id: core.source_id.clone(),
            opening_cash_t0: format_fixed_decimal(core.opening_cash_t0),
            opening_net_cash_delta_t1: format_fixed_decimal(core.opening_net_cash_delta_t1),
            balances: core
                .balances
                .iter()
                .map(OpeningInstrumentBalanceDto::from_domain)
                .collect(),
            instrument_codes: core.instrument_codes.clone(),
            baseline_evidence: core
                .baseline_evidence
                .iter()
                .map(BaseEvidenceDto::from_domain)
                .collect(),
            source_start_after_feed_seq: core.source_start_after_feed_seq,
            calendar: calendar.map(CalendarBindingDto::from_domain),
            default_calendar_requirement: default_calendar_requirement
                .map(|r| calendar_req_to_str(r).to_owned()),
            units: AnalysisUnitsDto::from_app(units),
        }
    }

    pub fn into_parts(
        self,
    ) -> ApplicationResult<(
        OpeningPositionBase,
        Option<CalendarBinding>,
        Option<CalendarRequirement>,
        AnalysisUnits,
    )> {
        let core = OpeningPositionBase {
            account_id: self.account_id,
            base_id: parse_ulid(&self.base_id)?,
            trading_date: parse_naive_date(&self.trading_date)?,
            next_settlement_date: parse_naive_date(&self.next_settlement_date)?,
            effective_from: self.effective_from.into_domain()?,
            watermark_time: self.watermark_time.into_domain()?,
            watermark_source_seq: self.watermark_source_seq,
            source_id: self.source_id,
            opening_cash_t0: parse_fixed_decimal_storage(&self.opening_cash_t0)?,
            opening_net_cash_delta_t1: parse_fixed_decimal_storage(
                &self.opening_net_cash_delta_t1,
            )?,
            balances: self
                .balances
                .into_iter()
                .map(OpeningInstrumentBalanceDto::into_domain)
                .collect::<ApplicationResult<Vec<_>>>()?,
            instrument_codes: self.instrument_codes,
            baseline_evidence: self
                .baseline_evidence
                .into_iter()
                .map(BaseEvidenceDto::into_domain)
                .collect::<ApplicationResult<Vec<_>>>()?,
            source_start_after_feed_seq: self.source_start_after_feed_seq,
        };
        let calendar = self
            .calendar
            .map(CalendarBindingDto::into_domain)
            .transpose()?;
        let default_req = self
            .default_calendar_requirement
            .as_deref()
            .map(parse_calendar_req)
            .transpose()?;
        let units = self.units.into_app()?;
        Ok((core, calendar, default_req, units))
    }
}

#[derive(Serialize, Deserialize)]
pub struct AccountDeskPolicyDto {
    pub policy_version: u64,
    pub default_quote_source_id: String,
    pub instrument_quote_sources: BTreeMap<String, String>,
    pub quote_staleness_seconds: u32,
    pub default_draft_hold_ttl_seconds: u32,
    pub allowed_hedge_product_codes: Vec<String>,
    pub regulatory_rule_pack: Option<MarketRulePackDto>,
    pub limit_ceilings: BTreeMap<String, String>,
}

impl AccountDeskPolicyDto {
    #[must_use]
    pub fn from_app(p: &AccountDeskPolicy) -> Self {
        Self {
            policy_version: p.policy_version,
            default_quote_source_id: p.default_quote_source_id.clone(),
            instrument_quote_sources: p.instrument_quote_sources.clone(),
            quote_staleness_seconds: p.quote_staleness_seconds,
            default_draft_hold_ttl_seconds: p.default_draft_hold_ttl_seconds,
            allowed_hedge_product_codes: p.allowed_hedge_product_codes.clone(),
            regulatory_rule_pack: p
                .regulatory_rule_pack
                .as_ref()
                .map(MarketRulePackDto::from_domain),
            limit_ceilings: p
                .limit_ceilings
                .iter()
                .map(|(k, v)| (k.clone(), format_fixed_decimal(*v)))
                .collect(),
        }
    }

    pub fn into_app(self) -> ApplicationResult<AccountDeskPolicy> {
        let mut limit_ceilings = BTreeMap::new();
        for (k, v) in self.limit_ceilings {
            limit_ceilings.insert(k, parse_fixed_decimal_storage(&v)?);
        }
        Ok(AccountDeskPolicy {
            policy_version: self.policy_version,
            default_quote_source_id: self.default_quote_source_id,
            instrument_quote_sources: self.instrument_quote_sources,
            quote_staleness_seconds: self.quote_staleness_seconds,
            default_draft_hold_ttl_seconds: self.default_draft_hold_ttl_seconds,
            allowed_hedge_product_codes: self.allowed_hedge_product_codes,
            regulatory_rule_pack: self
                .regulatory_rule_pack
                .map(MarketRulePackDto::into_domain)
                .transpose()?,
            limit_ceilings,
        })
    }
}

#[derive(Serialize, Deserialize)]
pub struct InstrumentCatalogEntryDto {
    pub instrument_code: String,
    pub bond: Option<BondDto>,
    pub eligibility_class: Option<String>,
    pub calendar_requirement: Option<String>,
}

impl InstrumentCatalogEntryDto {
    #[must_use]
    pub fn from_app(entry: &InstrumentCatalogEntry) -> Self {
        Self {
            instrument_code: entry.instrument_code.clone(),
            bond: entry.bond.as_ref().map(BondDto::from_domain),
            eligibility_class: entry
                .eligibility_class
                .map(|e| eligibility_to_str(e).to_owned()),
            calendar_requirement: entry
                .calendar_requirement
                .map(|r| calendar_req_to_str(r).to_owned()),
        }
    }

    pub fn into_app(self) -> ApplicationResult<InstrumentCatalogEntry> {
        Ok(InstrumentCatalogEntry {
            instrument_code: self.instrument_code,
            bond: self.bond.map(BondDto::into_domain).transpose()?,
            eligibility_class: self
                .eligibility_class
                .as_deref()
                .map(parse_eligibility)
                .transpose()?,
            calendar_requirement: self
                .calendar_requirement
                .as_deref()
                .map(parse_calendar_req)
                .transpose()?,
        })
    }
}

#[derive(Serialize, Deserialize)]
pub struct EarmarkRecordDto {
    pub earmark_id: String,
    pub earmark_revision: u64,
    pub account_id: String,
    pub instrument_code: String,
    pub side: String,
    pub settlement_bucket: String,
    pub settlement_date: String,
    pub notional: String,
    pub unreconciled_notional: String,
    pub quote_terms: Option<QuoteTermsDto>,
    pub locked_settlement_cash: Option<String>,
    pub unreconciled_settlement_cash: Option<String>,
    pub funding_precision: String,
    pub state: String,
    pub created_by: String,
    pub created_at: MarketTimeDto,
    pub expires_at: Option<MarketTimeDto>,
    pub committed_at: Option<MarketTimeDto>,
    pub overdue_alert: bool,
    pub pre_close_alert: bool,
    pub reopened_by_reversal: bool,
    pub note: String,
    pub created_seq: u64,
    pub updated_seq: u64,
    pub confirmed_by_trader_id: Option<String>,
    pub cancel_actor_id: Option<String>,
    pub cancel_reason: Option<String>,
    pub allocation_pool: AllocationPoolDto,
}

impl EarmarkRecordDto {
    #[must_use]
    pub fn from_domain(e: &EarmarkRecord) -> Self {
        Self {
            earmark_id: e.earmark_id.as_str().to_owned(),
            earmark_revision: e.earmark_revision,
            account_id: e.account_id.clone(),
            instrument_code: e.instrument_code.clone(),
            side: side_to_str(e.side).to_owned(),
            settlement_bucket: bucket_to_str(e.settlement_bucket).to_owned(),
            settlement_date: e.settlement_date.to_string(),
            notional: format_fixed_decimal(e.notional),
            unreconciled_notional: format_fixed_decimal(e.unreconciled_notional),
            quote_terms: e.quote_terms.as_ref().map(QuoteTermsDto::from_domain),
            locked_settlement_cash: e.locked_settlement_cash.map(format_fixed_decimal),
            unreconciled_settlement_cash: e.unreconciled_settlement_cash.map(format_fixed_decimal),
            funding_precision: funding_precision_to_str(e.funding_precision).to_owned(),
            state: earmark_state_to_str(e.state).to_owned(),
            created_by: e.created_by.clone(),
            created_at: MarketTimeDto::from_domain(&e.created_at),
            expires_at: e.expires_at.as_ref().map(MarketTimeDto::from_domain),
            committed_at: e.committed_at.as_ref().map(MarketTimeDto::from_domain),
            overdue_alert: e.overdue_alert,
            pre_close_alert: e.pre_close_alert,
            reopened_by_reversal: e.reopened_by_reversal,
            note: e.note.clone(),
            created_seq: e.created_seq,
            updated_seq: e.updated_seq,
            confirmed_by_trader_id: e.confirmed_by_trader_id.clone(),
            cancel_actor_id: e.cancel_actor_id.clone(),
            cancel_reason: e.cancel_reason.clone(),
            allocation_pool: AllocationPoolDto::from_domain(&e.allocation_pool),
        }
    }

    pub fn into_domain(self) -> ApplicationResult<EarmarkRecord> {
        Ok(EarmarkRecord {
            earmark_id: parse_ulid(&self.earmark_id)?,
            earmark_revision: self.earmark_revision,
            account_id: self.account_id,
            instrument_code: self.instrument_code,
            side: parse_side(&self.side)?,
            settlement_bucket: parse_bucket(&self.settlement_bucket)?,
            settlement_date: parse_naive_date(&self.settlement_date)?,
            notional: parse_fixed_decimal_storage(&self.notional)?,
            unreconciled_notional: parse_fixed_decimal_storage(&self.unreconciled_notional)?,
            quote_terms: self
                .quote_terms
                .map(QuoteTermsDto::into_domain)
                .transpose()?,
            locked_settlement_cash: self
                .locked_settlement_cash
                .as_deref()
                .map(parse_fixed_decimal_storage)
                .transpose()?,
            unreconciled_settlement_cash: self
                .unreconciled_settlement_cash
                .as_deref()
                .map(parse_fixed_decimal_storage)
                .transpose()?,
            funding_precision: parse_funding_precision(&self.funding_precision)?,
            state: parse_earmark_state(&self.state)?,
            created_by: self.created_by,
            created_at: self.created_at.into_domain()?,
            expires_at: self
                .expires_at
                .map(MarketTimeDto::into_domain)
                .transpose()?,
            committed_at: self
                .committed_at
                .map(MarketTimeDto::into_domain)
                .transpose()?,
            overdue_alert: self.overdue_alert,
            pre_close_alert: self.pre_close_alert,
            reopened_by_reversal: self.reopened_by_reversal,
            note: self.note,
            created_seq: self.created_seq,
            updated_seq: self.updated_seq,
            confirmed_by_trader_id: self.confirmed_by_trader_id,
            cancel_actor_id: self.cancel_actor_id,
            cancel_reason: self.cancel_reason,
            allocation_pool: self.allocation_pool.into_domain()?,
        })
    }
}

#[derive(Serialize, Deserialize)]
pub struct SourceTextAnchorDto {
    pub field_name: String,
    pub byte_start: u32,
    pub byte_end: u32,
    pub span_text: String,
}

#[derive(Serialize, Deserialize)]
pub struct RuleLineageDto {
    pub pack_id: String,
    pub version: u64,
    pub source: String,
    pub content_hash_hex: String,
    pub parsed_constraints_hash_hex: String,
}

#[derive(Serialize, Deserialize)]
pub struct DraftEarmarkRecordDto {
    pub draft_id: String,
    pub draft_revision: u64,
    pub account_id: String,
    pub instrument_code: String,
    pub side: String,
    pub settlement_bucket: String,
    pub notional: String,
    pub quote_terms: QuoteTermsDto,
    pub target_state: String,
    pub status: String,
    pub raw_source_text: String,
    pub anchors: Vec<SourceTextAnchorDto>,
    pub confidence_score: String,
    pub parser_source_id: String,
    pub preview_coverage_status: String,
    pub preview_delta_pre_tax_dv01: Option<String>,
    pub preview_uncovered_reason: Option<String>,
    pub preview_delta_uncovered_notional: Option<String>,
    pub preview_delta_cash_t0: Option<String>,
    pub preview_delta_cash_t1: Option<String>,
    pub preview_at_seq: u64,
    pub preview_quote_seq: u64,
    pub confirmation_token_hex: String,
    pub rule_lineage: RuleLineageDto,
    pub preview_quote_source_id: String,
    pub context_hash_hex: String,
    pub confirmed_by_trader_id: Option<String>,
    pub valuation_settlement_date: String,
}

impl DraftEarmarkRecordDto {
    #[must_use]
    pub fn from_app(d: &DraftEarmarkRecord) -> Self {
        Self {
            draft_id: d.draft_id.as_str().to_owned(),
            draft_revision: d.draft_revision,
            account_id: d.account_id.clone(),
            instrument_code: d.instrument_code.clone(),
            side: side_to_str(d.side).to_owned(),
            settlement_bucket: bucket_to_str(d.settlement_bucket).to_owned(),
            notional: format_fixed_decimal(d.notional),
            quote_terms: QuoteTermsDto::from_domain(&d.quote_terms),
            target_state: match d.target_state {
                DraftTargetState::Hold => "HOLD".to_owned(),
                DraftTargetState::Committed => "COMMITTED".to_owned(),
            },
            status: draft_status_to_str(d.status).to_owned(),
            raw_source_text: d.raw_source_text.clone(),
            anchors: d
                .anchors
                .iter()
                .map(|a| SourceTextAnchorDto {
                    field_name: a.field_name.clone(),
                    byte_start: a.byte_start,
                    byte_end: a.byte_end,
                    span_text: a.span_text.clone(),
                })
                .collect(),
            confidence_score: format_fixed_decimal(d.confidence_score),
            parser_source_id: d.parser_source_id.clone(),
            preview_coverage_status: coverage_status_to_str(d.preview_coverage_status).to_owned(),
            preview_delta_pre_tax_dv01: d.preview_delta_pre_tax_dv01.map(format_fixed_decimal),
            preview_uncovered_reason: d
                .preview_uncovered_reason
                .map(|r| uncovered_reason_to_str(r).to_owned()),
            preview_delta_uncovered_notional: d
                .preview_delta_uncovered_notional
                .map(format_fixed_decimal),
            preview_delta_cash_t0: d.preview_delta_cash_t0.map(format_fixed_decimal),
            preview_delta_cash_t1: d.preview_delta_cash_t1.map(format_fixed_decimal),
            preview_at_seq: d.preview_at_seq,
            preview_quote_seq: d.preview_quote_seq,
            confirmation_token_hex: encode_hex(d.confirmation_token.as_bytes()),
            rule_lineage: RuleLineageDto {
                pack_id: d.rule_lineage.pack_id.as_str().to_owned(),
                version: d.rule_lineage.version,
                source: d.rule_lineage.source.clone(),
                content_hash_hex: encode_hex(d.rule_lineage.content_hash.as_bytes()),
                parsed_constraints_hash_hex: encode_hex(
                    d.rule_lineage.parsed_constraints_hash.as_bytes(),
                ),
            },
            preview_quote_source_id: d.preview_quote_source_id.clone(),
            context_hash_hex: encode_hex(d.context_hash.as_bytes()),
            confirmed_by_trader_id: d.confirmed_by_trader_id.clone(),
            valuation_settlement_date: d.valuation_settlement_date.to_string(),
        }
    }

    pub fn into_app(self) -> ApplicationResult<DraftEarmarkRecord> {
        let target_state = match self.target_state.as_str() {
            "HOLD" => DraftTargetState::Hold,
            "COMMITTED" => DraftTargetState::Committed,
            _ => return Err(storage_unavailable()),
        };
        Ok(DraftEarmarkRecord {
            draft_id: parse_ulid(&self.draft_id)?,
            draft_revision: self.draft_revision,
            account_id: self.account_id,
            instrument_code: self.instrument_code,
            side: parse_side(&self.side)?,
            settlement_bucket: parse_bucket(&self.settlement_bucket)?,
            notional: parse_fixed_decimal_storage(&self.notional)?,
            quote_terms: self.quote_terms.into_domain()?,
            target_state,
            status: parse_draft_status(&self.status)?,
            raw_source_text: self.raw_source_text,
            anchors: self
                .anchors
                .into_iter()
                .map(|a| SourceTextAnchor {
                    field_name: a.field_name,
                    byte_start: a.byte_start,
                    byte_end: a.byte_end,
                    span_text: a.span_text,
                })
                .collect(),
            confidence_score: parse_fixed_decimal_storage(&self.confidence_score)?,
            parser_source_id: self.parser_source_id,
            preview_coverage_status: parse_coverage_status(&self.preview_coverage_status)?,
            preview_delta_pre_tax_dv01: self
                .preview_delta_pre_tax_dv01
                .as_deref()
                .map(parse_fixed_decimal_storage)
                .transpose()?,
            preview_uncovered_reason: self
                .preview_uncovered_reason
                .as_deref()
                .map(parse_uncovered_reason)
                .transpose()?,
            preview_delta_uncovered_notional: self
                .preview_delta_uncovered_notional
                .as_deref()
                .map(parse_fixed_decimal_storage)
                .transpose()?,
            preview_delta_cash_t0: self
                .preview_delta_cash_t0
                .as_deref()
                .map(parse_fixed_decimal_storage)
                .transpose()?,
            preview_delta_cash_t1: self
                .preview_delta_cash_t1
                .as_deref()
                .map(parse_fixed_decimal_storage)
                .transpose()?,
            preview_at_seq: self.preview_at_seq,
            preview_quote_seq: self.preview_quote_seq,
            confirmation_token: parse_content_hash_hex(&self.confirmation_token_hex)?,
            rule_lineage: RuleLineage {
                pack_id: parse_ulid(&self.rule_lineage.pack_id)?,
                version: self.rule_lineage.version,
                source: self.rule_lineage.source,
                content_hash: parse_content_hash_hex(&self.rule_lineage.content_hash_hex)?,
                parsed_constraints_hash: parse_content_hash_hex(
                    &self.rule_lineage.parsed_constraints_hash_hex,
                )?,
            },
            preview_quote_source_id: self.preview_quote_source_id,
            context_hash: parse_content_hash_hex(&self.context_hash_hex)?,
            confirmed_by_trader_id: self.confirmed_by_trader_id,
            valuation_settlement_date: parse_naive_date(&self.valuation_settlement_date)?,
        })
    }
}

#[derive(Serialize, Deserialize)]
pub struct TradeFillRecordDto {
    pub source_id: String,
    pub external_trade_id: String,
    pub source_revision: u64,
    pub feed_seq: u64,
    pub action: String,
    pub account_id: String,
    pub instrument_code: String,
    pub side: String,
    pub settlement_date: String,
    pub notional: String,
    pub quote_terms: Option<QuoteTermsDto>,
    pub executed_at: MarketTimeDto,
    pub batch_first_feed_seq: u64,
    pub batch_last_feed_seq: u64,
    pub exact_dirty_price_input: Option<ExactDirtyPriceCashInputDto>,
}

impl TradeFillRecordDto {
    #[must_use]
    pub fn from_domain(r: &TradeFillRecord) -> Self {
        Self {
            source_id: r.source_id.clone(),
            external_trade_id: r.external_trade_id.clone(),
            source_revision: r.source_revision,
            feed_seq: r.feed_seq,
            action: match r.action {
                TradeFeedRecordAction::NewFill => "NEW_FILL".to_owned(),
                TradeFeedRecordAction::Amend => "AMEND".to_owned(),
                TradeFeedRecordAction::CancelReversal => "CANCEL_REVERSAL".to_owned(),
            },
            account_id: r.account_id.clone(),
            instrument_code: r.instrument_code.clone(),
            side: side_to_str(r.side).to_owned(),
            settlement_date: r.settlement_date.to_string(),
            notional: format_fixed_decimal(r.notional),
            quote_terms: r.quote_terms.as_ref().map(QuoteTermsDto::from_domain),
            executed_at: MarketTimeDto::from_domain(&r.executed_at),
            batch_first_feed_seq: r.batch_first_feed_seq,
            batch_last_feed_seq: r.batch_last_feed_seq,
            exact_dirty_price_input: r
                .exact_dirty_price_input
                .map(ExactDirtyPriceCashInputDto::from_domain),
        }
    }

    pub fn into_domain(self) -> ApplicationResult<TradeFillRecord> {
        let action = match self.action.as_str() {
            "NEW_FILL" => TradeFeedRecordAction::NewFill,
            "AMEND" => TradeFeedRecordAction::Amend,
            "CANCEL_REVERSAL" => TradeFeedRecordAction::CancelReversal,
            _ => return Err(storage_unavailable()),
        };
        Ok(TradeFillRecord {
            source_id: self.source_id,
            external_trade_id: self.external_trade_id,
            source_revision: self.source_revision,
            feed_seq: self.feed_seq,
            action,
            account_id: self.account_id,
            instrument_code: self.instrument_code,
            side: parse_side(&self.side)?,
            settlement_date: parse_naive_date(&self.settlement_date)?,
            notional: parse_fixed_decimal_storage(&self.notional)?,
            quote_terms: self
                .quote_terms
                .map(QuoteTermsDto::into_domain)
                .transpose()?,
            executed_at: self.executed_at.into_domain()?,
            batch_first_feed_seq: self.batch_first_feed_seq,
            batch_last_feed_seq: self.batch_last_feed_seq,
            exact_dirty_price_input: self
                .exact_dirty_price_input
                .map(ExactDirtyPriceCashInputDto::into_domain)
                .transpose()?,
        })
    }
}

#[derive(Serialize, Deserialize)]
pub struct PendingMatchRecordDto {
    pub pending_match_id: String,
    pub pending_revision: u64,
    pub account_id: String,
    pub instrument_code: String,
    pub side: String,
    pub settlement_bucket: String,
    pub settlement_date: String,
    pub source_id: String,
    pub external_trade_id: String,
    pub source_revision: u64,
    pub original_notional: String,
    pub unresolved_notional: String,
    pub prior_baseline_notional: Option<String>,
    pub quote_terms: Option<QuoteTermsDto>,
    pub executed_at: MarketTimeDto,
    pub state: String,
    pub reason: String,
    pub candidate_earmark_ids: Vec<String>,
    pub created_seq: u64,
    pub updated_seq: u64,
    pub unresolved_signed_cash: Option<String>,
    pub funding_precision: String,
    pub evidence: Option<BaseEvidenceDto>,
    pub applied_delta: Option<BaselineDeltaDto>,
    pub resolution_actor_id: Option<String>,
    pub resolution_reason: Option<String>,
    pub cross_key_or_bucket_anomaly: bool,
    pub allocation_pool: AllocationPoolDto,
}

impl PendingMatchRecordDto {
    #[must_use]
    pub fn from_domain(pm: &PendingMatchRecord) -> Self {
        Self {
            pending_match_id: pm.pending_match_id.as_str().to_owned(),
            pending_revision: pm.pending_revision,
            account_id: pm.account_id.clone(),
            instrument_code: pm.instrument_code.clone(),
            side: side_to_str(pm.side).to_owned(),
            settlement_bucket: bucket_to_str(pm.settlement_bucket).to_owned(),
            settlement_date: pm.settlement_date.to_string(),
            source_id: pm.source_id.clone(),
            external_trade_id: pm.external_trade_id.clone(),
            source_revision: pm.source_revision,
            original_notional: format_fixed_decimal(pm.original_notional),
            unresolved_notional: format_fixed_decimal(pm.unresolved_notional),
            prior_baseline_notional: pm.prior_baseline_notional.map(format_fixed_decimal),
            quote_terms: pm.quote_terms.as_ref().map(QuoteTermsDto::from_domain),
            executed_at: MarketTimeDto::from_domain(&pm.executed_at),
            state: pending_state_to_str(pm.state).to_owned(),
            reason: pending_reason_to_str(pm.reason).to_owned(),
            candidate_earmark_ids: pm
                .candidate_earmark_ids
                .iter()
                .map(|id| id.as_str().to_owned())
                .collect(),
            created_seq: pm.created_seq,
            updated_seq: pm.updated_seq,
            unresolved_signed_cash: pm.unresolved_signed_cash.map(format_fixed_decimal),
            funding_precision: funding_precision_to_str(pm.funding_precision).to_owned(),
            evidence: pm.evidence.as_ref().map(BaseEvidenceDto::from_domain),
            applied_delta: pm.applied_delta.as_ref().map(BaselineDeltaDto::from_domain),
            resolution_actor_id: pm.resolution_actor_id.clone(),
            resolution_reason: pm.resolution_reason.clone(),
            cross_key_or_bucket_anomaly: pm.cross_key_or_bucket_anomaly,
            allocation_pool: AllocationPoolDto::from_domain(&pm.allocation_pool),
        }
    }

    pub fn into_domain(self) -> ApplicationResult<PendingMatchRecord> {
        Ok(PendingMatchRecord {
            pending_match_id: parse_ulid(&self.pending_match_id)?,
            pending_revision: self.pending_revision,
            account_id: self.account_id,
            instrument_code: self.instrument_code,
            side: parse_side(&self.side)?,
            settlement_bucket: parse_bucket(&self.settlement_bucket)?,
            settlement_date: parse_naive_date(&self.settlement_date)?,
            source_id: self.source_id,
            external_trade_id: self.external_trade_id,
            source_revision: self.source_revision,
            original_notional: parse_fixed_decimal_storage(&self.original_notional)?,
            unresolved_notional: parse_fixed_decimal_storage(&self.unresolved_notional)?,
            prior_baseline_notional: self
                .prior_baseline_notional
                .as_deref()
                .map(parse_fixed_decimal_storage)
                .transpose()?,
            quote_terms: self
                .quote_terms
                .map(QuoteTermsDto::into_domain)
                .transpose()?,
            executed_at: self.executed_at.into_domain()?,
            state: parse_pending_state(&self.state)?,
            reason: parse_pending_reason(&self.reason)?,
            candidate_earmark_ids: self
                .candidate_earmark_ids
                .iter()
                .map(|s| parse_ulid(s))
                .collect::<ApplicationResult<Vec<_>>>()?,
            created_seq: self.created_seq,
            updated_seq: self.updated_seq,
            unresolved_signed_cash: self
                .unresolved_signed_cash
                .as_deref()
                .map(parse_fixed_decimal_storage)
                .transpose()?,
            funding_precision: parse_funding_precision(&self.funding_precision)?,
            evidence: self
                .evidence
                .map(BaseEvidenceDto::into_domain)
                .transpose()?,
            applied_delta: self
                .applied_delta
                .map(BaselineDeltaDto::into_domain)
                .transpose()?,
            resolution_actor_id: self.resolution_actor_id,
            resolution_reason: self.resolution_reason,
            cross_key_or_bucket_anomaly: self.cross_key_or_bucket_anomaly,
            allocation_pool: self.allocation_pool.into_domain()?,
        })
    }
}

#[derive(Serialize, Deserialize)]
pub struct ReconcileAllocationRecordDto {
    pub allocation_id: String,
    pub account_id: String,
    pub instrument_code: String,
    pub side: String,
    pub settlement_bucket: String,
    pub source_id: String,
    pub external_trade_id: String,
    pub source_revision: u64,
    pub earmark_id: Option<String>,
    pub pending_match_id: Option<String>,
    pub allocated_notional: String,
    pub allocated_fill_cash: Option<String>,
    pub fill_funding_precision: String,
    pub fill_segment_index: Option<u32>,
    pub earmark_segment_index: Option<u32>,
    pub reversed_by_revision: Option<u64>,
    pub released_earmark_cash: Option<String>,
    pub baseline_delta: Option<BaselineDeltaDto>,
    pub actor_id: String,
}

impl ReconcileAllocationRecordDto {
    #[must_use]
    pub fn from_domain(a: &ReconcileAllocationRecord) -> Self {
        Self {
            allocation_id: a.allocation_id.as_str().to_owned(),
            account_id: a.account_id.clone(),
            instrument_code: a.instrument_code.clone(),
            side: side_to_str(a.side).to_owned(),
            settlement_bucket: bucket_to_str(a.settlement_bucket).to_owned(),
            source_id: a.source_id.clone(),
            external_trade_id: a.external_trade_id.clone(),
            source_revision: a.source_revision,
            earmark_id: a.earmark_id.as_ref().map(|id| id.as_str().to_owned()),
            pending_match_id: a.pending_match_id.as_ref().map(|id| id.as_str().to_owned()),
            allocated_notional: format_fixed_decimal(a.allocated_notional),
            allocated_fill_cash: a.allocated_fill_cash.map(format_fixed_decimal),
            fill_funding_precision: funding_precision_to_str(a.fill_funding_precision).to_owned(),
            fill_segment_index: a.fill_segment_index,
            earmark_segment_index: a.earmark_segment_index,
            reversed_by_revision: a.reversed_by_revision,
            released_earmark_cash: a.released_earmark_cash.map(format_fixed_decimal),
            baseline_delta: a.baseline_delta.as_ref().map(BaselineDeltaDto::from_domain),
            actor_id: a.actor_id.clone(),
        }
    }

    pub fn into_domain(self) -> ApplicationResult<ReconcileAllocationRecord> {
        Ok(ReconcileAllocationRecord {
            allocation_id: parse_ulid(&self.allocation_id)?,
            account_id: self.account_id,
            instrument_code: self.instrument_code,
            side: parse_side(&self.side)?,
            settlement_bucket: parse_bucket(&self.settlement_bucket)?,
            source_id: self.source_id,
            external_trade_id: self.external_trade_id,
            source_revision: self.source_revision,
            earmark_id: self.earmark_id.as_deref().map(parse_ulid).transpose()?,
            pending_match_id: self
                .pending_match_id
                .as_deref()
                .map(parse_ulid)
                .transpose()?,
            allocated_notional: parse_fixed_decimal_storage(&self.allocated_notional)?,
            allocated_fill_cash: self
                .allocated_fill_cash
                .as_deref()
                .map(parse_fixed_decimal_storage)
                .transpose()?,
            fill_funding_precision: parse_funding_precision(&self.fill_funding_precision)?,
            fill_segment_index: self.fill_segment_index,
            earmark_segment_index: self.earmark_segment_index,
            reversed_by_revision: self.reversed_by_revision,
            released_earmark_cash: self
                .released_earmark_cash
                .as_deref()
                .map(parse_fixed_decimal_storage)
                .transpose()?,
            baseline_delta: self
                .baseline_delta
                .map(BaselineDeltaDto::into_domain)
                .transpose()?,
            actor_id: self.actor_id,
        })
    }
}

#[derive(Serialize, Deserialize)]
pub struct SeqRangeDto {
    pub first: u64,
    pub last: u64,
}

impl SeqRangeDto {
    #[must_use]
    pub const fn from_domain(r: SeqRange) -> Self {
        Self {
            first: r.first,
            last: r.last,
        }
    }

    #[must_use]
    pub const fn into_domain(self) -> SeqRange {
        SeqRange {
            first: self.first,
            last: self.last,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct SourceBatchRecordDto {
    pub first_feed_seq: u64,
    pub last_feed_seq: u64,
    pub released: bool,
}

impl SourceBatchRecordDto {
    #[must_use]
    pub const fn from_domain(b: SourceBatchRecord) -> Self {
        Self {
            first_feed_seq: b.first_feed_seq,
            last_feed_seq: b.last_feed_seq,
            released: b.released,
        }
    }

    #[must_use]
    pub const fn into_domain(self) -> SourceBatchRecord {
        SourceBatchRecord {
            first_feed_seq: self.first_feed_seq,
            last_feed_seq: self.last_feed_seq,
            released: self.released,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct MarketQuoteTickDto {
    pub source_id: String,
    pub quote_seq: u64,
    pub instrument_code: String,
    pub source_type: String,
    pub bid_clean_price: Option<String>,
    pub ask_clean_price: Option<String>,
    pub mid_clean_price: Option<String>,
    pub mid_yield_to_maturity: Option<String>,
    pub observed_at: MarketTimeDto,
    pub received_at: MarketTimeDto,
}

impl MarketQuoteTickDto {
    #[must_use]
    pub fn from_app(t: &MarketQuoteTick) -> Self {
        Self {
            source_id: t.source_id.clone(),
            quote_seq: t.quote_seq,
            instrument_code: t.instrument_code.clone(),
            source_type: match t.source_type {
                PriceSourceType::RealTrade => "REAL_TRADE".to_owned(),
                PriceSourceType::ActiveQuote => "ACTIVE_QUOTE".to_owned(),
                PriceSourceType::ModelValuation => "MODEL_VALUATION".to_owned(),
                PriceSourceType::CurveInterpolation => "CURVE_INTERPOLATION".to_owned(),
            },
            bid_clean_price: t.bid_clean_price.map(format_fixed_decimal),
            ask_clean_price: t.ask_clean_price.map(format_fixed_decimal),
            mid_clean_price: t.mid_clean_price.map(format_fixed_decimal),
            mid_yield_to_maturity: t.mid_yield_to_maturity.map(format_fixed_decimal),
            observed_at: MarketTimeDto::from_domain(&t.observed_at),
            received_at: MarketTimeDto::from_domain(&t.received_at),
        }
    }

    pub fn into_app(self) -> ApplicationResult<MarketQuoteTick> {
        let source_type = match self.source_type.as_str() {
            "REAL_TRADE" => PriceSourceType::RealTrade,
            "ACTIVE_QUOTE" => PriceSourceType::ActiveQuote,
            "MODEL_VALUATION" => PriceSourceType::ModelValuation,
            "CURVE_INTERPOLATION" => PriceSourceType::CurveInterpolation,
            _ => return Err(storage_unavailable()),
        };
        Ok(MarketQuoteTick {
            source_id: self.source_id,
            quote_seq: self.quote_seq,
            instrument_code: self.instrument_code,
            source_type,
            bid_clean_price: self
                .bid_clean_price
                .as_deref()
                .map(parse_fixed_decimal_storage)
                .transpose()?,
            ask_clean_price: self
                .ask_clean_price
                .as_deref()
                .map(parse_fixed_decimal_storage)
                .transpose()?,
            mid_clean_price: self
                .mid_clean_price
                .as_deref()
                .map(parse_fixed_decimal_storage)
                .transpose()?,
            mid_yield_to_maturity: self
                .mid_yield_to_maturity
                .as_deref()
                .map(parse_fixed_decimal_storage)
                .transpose()?,
            observed_at: self.observed_at.into_domain()?,
            received_at: self.received_at.into_domain()?,
        })
    }
}

pub fn stored_quote_from_row(
    payload_hash_blob: &[u8],
    first_received_at_json: &str,
    first_ingested_at_json: &str,
    tick_json: &str,
) -> ApplicationResult<StoredMarketQuoteTick> {
    let payload_hash = content_hash_from_blob(payload_hash_blob)?;
    let first_received_at =
        deserialize_json::<MarketTimeDto>(first_received_at_json)?.into_domain()?;
    let first_ingested_at =
        deserialize_json::<MarketTimeDto>(first_ingested_at_json)?.into_domain()?;
    let tick = deserialize_json::<MarketQuoteTickDto>(tick_json)?.into_app()?;
    Ok(StoredMarketQuoteTick {
        tick,
        payload_hash,
        first_received_at,
        first_ingested_at,
    })
}

// ============================================================================
// Event envelope & snapshot DTOs for blotter_event_journal
// ============================================================================

#[derive(Serialize, Deserialize)]
pub struct BucketBalanceViewDto {
    pub settlement_bucket: String,
    pub settlement_date: String,
    pub available_notional: String,
    pub hold_buy_notional: String,
    pub hold_sell_notional: String,
    pub committed_buy_unreconciled_notional: String,
    pub committed_sell_unreconciled_notional: String,
    pub reconciled_buy_notional: String,
    pub reconciled_sell_notional: String,
    pub pending_match_buy_unresolved_notional: String,
    pub pending_match_sell_unresolved_notional: String,
    pub sell_ambiguity_locked: bool,
    pub negative_available_alert: bool,
}

impl BucketBalanceViewDto {
    fn from_domain(v: &BucketBalanceView) -> Self {
        Self {
            settlement_bucket: bucket_to_str(v.settlement_bucket).to_owned(),
            settlement_date: v.settlement_date.to_string(),
            available_notional: format_fixed_decimal(v.available_notional),
            hold_buy_notional: format_fixed_decimal(v.hold_buy_notional),
            hold_sell_notional: format_fixed_decimal(v.hold_sell_notional),
            committed_buy_unreconciled_notional: format_fixed_decimal(
                v.committed_buy_unreconciled_notional,
            ),
            committed_sell_unreconciled_notional: format_fixed_decimal(
                v.committed_sell_unreconciled_notional,
            ),
            reconciled_buy_notional: format_fixed_decimal(v.reconciled_buy_notional),
            reconciled_sell_notional: format_fixed_decimal(v.reconciled_sell_notional),
            pending_match_buy_unresolved_notional: format_fixed_decimal(
                v.pending_match_buy_unresolved_notional,
            ),
            pending_match_sell_unresolved_notional: format_fixed_decimal(
                v.pending_match_sell_unresolved_notional,
            ),
            sell_ambiguity_locked: v.sell_ambiguity_locked,
            negative_available_alert: v.negative_available_alert,
        }
    }

    fn into_domain(self) -> ApplicationResult<BucketBalanceView> {
        Ok(BucketBalanceView {
            settlement_bucket: parse_bucket(&self.settlement_bucket)?,
            settlement_date: parse_naive_date(&self.settlement_date)?,
            available_notional: parse_fixed_decimal_storage(&self.available_notional)?,
            hold_buy_notional: parse_fixed_decimal_storage(&self.hold_buy_notional)?,
            hold_sell_notional: parse_fixed_decimal_storage(&self.hold_sell_notional)?,
            committed_buy_unreconciled_notional: parse_fixed_decimal_storage(
                &self.committed_buy_unreconciled_notional,
            )?,
            committed_sell_unreconciled_notional: parse_fixed_decimal_storage(
                &self.committed_sell_unreconciled_notional,
            )?,
            reconciled_buy_notional: parse_fixed_decimal_storage(&self.reconciled_buy_notional)?,
            reconciled_sell_notional: parse_fixed_decimal_storage(&self.reconciled_sell_notional)?,
            pending_match_buy_unresolved_notional: parse_fixed_decimal_storage(
                &self.pending_match_buy_unresolved_notional,
            )?,
            pending_match_sell_unresolved_notional: parse_fixed_decimal_storage(
                &self.pending_match_sell_unresolved_notional,
            )?,
            sell_ambiguity_locked: self.sell_ambiguity_locked,
            negative_available_alert: self.negative_available_alert,
        })
    }
}

#[derive(Serialize, Deserialize)]
pub struct ValuedInstrumentPositionViewDto {
    pub account_id: String,
    pub instrument_code: String,
    pub position_version: u64,
    pub economic_net_notional: String,
    pub available_t0_notional: String,
    pub available_t1_notional: String,
    pub bucket_t0: BucketBalanceViewDto,
    pub bucket_t1: BucketBalanceViewDto,
    pub coverage_status: String,
    pub tax_basis: String,
    pub pre_tax_dv01: Option<String>,
    pub valuation_clean_price_per_100: Option<String>,
    pub valuation_yield_to_maturity: Option<String>,
    pub calendar_resolution: String,
    pub uncovered_reason: Option<String>,
    pub uncovered_detail: String,
    pub valuation_settlement_date: String,
    pub active_quote_source_id: String,
    pub active_quote_seq: Option<u64>,
}

impl ValuedInstrumentPositionViewDto {
    fn from_app(v: &ValuedInstrumentPositionView) -> Self {
        Self {
            account_id: v.position.account_id.clone(),
            instrument_code: v.position.instrument_code.clone(),
            position_version: v.position.position_version,
            economic_net_notional: format_fixed_decimal(v.position.economic_net_notional),
            available_t0_notional: format_fixed_decimal(v.position.available_t0_notional),
            available_t1_notional: format_fixed_decimal(v.position.available_t1_notional),
            bucket_t0: BucketBalanceViewDto::from_domain(&v.position.bucket_t0),
            bucket_t1: BucketBalanceViewDto::from_domain(&v.position.bucket_t1),
            coverage_status: coverage_status_to_str(v.coverage_status).to_owned(),
            tax_basis: tax_basis_to_str(v.tax_basis).to_owned(),
            pre_tax_dv01: v.pre_tax_dv01.map(format_fixed_decimal),
            valuation_clean_price_per_100: v
                .valuation_clean_price_per_100
                .map(format_fixed_decimal),
            valuation_yield_to_maturity: v.valuation_yield_to_maturity.map(format_fixed_decimal),
            calendar_resolution: calendar_resolution_to_str(v.calendar_resolution).to_owned(),
            uncovered_reason: v
                .uncovered_reason
                .map(|r| uncovered_reason_to_str(r).to_owned()),
            uncovered_detail: v.uncovered_detail.clone(),
            valuation_settlement_date: v.valuation_settlement_date.to_string(),
            active_quote_source_id: v.active_quote_source_id.clone(),
            active_quote_seq: v.active_quote_seq,
        }
    }

    fn into_app(self) -> ApplicationResult<ValuedInstrumentPositionView> {
        Ok(ValuedInstrumentPositionView {
            position: InstrumentPositionView {
                account_id: self.account_id,
                instrument_code: self.instrument_code,
                position_version: self.position_version,
                economic_net_notional: parse_fixed_decimal_storage(&self.economic_net_notional)?,
                available_t0_notional: parse_fixed_decimal_storage(&self.available_t0_notional)?,
                available_t1_notional: parse_fixed_decimal_storage(&self.available_t1_notional)?,
                bucket_t0: self.bucket_t0.into_domain()?,
                bucket_t1: self.bucket_t1.into_domain()?,
            },
            coverage_status: parse_coverage_status(&self.coverage_status)?,
            tax_basis: parse_tax_basis(&self.tax_basis)?,
            pre_tax_dv01: self
                .pre_tax_dv01
                .as_deref()
                .map(parse_fixed_decimal_storage)
                .transpose()?,
            valuation_clean_price_per_100: self
                .valuation_clean_price_per_100
                .as_deref()
                .map(parse_fixed_decimal_storage)
                .transpose()?,
            valuation_yield_to_maturity: self
                .valuation_yield_to_maturity
                .as_deref()
                .map(parse_fixed_decimal_storage)
                .transpose()?,
            calendar_resolution: parse_calendar_resolution(&self.calendar_resolution)?,
            uncovered_reason: self
                .uncovered_reason
                .as_deref()
                .map(parse_uncovered_reason)
                .transpose()?,
            uncovered_detail: self.uncovered_detail,
            valuation_settlement_date: parse_naive_date(&self.valuation_settlement_date)?,
            active_quote_source_id: self.active_quote_source_id,
            active_quote_seq: self.active_quote_seq,
        })
    }
}

#[derive(Serialize, Deserialize)]
pub struct RiskAndCoverageSummaryDto {
    pub covered_instrument_count: u32,
    pub uncovered_instrument_count: u32,
    pub covered_tax_basis: String,
    pub covered_net_pre_tax_dv01: String,
    pub covered_gross_notional: String,
    pub uncovered_gross_notional: String,
    pub uncovered_net_notional: String,
    pub whole_account_hedge_allowed: bool,
}

impl RiskAndCoverageSummaryDto {
    fn from_app(r: &RiskAndCoverageSummary) -> Self {
        Self {
            covered_instrument_count: r.covered_instrument_count,
            uncovered_instrument_count: r.uncovered_instrument_count,
            covered_tax_basis: tax_basis_to_str(r.covered_tax_basis).to_owned(),
            covered_net_pre_tax_dv01: format_fixed_decimal(r.covered_net_pre_tax_dv01),
            covered_gross_notional: format_fixed_decimal(r.covered_gross_notional),
            uncovered_gross_notional: format_fixed_decimal(r.uncovered_gross_notional),
            uncovered_net_notional: format_fixed_decimal(r.uncovered_net_notional),
            whole_account_hedge_allowed: r.whole_account_hedge_allowed,
        }
    }

    fn into_app(self) -> ApplicationResult<RiskAndCoverageSummary> {
        Ok(RiskAndCoverageSummary {
            covered_instrument_count: self.covered_instrument_count,
            uncovered_instrument_count: self.uncovered_instrument_count,
            covered_tax_basis: parse_tax_basis(&self.covered_tax_basis)?,
            covered_net_pre_tax_dv01: parse_fixed_decimal_storage(&self.covered_net_pre_tax_dv01)?,
            covered_gross_notional: parse_fixed_decimal_storage(&self.covered_gross_notional)?,
            uncovered_gross_notional: parse_fixed_decimal_storage(&self.uncovered_gross_notional)?,
            uncovered_net_notional: parse_fixed_decimal_storage(&self.uncovered_net_notional)?,
            whole_account_hedge_allowed: self.whole_account_hedge_allowed,
        })
    }
}

#[derive(Serialize, Deserialize)]
pub struct FundingGapSummaryDto {
    pub opening_cash_t0: String,
    pub opening_net_cash_delta_t1: String,
    pub net_settlement_flow_t0: String,
    pub net_settlement_flow_t1: String,
    pub projected_cash_t0: String,
    pub projected_cash_t1: String,
    pub funding_gap_t0: String,
    pub funding_gap_t1: String,
    pub clean_price_estimated_funding_count: u32,
    pub unpriced_funding_earmark_count: u32,
    pub funding_is_complete_exact: bool,
    pub pending_match_cash_t0: String,
    pub pending_match_cash_t1: String,
    pub pending_match_count: u32,
    pub pending_unknown_cash_count: u32,
    pub feed_incomplete: bool,
}

impl FundingGapSummaryDto {
    fn from_domain(f: &FundingGapSummary) -> Self {
        Self {
            opening_cash_t0: format_fixed_decimal(f.opening_cash_t0),
            opening_net_cash_delta_t1: format_fixed_decimal(f.opening_net_cash_delta_t1),
            net_settlement_flow_t0: format_fixed_decimal(f.net_settlement_flow_t0),
            net_settlement_flow_t1: format_fixed_decimal(f.net_settlement_flow_t1),
            projected_cash_t0: format_fixed_decimal(f.projected_cash_t0),
            projected_cash_t1: format_fixed_decimal(f.projected_cash_t1),
            funding_gap_t0: format_fixed_decimal(f.funding_gap_t0),
            funding_gap_t1: format_fixed_decimal(f.funding_gap_t1),
            clean_price_estimated_funding_count: f.clean_price_estimated_funding_count,
            unpriced_funding_earmark_count: f.unpriced_funding_earmark_count,
            funding_is_complete_exact: f.funding_is_complete_exact,
            pending_match_cash_t0: format_fixed_decimal(f.pending_match_cash_t0),
            pending_match_cash_t1: format_fixed_decimal(f.pending_match_cash_t1),
            pending_match_count: f.pending_match_count,
            pending_unknown_cash_count: f.pending_unknown_cash_count,
            feed_incomplete: f.feed_incomplete,
        }
    }

    fn into_domain(self) -> ApplicationResult<FundingGapSummary> {
        Ok(FundingGapSummary {
            opening_cash_t0: parse_fixed_decimal_storage(&self.opening_cash_t0)?,
            opening_net_cash_delta_t1: parse_fixed_decimal_storage(
                &self.opening_net_cash_delta_t1,
            )?,
            net_settlement_flow_t0: parse_fixed_decimal_storage(&self.net_settlement_flow_t0)?,
            net_settlement_flow_t1: parse_fixed_decimal_storage(&self.net_settlement_flow_t1)?,
            projected_cash_t0: parse_fixed_decimal_storage(&self.projected_cash_t0)?,
            projected_cash_t1: parse_fixed_decimal_storage(&self.projected_cash_t1)?,
            funding_gap_t0: parse_fixed_decimal_storage(&self.funding_gap_t0)?,
            funding_gap_t1: parse_fixed_decimal_storage(&self.funding_gap_t1)?,
            clean_price_estimated_funding_count: self.clean_price_estimated_funding_count,
            unpriced_funding_earmark_count: self.unpriced_funding_earmark_count,
            funding_is_complete_exact: self.funding_is_complete_exact,
            pending_match_cash_t0: parse_fixed_decimal_storage(&self.pending_match_cash_t0)?,
            pending_match_cash_t1: parse_fixed_decimal_storage(&self.pending_match_cash_t1)?,
            pending_match_count: self.pending_match_count,
            pending_unknown_cash_count: self.pending_unknown_cash_count,
            feed_incomplete: self.feed_incomplete,
        })
    }
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Serialize, Deserialize)]
pub struct FeedStatusViewDto {
    pub source_id: String,
    pub feed_status_revision: u64,
    pub health_state: String,
    pub earmarks_paused: bool,
    pub paused_by_gap: bool,
    pub paused_by_staleness: bool,
    pub paused_by_operator: bool,
    pub last_heartbeat_at: Option<MarketTimeDto>,
    pub contiguous_feed_seq: u64,
    pub max_seen_feed_seq: u64,
    pub missing_feed_ranges: Vec<SeqRangeDto>,
    pub open_pending_match_count: u32,
    pub status_reason: String,
}

impl FeedStatusViewDto {
    fn from_domain(f: &FeedStatusView) -> Self {
        Self {
            source_id: f.source_id.clone(),
            feed_status_revision: f.feed_status_revision,
            health_state: match f.health_state {
                FeedHealthState::Healthy => "HEALTHY".to_owned(),
                FeedHealthState::Stale => "STALE".to_owned(),
                FeedHealthState::GapDetected => "GAP_DETECTED".to_owned(),
                FeedHealthState::PausedByOperator => "PAUSED_BY_OPERATOR".to_owned(),
            },
            earmarks_paused: f.earmarks_paused,
            paused_by_gap: f.paused_by_gap,
            paused_by_staleness: f.paused_by_staleness,
            paused_by_operator: f.paused_by_operator,
            last_heartbeat_at: f.last_heartbeat_at.as_ref().map(MarketTimeDto::from_domain),
            contiguous_feed_seq: f.contiguous_feed_seq,
            max_seen_feed_seq: f.max_seen_feed_seq,
            missing_feed_ranges: f
                .missing_feed_ranges
                .iter()
                .copied()
                .map(SeqRangeDto::from_domain)
                .collect(),
            open_pending_match_count: f.open_pending_match_count,
            status_reason: f.status_reason.clone(),
        }
    }

    fn into_domain(self) -> ApplicationResult<FeedStatusView> {
        let health_state = match self.health_state.as_str() {
            "HEALTHY" => FeedHealthState::Healthy,
            "STALE" => FeedHealthState::Stale,
            "GAP_DETECTED" => FeedHealthState::GapDetected,
            "PAUSED_BY_OPERATOR" => FeedHealthState::PausedByOperator,
            _ => return Err(storage_unavailable()),
        };
        Ok(FeedStatusView {
            source_id: self.source_id,
            feed_status_revision: self.feed_status_revision,
            health_state,
            earmarks_paused: self.earmarks_paused,
            paused_by_gap: self.paused_by_gap,
            paused_by_staleness: self.paused_by_staleness,
            paused_by_operator: self.paused_by_operator,
            last_heartbeat_at: self
                .last_heartbeat_at
                .map(MarketTimeDto::into_domain)
                .transpose()?,
            contiguous_feed_seq: self.contiguous_feed_seq,
            max_seen_feed_seq: self.max_seen_feed_seq,
            missing_feed_ranges: self
                .missing_feed_ranges
                .into_iter()
                .map(SeqRangeDto::into_domain)
                .collect(),
            open_pending_match_count: self.open_pending_match_count,
            status_reason: self.status_reason,
        })
    }
}

#[derive(Serialize, Deserialize)]
pub struct EnrichedAccountBlotterSnapshotDto {
    pub snapshot_seq: u64,
    pub account_id: String,
    pub account_version: u64,
    pub opening_base: OpeningBaseFullImageDto,
    pub opening_instruments: Vec<InstrumentCatalogEntryDto>,
    pub risk_summary: RiskAndCoverageSummaryDto,
    pub funding_summary: FundingGapSummaryDto,
    pub feed_statuses: Vec<FeedStatusViewDto>,
    pub positions: Vec<ValuedInstrumentPositionViewDto>,
    pub active_earmarks: Vec<EarmarkRecordDto>,
    pub open_pending_matches: Vec<PendingMatchRecordDto>,
    pub pending_drafts: Vec<DraftEarmarkRecordDto>,
    pub allowed_hedge_product_codes: Vec<String>,
}

impl EnrichedAccountBlotterSnapshotDto {
    fn from_app(s: &EnrichedAccountBlotterSnapshot) -> Self {
        Self {
            snapshot_seq: s.snapshot_seq,
            account_id: s.account_id.clone(),
            account_version: s.account_version,
            opening_base: OpeningBaseFullImageDto::from_parts(
                &s.opening_base.core,
                s.opening_base.calendar.as_ref(),
                s.opening_base.default_calendar_requirement,
                &s.opening_base.units,
            ),
            opening_instruments: s
                .opening_base
                .instruments
                .iter()
                .map(InstrumentCatalogEntryDto::from_app)
                .collect(),
            risk_summary: RiskAndCoverageSummaryDto::from_app(&s.risk_summary),
            funding_summary: FundingGapSummaryDto::from_domain(&s.funding_summary),
            feed_statuses: s
                .feed_statuses
                .iter()
                .map(FeedStatusViewDto::from_domain)
                .collect(),
            positions: s
                .positions
                .iter()
                .map(ValuedInstrumentPositionViewDto::from_app)
                .collect(),
            active_earmarks: s
                .active_earmarks
                .iter()
                .map(EarmarkRecordDto::from_domain)
                .collect(),
            open_pending_matches: s
                .open_pending_matches
                .iter()
                .map(PendingMatchRecordDto::from_domain)
                .collect(),
            pending_drafts: s
                .pending_drafts
                .iter()
                .map(DraftEarmarkRecordDto::from_app)
                .collect(),
            allowed_hedge_product_codes: s.allowed_hedge_product_codes.clone(),
        }
    }

    fn into_app(self) -> ApplicationResult<EnrichedAccountBlotterSnapshot> {
        let (core, calendar, default_calendar_requirement, units) =
            self.opening_base.into_parts()?;
        let instruments = self
            .opening_instruments
            .into_iter()
            .map(InstrumentCatalogEntryDto::into_app)
            .collect::<ApplicationResult<Vec<_>>>()?;
        Ok(EnrichedAccountBlotterSnapshot {
            snapshot_seq: self.snapshot_seq,
            account_id: self.account_id,
            account_version: self.account_version,
            opening_base: FullOpeningPositionBase {
                core,
                instruments,
                calendar,
                default_calendar_requirement,
                units,
            },
            risk_summary: self.risk_summary.into_app()?,
            funding_summary: self.funding_summary.into_domain()?,
            feed_statuses: self
                .feed_statuses
                .into_iter()
                .map(FeedStatusViewDto::into_domain)
                .collect::<ApplicationResult<Vec<_>>>()?,
            positions: self
                .positions
                .into_iter()
                .map(ValuedInstrumentPositionViewDto::into_app)
                .collect::<ApplicationResult<Vec<_>>>()?,
            active_earmarks: self
                .active_earmarks
                .into_iter()
                .map(EarmarkRecordDto::into_domain)
                .collect::<ApplicationResult<Vec<_>>>()?,
            open_pending_matches: self
                .open_pending_matches
                .into_iter()
                .map(PendingMatchRecordDto::into_domain)
                .collect::<ApplicationResult<Vec<_>>>()?,
            pending_drafts: self
                .pending_drafts
                .into_iter()
                .map(DraftEarmarkRecordDto::into_app)
                .collect::<ApplicationResult<Vec<_>>>()?,
            allowed_hedge_product_codes: self.allowed_hedge_product_codes,
        })
    }
}

#[derive(Serialize, Deserialize)]
pub struct AccountChangeDto {
    pub account_id: String,
    pub replace_snapshot: EnrichedAccountBlotterSnapshotDto,
    pub changed_allocations: Vec<ReconcileAllocationRecordDto>,
}

#[derive(Serialize, Deserialize)]
pub struct BlotterEventEnvelopeDto {
    pub seq: u64,
    pub occurred_at: MarketTimeDto,
    pub changes: Vec<AccountChangeDto>,
    pub event_type: String,
}

impl BlotterEventEnvelopeDto {
    #[must_use]
    pub fn from_app(env: &BlotterEventEnvelope) -> Self {
        Self {
            seq: env.seq,
            occurred_at: MarketTimeDto::from_domain(&env.occurred_at),
            changes: env
                .changes
                .iter()
                .map(|c| AccountChangeDto {
                    account_id: c.account_id.clone(),
                    replace_snapshot: EnrichedAccountBlotterSnapshotDto::from_app(
                        &c.replace_snapshot,
                    ),
                    changed_allocations: c
                        .changed_allocations
                        .iter()
                        .map(ReconcileAllocationRecordDto::from_domain)
                        .collect(),
                })
                .collect(),
            event_type: env.event_type.clone(),
        }
    }

    pub fn into_app(self) -> ApplicationResult<BlotterEventEnvelope> {
        let mut changes = Vec::with_capacity(self.changes.len());
        for c in self.changes {
            changes.push(AccountChange {
                account_id: c.account_id,
                replace_snapshot: c.replace_snapshot.into_app()?,
                changed_allocations: c
                    .changed_allocations
                    .into_iter()
                    .map(ReconcileAllocationRecordDto::into_domain)
                    .collect::<ApplicationResult<Vec<_>>>()?,
            });
        }
        Ok(BlotterEventEnvelope {
            seq: self.seq,
            occurred_at: self.occurred_at.into_domain()?,
            changes,
            event_type: self.event_type,
        })
    }
}

// ============================================================================
// Private parsing helpers
// ============================================================================

pub fn parse_fixed_decimal_storage(text: &str) -> ApplicationResult<FixedDecimal> {
    parse_fixed_decimal(text).map_err(|_| storage_unavailable())
}

pub fn parse_ulid(text: &str) -> ApplicationResult<Ulid> {
    Ulid::new(text).map_err(|_| storage_unavailable())
}

pub fn parse_version(value: u64) -> ApplicationResult<Version> {
    Version::new(value).map_err(|_| storage_unavailable())
}

pub fn parse_naive_date(text: &str) -> ApplicationResult<NaiveDate> {
    NaiveDate::parse_from_str(text, "%Y-%m-%d").map_err(|_| storage_unavailable())
}

pub fn parse_content_hash_hex(hex: &str) -> ApplicationResult<ContentHash> {
    let bytes = decode_hex(hex)?;
    ContentHash::from_bytes(&bytes).map_err(|_| storage_unavailable())
}

pub fn parse_u64_set(json: &str) -> ApplicationResult<BTreeSet<u64>> {
    deserialize_json(json)
}

fn parse_side(text: &str) -> ApplicationResult<TradeSide> {
    match text {
        "BUY" => Ok(TradeSide::Buy),
        "SELL" => Ok(TradeSide::Sell),
        _ => Err(storage_unavailable()),
    }
}

fn parse_bucket(text: &str) -> ApplicationResult<SettlementBucket> {
    match text {
        "T_PLUS_0" => Ok(SettlementBucket::TPlus0),
        "T_PLUS_1" => Ok(SettlementBucket::TPlus1),
        _ => Err(storage_unavailable()),
    }
}

fn parse_earmark_state(text: &str) -> ApplicationResult<EarmarkState> {
    match text {
        "HOLD" => Ok(EarmarkState::Hold),
        "COMMITTED" => Ok(EarmarkState::Committed),
        "EXPIRED" => Ok(EarmarkState::Expired),
        "RELEASED" => Ok(EarmarkState::Released),
        "CANCELLED" => Ok(EarmarkState::Cancelled),
        "RECONCILED" => Ok(EarmarkState::Reconciled),
        _ => Err(storage_unavailable()),
    }
}

fn parse_funding_precision(text: &str) -> ApplicationResult<FundingPrecision> {
    match text {
        "EXACT_SETTLEMENT_AMOUNT" => Ok(FundingPrecision::ExactSettlementAmount),
        "EXACT_DIRTY_PRICE" => Ok(FundingPrecision::ExactDirtyPrice),
        "CLEAN_PRICE_ESTIMATE" => Ok(FundingPrecision::CleanPriceEstimate),
        "UNPRICED_YIELD_ONLY" => Ok(FundingPrecision::UnpricedYieldOnly),
        _ => Err(storage_unavailable()),
    }
}

fn parse_draft_status(text: &str) -> ApplicationResult<DraftEarmarkStatus> {
    match text {
        "PENDING_CONFIRMATION" => Ok(DraftEarmarkStatus::PendingConfirmation),
        "CONFIRMED" => Ok(DraftEarmarkStatus::Confirmed),
        "DISCARDED" => Ok(DraftEarmarkStatus::Discarded),
        _ => Err(storage_unavailable()),
    }
}

fn parse_pending_state(text: &str) -> ApplicationResult<PendingMatchState> {
    match text {
        "OPEN" => Ok(PendingMatchState::Open),
        "RESOLVED_MATCHED" => Ok(PendingMatchState::ResolvedMatched),
        "RESOLVED_ORPHAN_ACCEPTED" => Ok(PendingMatchState::ResolvedOrphanAccepted),
        "RELEASED" => Ok(PendingMatchState::Released),
        "REVERSED_BY_FEED" => Ok(PendingMatchState::ReversedByFeed),
        "RESOLVED_BASELINE_DELTA" => Ok(PendingMatchState::ResolvedBaselineDelta),
        _ => Err(storage_unavailable()),
    }
}

fn parse_pending_reason(text: &str) -> ApplicationResult<PendingMatchReason> {
    match text {
        "AMBIGUOUS_CANDIDATES" => Ok(PendingMatchReason::AmbiguousCandidates),
        "NOTIONAL_MISMATCH_OR_SPLIT" => Ok(PendingMatchReason::NotionalMismatchOrSplit),
        "PRICE_OUT_OF_TOLERANCE" => Ok(PendingMatchReason::PriceOutOfTolerance),
        "ORPHAN_NO_COMMITMENT" => Ok(PendingMatchReason::OrphanNoCommitment),
        "BURST_REPEAT" => Ok(PendingMatchReason::BurstRepeat),
        "EXISTING_PENDING_ON_KEY" => Ok(PendingMatchReason::ExistingPendingOnKey),
        "AMENDMENT_MISMATCH" => Ok(PendingMatchReason::AmendmentMismatch),
        "BASELINE_REVISION_MANUAL" => Ok(PendingMatchReason::BaselineRevisionManual),
        "INCOMPARABLE_BASIS" => Ok(PendingMatchReason::IncomparableBasis),
        "REVISION_HOLD" => Ok(PendingMatchReason::RevisionHold),
        "GAP_HOLD" => Ok(PendingMatchReason::GapHold),
        _ => Err(storage_unavailable()),
    }
}

const fn eligibility_to_str(value: RateCoverageEligibility) -> &'static str {
    match value {
        RateCoverageEligibility::SovereignOrPolicyFixed => "SOVEREIGN_OR_POLICY_FIXED",
        RateCoverageEligibility::CreditOrSpreadProduct => "CREDIT_OR_SPREAD_PRODUCT",
        RateCoverageEligibility::FloatingOrIndexed => "FLOATING_OR_INDEXED",
        RateCoverageEligibility::EmbeddedOptionOrSinking => "EMBEDDED_OPTION_OR_SINKING",
    }
}

fn parse_eligibility(text: &str) -> ApplicationResult<RateCoverageEligibility> {
    match text {
        "SOVEREIGN_OR_POLICY_FIXED" => Ok(RateCoverageEligibility::SovereignOrPolicyFixed),
        "CREDIT_OR_SPREAD_PRODUCT" => Ok(RateCoverageEligibility::CreditOrSpreadProduct),
        "FLOATING_OR_INDEXED" => Ok(RateCoverageEligibility::FloatingOrIndexed),
        "EMBEDDED_OPTION_OR_SINKING" => Ok(RateCoverageEligibility::EmbeddedOptionOrSinking),
        _ => Err(storage_unavailable()),
    }
}

const fn calendar_req_to_str(value: CalendarRequirement) -> &'static str {
    match value {
        CalendarRequirement::ReferenceReplay => "REFERENCE_REPLAY",
        CalendarRequirement::ExactMarket => "EXACT_MARKET",
    }
}

fn parse_calendar_req(text: &str) -> ApplicationResult<CalendarRequirement> {
    match text {
        "REFERENCE_REPLAY" => Ok(CalendarRequirement::ReferenceReplay),
        "EXACT_MARKET" => Ok(CalendarRequirement::ExactMarket),
        _ => Err(storage_unavailable()),
    }
}

const fn coverage_status_to_str(value: CoverageStatus) -> &'static str {
    match value {
        CoverageStatus::CoveredPreTax => "COVERED_PRE_TAX",
        CoverageStatus::Uncovered => "UNCOVERED",
        CoverageStatus::InactiveZeroPosition => "INACTIVE_ZERO_POSITION",
    }
}

fn parse_coverage_status(text: &str) -> ApplicationResult<CoverageStatus> {
    match text {
        "COVERED_PRE_TAX" => Ok(CoverageStatus::CoveredPreTax),
        "UNCOVERED" => Ok(CoverageStatus::Uncovered),
        "INACTIVE_ZERO_POSITION" => Ok(CoverageStatus::InactiveZeroPosition),
        _ => Err(storage_unavailable()),
    }
}

const fn uncovered_reason_to_str(value: UncoveredReason) -> &'static str {
    match value {
        UncoveredReason::InstrumentSpecMissing => "INSTRUMENT_SPEC_MISSING",
        UncoveredReason::UnsupportedCouponStructure => "UNSUPPORTED_COUPON_STRUCTURE",
        UncoveredReason::EmbeddedOptionOrCallable => "EMBEDDED_OPTION_OR_CALLABLE",
        UncoveredReason::UnsupportedConvention => "UNSUPPORTED_CONVENTION",
        UncoveredReason::MissingMarketQuote => "MISSING_MARKET_QUOTE",
        UncoveredReason::CalendarCoverageMissing => "CALENDAR_COVERAGE_MISSING",
        UncoveredReason::KernelPricingFailed => "KERNEL_PRICING_FAILED",
        UncoveredReason::EligibilityUnverified => "ELIGIBILITY_UNVERIFIED",
        UncoveredReason::CreditOrNonRatesIssuer => "CREDIT_OR_NON_RATES_ISSUER",
        UncoveredReason::StaleQuote => "STALE_QUOTE",
    }
}

fn parse_uncovered_reason(text: &str) -> ApplicationResult<UncoveredReason> {
    match text {
        "INSTRUMENT_SPEC_MISSING" => Ok(UncoveredReason::InstrumentSpecMissing),
        "UNSUPPORTED_COUPON_STRUCTURE" => Ok(UncoveredReason::UnsupportedCouponStructure),
        "EMBEDDED_OPTION_OR_CALLABLE" => Ok(UncoveredReason::EmbeddedOptionOrCallable),
        "UNSUPPORTED_CONVENTION" => Ok(UncoveredReason::UnsupportedConvention),
        "MISSING_MARKET_QUOTE" => Ok(UncoveredReason::MissingMarketQuote),
        "CALENDAR_COVERAGE_MISSING" => Ok(UncoveredReason::CalendarCoverageMissing),
        "KERNEL_PRICING_FAILED" => Ok(UncoveredReason::KernelPricingFailed),
        "ELIGIBILITY_UNVERIFIED" => Ok(UncoveredReason::EligibilityUnverified),
        "CREDIT_OR_NON_RATES_ISSUER" => Ok(UncoveredReason::CreditOrNonRatesIssuer),
        "STALE_QUOTE" => Ok(UncoveredReason::StaleQuote),
        _ => Err(storage_unavailable()),
    }
}

const fn tax_basis_to_str(value: BondTaxBasis) -> &'static str {
    match value {
        BondTaxBasis::PreTax => "PRE_TAX",
        BondTaxBasis::TaxAdjusted => "TAX_ADJUSTED",
    }
}

fn parse_tax_basis(text: &str) -> ApplicationResult<BondTaxBasis> {
    match text {
        "PRE_TAX" => Ok(BondTaxBasis::PreTax),
        "TAX_ADJUSTED" => Ok(BondTaxBasis::TaxAdjusted),
        _ => Err(storage_unavailable()),
    }
}

const fn calendar_resolution_to_str(value: CalendarResolutionMode) -> &'static str {
    match value {
        CalendarResolutionMode::Unspecified => "UNSPECIFIED",
        CalendarResolutionMode::Exact => "EXACT",
        CalendarResolutionMode::ProvisionalWeekendBeyondCoverage => {
            "PROVISIONAL_WEEKEND_BEYOND_COVERAGE"
        }
    }
}

fn parse_calendar_resolution(text: &str) -> ApplicationResult<CalendarResolutionMode> {
    match text {
        "UNSPECIFIED" => Ok(CalendarResolutionMode::Unspecified),
        "EXACT" => Ok(CalendarResolutionMode::Exact),
        "PROVISIONAL_WEEKEND_BEYOND_COVERAGE" => {
            Ok(CalendarResolutionMode::ProvisionalWeekendBeyondCoverage)
        }
        _ => Err(storage_unavailable()),
    }
}

const fn validation_failed() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}

const fn storage_unavailable() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::StorageUnavailable, true)
}
