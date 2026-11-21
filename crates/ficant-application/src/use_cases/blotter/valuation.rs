use chrono::{NaiveDate, TimeZone};
use chrono_tz::Asia::Shanghai;
use ficant_domain::analytics::{
    AnalyticsError, AnalyticsMode, AnalyticsObjectRef, BondAnalyticsInput, BondTaxBasis, BondTerms,
    BusinessDayConvention, CalendarBinding, CalendarRequirement, CalendarResolution,
    CouponFrequency, DECIMAL_SCALE, DayCountConvention, FixedDecimal, MARKET_TIMEZONE,
};
use ficant_domain::blotter::{
    AccountState, ExactDirtyPriceCashInput, QuoteBasis, QuoteTerms, project_instrument_position,
    wide_mul_div_fixed_round_half_even,
};
use ficant_domain::futures_hedge::FuturesHedgeInput;
use ficant_domain::market::{
    Bond, BondBusinessDayConvention, BondCouponFrequency, BondDayCountConvention,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::{ContentAddressed, DomainErrorCode, VersionedDefinition};

use crate::ports::{
    ApplicationResult, BondAnalyticsEngine, FuturesDeliveryRuleParser, FuturesHedgeEngine,
};
use crate::use_cases::blotter::model::{
    AccountMetadataState, AnalysisUnits, CalendarResolutionMode, CoverageStatus,
    DeskBlotterWorkspaceState, InstrumentCatalogEntry, MarketQuoteTick, PreviewCoveredHedgeInput,
    PreviewCoveredHedgeMeasures, PreviewCoveredHedgeOutput, PreviewHedgeScope,
    RateCoverageEligibility, RiskAndCoverageSummary, STANDARD_PAR_UNITS, UncoveredReason,
    ValuedInstrumentPositionView, append_fixed, append_str, append_u64, invalid,
    require_non_empty_text,
};
use crate::use_cases::blotter::quotes::is_quote_stale;
use crate::use_cases::{
    CalculateBondAnalytics, CalculateFuturesHedge, parse_verified_delivery_rule_for_per_100_hedge,
};
use crate::{ApplicationError, ApplicationErrorCategory, map_blotter_error, map_domain_error};

const PLACEHOLDER_ULID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

/// Result of standardized per-100 `PRE_TAX` bond pricing preserving original bond lineage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StandardizedBondPricing {
    pub d100: FixedDecimal,
    pub clean_price_per_100: FixedDecimal,
    pub dirty_price_per_100: FixedDecimal,
    pub yield_to_maturity: FixedDecimal,
    pub calendar_resolution: CalendarResolutionMode,
    pub original_face_value: FixedDecimal,
}

/// Outcome of evaluating one instrument's valuation eligibility and `PRE_TAX` pricing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstrumentPricingOutcome {
    Covered {
        pricing: StandardizedBondPricing,
        active_quote_source_id: String,
        active_quote_seq: u64,
        quote_payload_hash: ContentHash,
    },
    Uncovered {
        reason: UncoveredReason,
        detail: String,
        active_quote_source_id: String,
        active_quote_seq: Option<u64>,
        quote_payload_hash: Option<ContentHash>,
        quote_is_stale: bool,
    },
}

/// Resolves the effective [`CalendarRequirement`] following the strict BP-2 precedence law:
/// 1. Valuation settlement date MUST fall inside `[calendar.coverage_start(), calendar.coverage_end()]`.
/// 2. If either account or instrument specifies `ExactMarket`, `ExactMarket` is enforced.
/// 3. Else if either specifies `ReferenceReplay`, `ReferenceReplay` is used.
/// 4. Else (both unspecified), auto-determine via calendar coverage (`ReferenceReplay` lets the
///    kernel report `Exact` when all adjusted dates are covered and `ProvisionalWeekendOnly`
///    when future dates extend beyond coverage).
pub fn resolve_effective_calendar_requirement(
    account_req: Option<CalendarRequirement>,
    instrument_req: Option<CalendarRequirement>,
    settlement_date: NaiveDate,
    calendar: &CalendarBinding,
) -> Result<CalendarRequirement, UncoveredReason> {
    if settlement_date < calendar.coverage_start() || settlement_date > calendar.coverage_end() {
        return Err(UncoveredReason::CalendarCoverageMissing);
    }
    if account_req == Some(CalendarRequirement::ExactMarket)
        || instrument_req == Some(CalendarRequirement::ExactMarket)
    {
        return Ok(CalendarRequirement::ExactMarket);
    }
    if account_req == Some(CalendarRequirement::ReferenceReplay)
        || instrument_req == Some(CalendarRequirement::ReferenceReplay)
    {
        return Ok(CalendarRequirement::ReferenceReplay);
    }
    Ok(CalendarRequirement::ReferenceReplay)
}

/// Converts a domain [`DecimalValue`] to a 12-scale [`FixedDecimal`], validating its [`UnitRef`].
pub fn decimal_value_to_fixed(
    value: &DecimalValue,
    expected_unit: &UnitRef,
) -> ApplicationResult<FixedDecimal> {
    if value.unit() != expected_unit {
        return Err(invalid());
    }
    let coefficient = value.coefficient().parse::<i128>().map_err(|_| invalid())?;
    let scale = value.scale();
    let scaled = if scale <= DECIMAL_SCALE {
        let factor = 10_i128
            .checked_pow(DECIMAL_SCALE - scale)
            .ok_or_else(invalid)?;
        coefficient.checked_mul(factor).ok_or_else(invalid)?
    } else {
        let divisor = 10_i128
            .checked_pow(scale - DECIMAL_SCALE)
            .ok_or_else(invalid)?;
        if coefficient % divisor != 0 {
            return Err(invalid());
        }
        coefficient / divisor
    };
    Ok(FixedDecimal::from_scaled(scaled))
}

/// Computes signed `position_dv01 = round_half_even(Q * d100 / 100)` using safe wide integers.
///
/// Long positions (`Q > 0`) produce positive DV01; short positions (`Q < 0`) produce negative
/// DV01 with exact opposite sign (`dv01(-Q) == -dv01(+Q)`).
pub fn position_dv01_from_d100(
    economic_net_notional: FixedDecimal,
    d100: FixedDecimal,
) -> ApplicationResult<FixedDecimal> {
    wide_mul_div_fixed_round_half_even(economic_net_notional, d100, STANDARD_PAR_UNITS)
        .map_err(|err| map_blotter_error(&err))
}

/// Prices a fixed-rate bond via `CalculateBondAnalytics` using standardized per-100 par scaling
/// while preserving original bond and face-value lineage.
#[allow(clippy::too_many_arguments)]
pub fn price_standardized_pre_tax_bond(
    bond: &Bond,
    units: &AnalysisUnits,
    calendar: &CalendarBinding,
    account_calendar_req: Option<CalendarRequirement>,
    instrument_calendar_req: Option<CalendarRequirement>,
    settlement_date: NaiveDate,
    valuation_at: &MarketTime,
    mode: AnalyticsMode,
    input_value: FixedDecimal,
    bond_engine: &dyn BondAnalyticsEngine,
) -> Result<StandardizedBondPricing, UncoveredReason> {
    let pricing_terms = bond
        .pricing_terms()
        .ok_or(UncoveredReason::InstrumentSpecMissing)?;
    let original_face_value = decimal_value_to_fixed(bond.face_value(), &units.currency_amount)
        .map_err(|_| UncoveredReason::UnsupportedConvention)?;
    if !original_face_value.is_positive() {
        return Err(UncoveredReason::UnsupportedConvention);
    }
    let coupon_rate = decimal_value_to_fixed(pricing_terms.coupon_rate(), &units.rate)
        .map_err(|_| UncoveredReason::UnsupportedConvention)?;

    let effective_cal_req = resolve_effective_calendar_requirement(
        account_calendar_req,
        instrument_calendar_req,
        settlement_date,
        calendar,
    )?;

    let frequency = match pricing_terms.frequency() {
        BondCouponFrequency::Annual => CouponFrequency::Annual,
        BondCouponFrequency::Semiannual => CouponFrequency::Semiannual,
    };
    let day_count = match pricing_terms.day_count() {
        BondDayCountConvention::ActActBondIsma => DayCountConvention::ActActBondIsma,
    };
    let business_day = match pricing_terms.business_day() {
        BondBusinessDayConvention::Following => BusinessDayConvention::Following,
    };

    let standardized_terms = BondTerms::with_issuance_and_optional_tax(
        bond.first_issue_date(),
        bond.current_issue_date(),
        bond.maturity_date(),
        frequency,
        day_count,
        business_day,
        coupon_rate,
        STANDARD_PAR_UNITS,
        STANDARD_PAR_UNITS,
        bond.tax_attributes(),
    )
    .map_err(|_| UncoveredReason::UnsupportedConvention)?;

    let bond_lineage_hash = compute_bond_lineage_hash(bond, original_face_value, coupon_rate);
    let bond_ref = AnalyticsObjectRef::new(bond.instrument().clone(), bond_lineage_hash);

    let analytics_input = BondAnalyticsInput::new(
        placeholder_owner(),
        bond_ref,
        placeholder_object_ref(),
        placeholder_object_ref(),
        valuation_at.clone(),
        settlement_date,
        effective_cal_req,
        calendar.clone(),
        standardized_terms,
        mode,
        input_value,
    )
    .map_err(|err| match err {
        DomainErrorCode::InvalidEffectiveTime => UncoveredReason::CalendarCoverageMissing,
        _ => UncoveredReason::KernelPricingFailed,
    })?;

    // Call the engine through CalculateBondAnalytics to enforce PRE_TAX lineage validation,
    // while mapping AnalyticsError::CalendarCoverageMissing cleanly before ApplicationError erasure.
    let result = bond_engine
        .calculate(&analytics_input)
        .map_err(|err| match err {
            AnalyticsError::CalendarCoverageMissing => UncoveredReason::CalendarCoverageMissing,
            _ => UncoveredReason::KernelPricingFailed,
        })?;
    result
        .validate_against(&analytics_input)
        .map_err(|_| UncoveredReason::KernelPricingFailed)?;
    if result.tax_basis() != BondTaxBasis::PreTax {
        return Err(UncoveredReason::KernelPricingFailed);
    }

    let calendar_resolution = match result.calendar_resolution() {
        CalendarResolution::Exact => CalendarResolutionMode::Exact,
        CalendarResolution::ProvisionalWeekendOnly => {
            CalendarResolutionMode::ProvisionalWeekendBeyondCoverage
        }
    };

    Ok(StandardizedBondPricing {
        d100: result.measures().dv01(),
        clean_price_per_100: result.measures().clean_price(),
        dirty_price_per_100: result.measures().dirty_price(),
        yield_to_maturity: result.measures().yield_to_maturity(),
        calendar_resolution,
        original_face_value,
    })
}

/// Evaluates an instrument's valuation eligibility, active market quote, and standardized
/// `PRE_TAX` pricing at `opening_base.trading_date`.
///
/// Shared by `GetAccountSnapshot`, `SubmitDraftEarmark` / `ConfirmDraftEarmark`, and
/// `PreviewCoveredHedge`.
#[allow(clippy::too_many_lines)]
pub fn evaluate_instrument_pricing(
    workspace: &DeskBlotterWorkspaceState,
    account: &AccountState,
    meta: &AccountMetadataState,
    instrument_code: &str,
    now: &MarketTime,
    bond_engine: &dyn BondAnalyticsEngine,
) -> InstrumentPricingOutcome {
    let active_source = meta
        .policy
        .active_quote_source_for(instrument_code)
        .to_owned();
    let latest_quote = workspace.quotes.latest_for(&active_source, instrument_code);
    let active_quote_seq = latest_quote.map(|q| q.tick.quote_seq);
    let quote_payload_hash = latest_quote.map(|q| q.payload_hash.clone());
    let quote_is_stale =
        latest_quote.is_some_and(|q| is_quote_stale(q, now, meta.policy.quote_staleness_seconds));

    let Some(entry) = meta.instruments.get(instrument_code) else {
        return InstrumentPricingOutcome::Uncovered {
            reason: UncoveredReason::InstrumentSpecMissing,
            detail: "instrument catalog entry missing".to_owned(),
            active_quote_source_id: active_source,
            active_quote_seq,
            quote_payload_hash,
            quote_is_stale,
        };
    };

    if let Some(reason) = eligibility_uncovered_reason(entry) {
        return InstrumentPricingOutcome::Uncovered {
            reason,
            detail: uncovered_detail_for(reason).to_owned(),
            active_quote_source_id: active_source,
            active_quote_seq,
            quote_payload_hash,
            quote_is_stale,
        };
    }

    let Some(bond) = &entry.bond else {
        return InstrumentPricingOutcome::Uncovered {
            reason: UncoveredReason::InstrumentSpecMissing,
            detail: "bond specification missing".to_owned(),
            active_quote_source_id: active_source,
            active_quote_seq,
            quote_payload_hash,
            quote_is_stale,
        };
    };

    if bond.pricing_terms().is_none() {
        return InstrumentPricingOutcome::Uncovered {
            reason: UncoveredReason::InstrumentSpecMissing,
            detail: "bond pricing terms missing".to_owned(),
            active_quote_source_id: active_source,
            active_quote_seq,
            quote_payload_hash,
            quote_is_stale,
        };
    }

    if bond.face_value().unit() != &meta.units.currency_amount {
        return InstrumentPricingOutcome::Uncovered {
            reason: UncoveredReason::UnsupportedConvention,
            detail: "bond face_value unit does not match account currency unit".to_owned(),
            active_quote_source_id: active_source,
            active_quote_seq,
            quote_payload_hash,
            quote_is_stale,
        };
    }

    let Some(calendar) = &meta.calendar else {
        return InstrumentPricingOutcome::Uncovered {
            reason: UncoveredReason::CalendarCoverageMissing,
            detail: "account market calendar missing".to_owned(),
            active_quote_source_id: active_source,
            active_quote_seq,
            quote_payload_hash,
            quote_is_stale,
        };
    };

    let Some(stored_quote) = latest_quote else {
        return InstrumentPricingOutcome::Uncovered {
            reason: UncoveredReason::MissingMarketQuote,
            detail: "no market quote available for active quote source".to_owned(),
            active_quote_source_id: active_source,
            active_quote_seq: None,
            quote_payload_hash: None,
            quote_is_stale: false,
        };
    };

    if quote_is_stale {
        return InstrumentPricingOutcome::Uncovered {
            reason: UncoveredReason::StaleQuote,
            detail: "active market quote exceeded staleness threshold".to_owned(),
            active_quote_source_id: active_source,
            active_quote_seq,
            quote_payload_hash,
            quote_is_stale: true,
        };
    }

    let Some((mode, input_value)) = extract_quote_mode_and_value(&stored_quote.tick) else {
        return InstrumentPricingOutcome::Uncovered {
            reason: UncoveredReason::MissingMarketQuote,
            detail: "active market quote has no positive clean price or yield".to_owned(),
            active_quote_source_id: active_source,
            active_quote_seq,
            quote_payload_hash,
            quote_is_stale: false,
        };
    };

    let valuation_settlement_date = account.opening_base.trading_date;
    let Ok(valuation_at) = shanghai_market_time_for_date(now, valuation_settlement_date) else {
        return InstrumentPricingOutcome::Uncovered {
            reason: UncoveredReason::KernelPricingFailed,
            detail: "invalid valuation market timestamp".to_owned(),
            active_quote_source_id: active_source,
            active_quote_seq,
            quote_payload_hash,
            quote_is_stale: false,
        };
    };

    match price_standardized_pre_tax_bond(
        bond,
        &meta.units,
        calendar,
        meta.default_calendar_requirement,
        entry.calendar_requirement,
        valuation_settlement_date,
        &valuation_at,
        mode,
        input_value,
        bond_engine,
    ) {
        Ok(pricing) => InstrumentPricingOutcome::Covered {
            pricing,
            active_quote_source_id: active_source,
            active_quote_seq: stored_quote.tick.quote_seq,
            quote_payload_hash: stored_quote.payload_hash.clone(),
        },
        Err(reason) => InstrumentPricingOutcome::Uncovered {
            reason,
            detail: uncovered_detail_for(reason).to_owned(),
            active_quote_source_id: active_source,
            active_quote_seq,
            quote_payload_hash,
            quote_is_stale: false,
        },
    }
}

/// Evaluates all positions and the [`RiskAndCoverageSummary`] for one account at `now`.
#[allow(clippy::too_many_lines)]
pub fn evaluate_account_positions_and_risk(
    workspace: &DeskBlotterWorkspaceState,
    account_id: &str,
    now: &MarketTime,
    bond_engine: &dyn BondAnalyticsEngine,
) -> ApplicationResult<(Vec<ValuedInstrumentPositionView>, RiskAndCoverageSummary)> {
    let account = workspace
        .domain
        .accounts
        .get(account_id)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))?;
    let meta = workspace
        .accounts_meta
        .get(account_id)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))?;

    let mut valued_positions = Vec::with_capacity(account.positions.len());
    let mut covered_count = 0_u32;
    let mut uncovered_count = 0_u32;
    let mut covered_net_dv01 = FixedDecimal::ZERO;
    let mut covered_gross_notional = FixedDecimal::ZERO;
    let mut uncovered_gross_notional = FixedDecimal::ZERO;
    let mut uncovered_net_notional = FixedDecimal::ZERO;

    let valuation_settlement_date = account.opening_base.trading_date;

    for instrument_code in account.positions.keys() {
        let pos_view = project_instrument_position(account, instrument_code)
            .map_err(|err| map_blotter_error(&err))?;

        if pos_view.economic_net_notional == FixedDecimal::ZERO {
            let active_source = meta
                .policy
                .active_quote_source_for(instrument_code)
                .to_owned();
            let active_seq = workspace
                .quotes
                .latest_for(&active_source, instrument_code)
                .map(|q| q.tick.quote_seq);
            valued_positions.push(ValuedInstrumentPositionView {
                position: pos_view,
                coverage_status: CoverageStatus::InactiveZeroPosition,
                tax_basis: BondTaxBasis::PreTax,
                pre_tax_dv01: None,
                valuation_clean_price_per_100: None,
                valuation_yield_to_maturity: None,
                calendar_resolution: CalendarResolutionMode::Unspecified,
                uncovered_reason: None,
                uncovered_detail: String::new(),
                valuation_settlement_date,
                active_quote_source_id: active_source,
                active_quote_seq: active_seq,
            });
            continue;
        }

        let abs_notional = FixedDecimal::from_scaled(pos_view.economic_net_notional.scaled().abs());
        match evaluate_instrument_pricing(
            workspace,
            account,
            meta,
            instrument_code,
            now,
            bond_engine,
        ) {
            InstrumentPricingOutcome::Covered {
                pricing,
                active_quote_source_id,
                active_quote_seq,
                ..
            } => {
                let dv01 = position_dv01_from_d100(pos_view.economic_net_notional, pricing.d100)?;
                covered_count = covered_count.saturating_add(1);
                covered_net_dv01 = covered_net_dv01
                    .checked_add(dv01)
                    .map_err(map_domain_error)?;
                covered_gross_notional = covered_gross_notional
                    .checked_add(abs_notional)
                    .map_err(map_domain_error)?;

                valued_positions.push(ValuedInstrumentPositionView {
                    position: pos_view,
                    coverage_status: CoverageStatus::CoveredPreTax,
                    tax_basis: BondTaxBasis::PreTax,
                    pre_tax_dv01: Some(dv01),
                    valuation_clean_price_per_100: Some(pricing.clean_price_per_100),
                    valuation_yield_to_maturity: Some(pricing.yield_to_maturity),
                    calendar_resolution: pricing.calendar_resolution,
                    uncovered_reason: None,
                    uncovered_detail: String::new(),
                    valuation_settlement_date,
                    active_quote_source_id,
                    active_quote_seq: Some(active_quote_seq),
                });
            }
            InstrumentPricingOutcome::Uncovered {
                reason,
                detail,
                active_quote_source_id,
                active_quote_seq,
                ..
            } => {
                uncovered_count = uncovered_count.saturating_add(1);
                uncovered_gross_notional = uncovered_gross_notional
                    .checked_add(abs_notional)
                    .map_err(map_domain_error)?;
                uncovered_net_notional = uncovered_net_notional
                    .checked_add(pos_view.economic_net_notional)
                    .map_err(map_domain_error)?;

                valued_positions.push(ValuedInstrumentPositionView {
                    position: pos_view,
                    coverage_status: CoverageStatus::Uncovered,
                    tax_basis: BondTaxBasis::PreTax,
                    pre_tax_dv01: None,
                    valuation_clean_price_per_100: None,
                    valuation_yield_to_maturity: None,
                    calendar_resolution: CalendarResolutionMode::Unspecified,
                    uncovered_reason: Some(reason),
                    uncovered_detail: detail,
                    valuation_settlement_date,
                    active_quote_source_id,
                    active_quote_seq,
                });
            }
        }
    }

    let summary = RiskAndCoverageSummary {
        covered_instrument_count: covered_count,
        uncovered_instrument_count: uncovered_count,
        covered_tax_basis: BondTaxBasis::PreTax,
        covered_net_pre_tax_dv01: covered_net_dv01,
        covered_gross_notional,
        uncovered_gross_notional,
        uncovered_net_notional,
        whole_account_hedge_allowed: uncovered_count == 0,
    };

    Ok((valued_positions, summary))
}

/// Evaluates one valued instrument position view for an account.
pub fn evaluate_single_instrument_position(
    workspace: &DeskBlotterWorkspaceState,
    account_id: &str,
    instrument_code: &str,
    now: &MarketTime,
    bond_engine: &dyn BondAnalyticsEngine,
) -> ApplicationResult<ValuedInstrumentPositionView> {
    let account = workspace
        .domain
        .accounts
        .get(account_id)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))?;
    let meta = workspace
        .accounts_meta
        .get(account_id)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))?;
    let pos_view = project_instrument_position(account, instrument_code)
        .map_err(|err| map_blotter_error(&err))?;
    let valuation_settlement_date = account.opening_base.trading_date;

    if pos_view.economic_net_notional == FixedDecimal::ZERO {
        let active_source = meta
            .policy
            .active_quote_source_for(instrument_code)
            .to_owned();
        let active_seq = workspace
            .quotes
            .latest_for(&active_source, instrument_code)
            .map(|q| q.tick.quote_seq);
        return Ok(ValuedInstrumentPositionView {
            position: pos_view,
            coverage_status: CoverageStatus::InactiveZeroPosition,
            tax_basis: BondTaxBasis::PreTax,
            pre_tax_dv01: None,
            valuation_clean_price_per_100: None,
            valuation_yield_to_maturity: None,
            calendar_resolution: CalendarResolutionMode::Unspecified,
            uncovered_reason: None,
            uncovered_detail: String::new(),
            valuation_settlement_date,
            active_quote_source_id: active_source,
            active_quote_seq: active_seq,
        });
    }

    match evaluate_instrument_pricing(workspace, account, meta, instrument_code, now, bond_engine) {
        InstrumentPricingOutcome::Covered {
            pricing,
            active_quote_source_id,
            active_quote_seq,
            ..
        } => {
            let dv01 = position_dv01_from_d100(pos_view.economic_net_notional, pricing.d100)?;
            Ok(ValuedInstrumentPositionView {
                position: pos_view,
                coverage_status: CoverageStatus::CoveredPreTax,
                tax_basis: BondTaxBasis::PreTax,
                pre_tax_dv01: Some(dv01),
                valuation_clean_price_per_100: Some(pricing.clean_price_per_100),
                valuation_yield_to_maturity: Some(pricing.yield_to_maturity),
                calendar_resolution: pricing.calendar_resolution,
                uncovered_reason: None,
                uncovered_detail: String::new(),
                valuation_settlement_date,
                active_quote_source_id,
                active_quote_seq: Some(active_quote_seq),
            })
        }
        InstrumentPricingOutcome::Uncovered {
            reason,
            detail,
            active_quote_source_id,
            active_quote_seq,
            ..
        } => Ok(ValuedInstrumentPositionView {
            position: pos_view,
            coverage_status: CoverageStatus::Uncovered,
            tax_basis: BondTaxBasis::PreTax,
            pre_tax_dv01: None,
            valuation_clean_price_per_100: None,
            valuation_yield_to_maturity: None,
            calendar_resolution: CalendarResolutionMode::Unspecified,
            uncovered_reason: Some(reason),
            uncovered_detail: detail,
            valuation_settlement_date,
            active_quote_source_id,
            active_quote_seq,
        }),
    }
}

/// Computes deterministic trade settlement-date exact dirty-price input (`ExactDirtyPriceCashInput`)
/// for a trade's own `settlement_date` and `quote_terms`.
///
/// Returns `None` if explicit `settlement_amount` is already supplied, or if the instrument is
/// uncovered / lacks a positive trade quote, allowing `resolve_settlement_cash` to fall back cleanly.
#[must_use]
pub fn compute_trade_exact_dirty_price_input(
    meta: &AccountMetadataState,
    instrument_code: &str,
    trade_settlement_date: NaiveDate,
    quote_terms: Option<&QuoteTerms>,
    bond_engine: &dyn BondAnalyticsEngine,
) -> Option<ExactDirtyPriceCashInput> {
    let terms = quote_terms?;
    if terms.settlement_amount.is_some() {
        return None;
    }
    let (mode, input_value) = match terms.basis? {
        QuoteBasis::CleanPricePer100(price) if price.is_positive() => {
            (AnalyticsMode::PriceIn, price)
        }
        QuoteBasis::YieldToMaturity(ytm) if ytm.is_positive() => (AnalyticsMode::YieldIn, ytm),
        _ => return None,
    };

    let entry = meta.instruments.get(instrument_code)?;
    if eligibility_uncovered_reason(entry).is_some() {
        return None;
    }
    let bond = entry.bond.as_ref()?;
    let calendar = meta.calendar.as_ref()?;
    let deterministic_val_time = shanghai_noon(trade_settlement_date).ok()?;

    let priced = price_standardized_pre_tax_bond(
        bond,
        &meta.units,
        calendar,
        meta.default_calendar_requirement,
        entry.calendar_requirement,
        trade_settlement_date,
        &deterministic_val_time,
        mode,
        input_value,
        bond_engine,
    )
    .ok()?;

    Some(ExactDirtyPriceCashInput {
        dirty_price: priced.dirty_price_per_100,
        face_value: STANDARD_PAR_UNITS,
    })
}

/// Executes `PreviewCoveredHedge` with strict pre-validation of contract, external policy
/// allowlist, CTD/CF, and verified delivery `RulePack` BEFORE handling zero net exposure.
#[allow(clippy::too_many_lines)]
pub fn execute_preview_covered_hedge(
    workspace: &DeskBlotterWorkspaceState,
    input: &PreviewCoveredHedgeInput,
    now: &MarketTime,
    bond_engine: &dyn BondAnalyticsEngine,
    hedge_engine: &dyn FuturesHedgeEngine,
    delivery_rule_parser: &dyn FuturesDeliveryRuleParser,
) -> ApplicationResult<PreviewCoveredHedgeOutput> {
    require_non_empty_text(&input.account_id)?;
    require_non_empty_text(&input.product_code)?;
    require_non_empty_text(&input.contract_code)?;

    let meta = workspace
        .accounts_meta
        .get(&input.account_id)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))?;
    if !workspace.domain.accounts.contains_key(&input.account_id) {
        return Err(ApplicationError::new(
            ApplicationErrorCategory::NotFound,
            false,
        ));
    }

    if let Some(expected_seq) = input.expected_snapshot_seq
        && expected_seq != workspace.domain.head_seq
    {
        return Err(ApplicationError::new(
            ApplicationErrorCategory::ConcurrencyConflict,
            true,
        ));
    }

    // Validate product code against externally configured desk policy allowlist (zero hardcoded codes).
    if !meta
        .policy
        .allowed_hedge_product_codes
        .iter()
        .any(|allowed| allowed == &input.product_code)
    {
        return Err(ApplicationError::blotter_hedge_product_not_allowed());
    }

    let product = delivery_rule_parser.parse_product_code(&input.product_code)?;
    if !input.ctd_dv01_per_100.is_positive() || !input.conversion_factor.is_positive() {
        return Err(invalid());
    }

    if let Some(val_at) = &input.valuation_at
        && val_at.market_timezone() != MARKET_TIMEZONE
    {
        return Err(invalid());
    }
    let eval_time = input.valuation_at.as_ref().unwrap_or(now);

    let delivery_rule = parse_verified_delivery_rule_for_per_100_hedge(
        &input.delivery_rule_pack,
        Some(eval_time),
        product,
        delivery_rule_parser,
    )?;
    let resolved_notional = delivery_rule
        .contract_notional()
        .map_err(map_domain_error)?
        .ok_or_else(invalid)?;
    let resolved_basis = delivery_rule.face_quote_basis();

    // Evaluate covered book risk using the shared PRE_TAX valuation function.
    let (_positions, risk_summary) =
        evaluate_account_positions_and_risk(workspace, &input.account_id, now, bond_engine)?;

    if input.scope == PreviewHedgeScope::WholeAccount && risk_summary.uncovered_instrument_count > 0
    {
        return Err(ApplicationError::blotter_whole_account_hedge_blocked());
    }
    if risk_summary.covered_instrument_count == 0 {
        return Err(ApplicationError::blotter_empty_covered_book());
    }

    if risk_summary.covered_net_pre_tax_dv01 == FixedDecimal::ZERO {
        let scaled_ctd = wide_mul_div_fixed_round_half_even(
            input.ctd_dv01_per_100,
            resolved_notional,
            resolved_basis,
        )
        .map_err(|err| map_blotter_error(&err))?;
        let futures_contract_dv01 = scaled_ctd
            .checked_div_round_ties_even(input.conversion_factor)
            .map_err(map_domain_error)?;
        if !futures_contract_dv01.is_positive() {
            return Err(invalid());
        }

        return Ok(PreviewCoveredHedgeOutput {
            evaluated_at_seq: workspace.domain.head_seq,
            scope: input.scope,
            product_code: input.product_code.clone(),
            contract_code: input.contract_code.clone(),
            zero_net_exposure: true,
            covered_tax_basis: BondTaxBasis::PreTax,
            covered_instrument_count: risk_summary.covered_instrument_count,
            uncovered_instrument_count: risk_summary.uncovered_instrument_count,
            covered_net_pre_tax_dv01: FixedDecimal::ZERO,
            excluded_uncovered_gross_notional: risk_summary.uncovered_gross_notional,
            measures: PreviewCoveredHedgeMeasures {
                futures_contract_dv01,
                raw_contracts: FixedDecimal::ZERO,
                recommended_contracts: 0,
                residual_dv01: FixedDecimal::ZERO,
                hedge_effectiveness: FixedDecimal::ONE,
                contract_notional: resolved_notional,
            },
        });
    }

    let pack_ref = AnalyticsObjectRef::new(
        VersionRef::new(
            Ulid::new(input.delivery_rule_pack.identity()).map_err(map_domain_error)?,
            Version::new(input.delivery_rule_pack.version()).map_err(map_domain_error)?,
        ),
        input.delivery_rule_pack.content_hash().clone(),
    );

    let hedge_input = FuturesHedgeInput::new(
        placeholder_owner(),
        placeholder_object_ref(),
        placeholder_object_ref(),
        placeholder_object_ref(),
        placeholder_object_ref(),
        placeholder_object_ref(),
        pack_ref,
        placeholder_object_ref(),
        Some(eval_time.clone()),
        product,
        resolved_notional,
        resolved_basis,
        risk_summary.covered_net_pre_tax_dv01,
        input.ctd_dv01_per_100,
        input.conversion_factor,
    )
    .map_err(map_domain_error)?;

    let result = CalculateFuturesHedge::new(hedge_engine).execute(&hedge_input)?;
    let m = result.measures();

    Ok(PreviewCoveredHedgeOutput {
        evaluated_at_seq: workspace.domain.head_seq,
        scope: input.scope,
        product_code: input.product_code.clone(),
        contract_code: input.contract_code.clone(),
        zero_net_exposure: false,
        covered_tax_basis: BondTaxBasis::PreTax,
        covered_instrument_count: risk_summary.covered_instrument_count,
        uncovered_instrument_count: risk_summary.uncovered_instrument_count,
        covered_net_pre_tax_dv01: risk_summary.covered_net_pre_tax_dv01,
        excluded_uncovered_gross_notional: risk_summary.uncovered_gross_notional,
        measures: PreviewCoveredHedgeMeasures {
            futures_contract_dv01: m.futures_contract_dv01(),
            raw_contracts: m.raw_contracts(),
            recommended_contracts: m.recommended_contracts(),
            residual_dv01: m.residual_dv01(),
            hedge_effectiveness: m.hedge_effectiveness(),
            contract_notional: resolved_notional,
        },
    })
}

/// Helper to directly invoke `CalculateBondAnalytics` on an unnormalized original `face_value`
/// bond (e.g. for cross-checking `Q * dF / F` against standardized `Q * d100 / 100`).
#[allow(clippy::too_many_arguments)]
pub fn price_original_face_pre_tax_bond(
    bond: &Bond,
    units: &AnalysisUnits,
    calendar: &CalendarBinding,
    calendar_requirement: CalendarRequirement,
    settlement_date: NaiveDate,
    valuation_at: &MarketTime,
    mode: AnalyticsMode,
    input_value: FixedDecimal,
    bond_engine: &dyn BondAnalyticsEngine,
) -> ApplicationResult<ficant_domain::analytics::BondAnalyticsResult> {
    let pricing_terms = bond.pricing_terms().ok_or_else(invalid)?;
    let original_face_value = decimal_value_to_fixed(bond.face_value(), &units.currency_amount)?;
    let coupon_rate = decimal_value_to_fixed(pricing_terms.coupon_rate(), &units.rate)?;
    let frequency = match pricing_terms.frequency() {
        BondCouponFrequency::Annual => CouponFrequency::Annual,
        BondCouponFrequency::Semiannual => CouponFrequency::Semiannual,
    };
    let day_count = match pricing_terms.day_count() {
        BondDayCountConvention::ActActBondIsma => DayCountConvention::ActActBondIsma,
    };
    let business_day = match pricing_terms.business_day() {
        BondBusinessDayConvention::Following => BusinessDayConvention::Following,
    };
    let terms = BondTerms::with_issuance_and_optional_tax(
        bond.first_issue_date(),
        bond.current_issue_date(),
        bond.maturity_date(),
        frequency,
        day_count,
        business_day,
        coupon_rate,
        original_face_value,
        original_face_value,
        bond.tax_attributes(),
    )
    .map_err(map_domain_error)?;

    let bond_ref = AnalyticsObjectRef::new(
        bond.instrument().clone(),
        compute_bond_lineage_hash(bond, original_face_value, coupon_rate),
    );
    let input = BondAnalyticsInput::new(
        placeholder_owner(),
        bond_ref,
        placeholder_object_ref(),
        placeholder_object_ref(),
        valuation_at.clone(),
        settlement_date,
        calendar_requirement,
        calendar.clone(),
        terms,
        mode,
        input_value,
    )
    .map_err(map_domain_error)?;
    CalculateBondAnalytics::new(bond_engine).execute(&input)
}

fn eligibility_uncovered_reason(entry: &InstrumentCatalogEntry) -> Option<UncoveredReason> {
    match entry.eligibility_class {
        Some(RateCoverageEligibility::SovereignOrPolicyFixed) => None,
        Some(RateCoverageEligibility::CreditOrSpreadProduct) => {
            Some(UncoveredReason::CreditOrNonRatesIssuer)
        }
        Some(RateCoverageEligibility::FloatingOrIndexed) => {
            Some(UncoveredReason::UnsupportedCouponStructure)
        }
        Some(RateCoverageEligibility::EmbeddedOptionOrSinking) => {
            Some(UncoveredReason::EmbeddedOptionOrCallable)
        }
        None => Some(UncoveredReason::EligibilityUnverified),
    }
}

fn uncovered_detail_for(reason: UncoveredReason) -> &'static str {
    match reason {
        UncoveredReason::InstrumentSpecMissing => "instrument specification missing",
        UncoveredReason::UnsupportedCouponStructure => "unsupported coupon structure",
        UncoveredReason::EmbeddedOptionOrCallable => "embedded option or sinking structure",
        UncoveredReason::UnsupportedConvention => "unsupported bond convention or unit mismatch",
        UncoveredReason::MissingMarketQuote => "missing active market quote",
        UncoveredReason::CalendarCoverageMissing => "required calendar coverage missing",
        UncoveredReason::KernelPricingFailed => "native kernel pricing failed",
        UncoveredReason::EligibilityUnverified => "rate coverage eligibility unverified",
        UncoveredReason::CreditOrNonRatesIssuer => "credit or non-rates issuer product",
        UncoveredReason::StaleQuote => "active market quote is stale",
    }
}

fn extract_quote_mode_and_value(tick: &MarketQuoteTick) -> Option<(AnalyticsMode, FixedDecimal)> {
    if let Some(mid) = tick.mid_clean_price
        && mid.is_positive()
    {
        return Some((AnalyticsMode::PriceIn, mid));
    }
    if let (Some(bid), Some(ask)) = (tick.bid_clean_price, tick.ask_clean_price)
        && bid.is_positive()
        && ask >= bid
        && let Ok(sum) = bid.checked_add(ask)
        && let Ok(mid) =
            sum.checked_div_round_ties_even(FixedDecimal::from_scaled(2_000_000_000_000))
        && mid.is_positive()
    {
        return Some((AnalyticsMode::PriceIn, mid));
    }
    if let Some(ytm) = tick.mid_yield_to_maturity
        && ytm.is_positive()
    {
        return Some((AnalyticsMode::YieldIn, ytm));
    }
    None
}

fn compute_bond_lineage_hash(
    bond: &Bond,
    original_face_value: FixedDecimal,
    coupon_rate: FixedDecimal,
) -> ContentHash {
    let mut bytes = Vec::new();
    append_str(&mut bytes, bond.instrument().id().as_str());
    append_u64(&mut bytes, bond.instrument().version().get());
    append_str(&mut bytes, &bond.first_issue_date().to_string());
    append_str(&mut bytes, &bond.current_issue_date().to_string());
    append_str(&mut bytes, &bond.maturity_date().to_string());
    append_fixed(&mut bytes, original_face_value);
    append_fixed(&mut bytes, coupon_rate);
    ContentHash::digest(&bytes)
}

fn shanghai_noon(date: NaiveDate) -> ApplicationResult<MarketTime> {
    let local = date.and_hms_opt(12, 0, 0).ok_or_else(invalid)?;
    let instant = Shanghai
        .from_local_datetime(&local)
        .single()
        .ok_or_else(invalid)?
        .to_utc();
    MarketTime::new(instant, MARKET_TIMEZONE, date).map_err(map_domain_error)
}

fn shanghai_market_time_for_date(
    now: &MarketTime,
    trading_date: NaiveDate,
) -> ApplicationResult<MarketTime> {
    if now.market_timezone() == MARKET_TIMEZONE {
        return Ok(now.clone());
    }
    shanghai_noon(trading_date)
}

fn placeholder_owner() -> OwnerRef {
    let id = Ulid::new(PLACEHOLDER_ULID).expect("constant ULID is valid");
    OwnerRef::new(id.clone(), id)
}

fn placeholder_object_ref() -> AnalyticsObjectRef {
    let id = Ulid::new(PLACEHOLDER_ULID).expect("constant ULID is valid");
    let version = Version::new(1).expect("version 1 is valid");
    AnalyticsObjectRef::new(
        VersionRef::new(id, version),
        ContentHash::digest(b"blotter"),
    )
}
