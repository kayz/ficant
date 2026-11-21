use std::collections::BTreeMap;

use chrono::NaiveDate;
use ficant_domain::analytics::{BondTaxBasis, CalendarBinding, CalendarRequirement, FixedDecimal};
use ficant_domain::blotter::{
    BlotterConfig, BlotterState, EarmarkRecord, FeedStatusView, FundingGapSummary,
    InstrumentPositionView, OpeningPositionBase, PendingMatchRecord, QuoteBasis, QuoteTerms,
    ReconcileAllocationRecord, SettlementBucket, TradeFillRecord, TradeSide,
};
use ficant_domain::market::{Bond, MarketRulePack, PriceSourceType};
use ficant_domain::primitives::{ContentHash, MarketTime, Ulid, UnitRef};

use crate::{ApplicationError, ApplicationErrorCategory, ports::ApplicationResult};

/// Standardized per-100 par scaling unit used for fixed-rate bond DV01 normalization.
///
/// Using a fixed 100-unit par call ensures bonds with `face_value = 100` and `face_value = 1000`
/// (identical economic terms and quotes) yield bit-for-bit identical `PRE_TAX` DV01, draft delta
/// DV01, and futures hedge contracts.
pub(crate) const STANDARD_PAR_UNITS: FixedDecimal = FixedDecimal::from_scaled(100_000_000_000_000);

/// Positive coverage eligibility class declared in the instrument catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RateCoverageEligibility {
    SovereignOrPolicyFixed,
    CreditOrSpreadProduct,
    FloatingOrIndexed,
    EmbeddedOptionOrSinking,
}

/// Valuation coverage status for an instrument position or draft preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CoverageStatus {
    CoveredPreTax,
    Uncovered,
    InactiveZeroPosition,
}

/// Explicit reason an instrument position or draft preview is uncovered for `PRE_TAX` DV01.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UncoveredReason {
    InstrumentSpecMissing,
    UnsupportedCouponStructure,
    EmbeddedOptionOrCallable,
    UnsupportedConvention,
    MissingMarketQuote,
    CalendarCoverageMissing,
    KernelPricingFailed,
    EligibilityUnverified,
    CreditOrNonRatesIssuer,
    StaleQuote,
}

/// Calendar resolution mode echoed in valued instrument positions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CalendarResolutionMode {
    Unspecified,
    Exact,
    ProvisionalWeekendBeyondCoverage,
}

/// Scope selector for `PreviewCoveredHedge`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PreviewHedgeScope {
    WholeAccount,
    CoveredSubsetOnly,
}

/// Target lifecycle state when a draft earmark is confirmed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DraftTargetState {
    Hold,
    Committed,
}

/// Lifecycle status of a draft earmark.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DraftEarmarkStatus {
    PendingConfirmation,
    Confirmed,
    Discarded,
}

/// Canonical unit bindings matching `ficant.rates.v1.AnalysisUnits`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnalysisUnits {
    pub currency_amount: UnitRef,
    pub price_per_100: UnitRef,
    pub rate: UnitRef,
    pub dv01: UnitRef,
    pub dv01_per_100: UnitRef,
    pub dimensionless: UnitRef,
    pub contract_count: UnitRef,
}

/// Instrument specification entry attached to an account's opening position base.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstrumentCatalogEntry {
    pub instrument_code: String,
    pub bond: Option<Bond>,
    /// `None` represents `RATE_COVERAGE_ELIGIBILITY_UNSPECIFIED` (fail-closed as
    /// `UncoveredReason::EligibilityUnverified`).
    pub eligibility_class: Option<RateCoverageEligibility>,
    /// `None` represents `CALENDAR_REQUIREMENT_UNSPECIFIED` (inherit / auto-determine).
    pub calendar_requirement: Option<CalendarRequirement>,
}

/// Full L2 opening position base extending the L1 `OpeningPositionBase` with bond specs,
/// calendar binding, default calendar requirement, and unit definitions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FullOpeningPositionBase {
    pub core: OpeningPositionBase,
    pub instruments: Vec<InstrumentCatalogEntry>,
    pub calendar: Option<CalendarBinding>,
    /// `None` represents `CALENDAR_REQUIREMENT_UNSPECIFIED`.
    pub default_calendar_requirement: Option<CalendarRequirement>,
    pub units: AnalysisUnits,
}

/// Server-configured per-account desk policy, rule pack binding, and active quote source selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountDeskPolicy {
    pub policy_version: u64,
    pub default_quote_source_id: String,
    /// Optional per-instrument active valuation quote source overrides (`instrument_code -> source_id`).
    pub instrument_quote_sources: BTreeMap<String, String>,
    /// Maximum allowed age in seconds for market quotes before being classified `StaleQuote`.
    pub quote_staleness_seconds: u32,
    /// Hold TTL used when confirming a draft targeting `DraftTargetState::Hold` (`1800..=2700`).
    pub default_draft_hold_ttl_seconds: u32,
    /// Externally configured allowlist of futures hedge product codes (never hardcoded in L2).
    pub allowed_hedge_product_codes: Vec<String>,
    /// Server-selected trusted regulatory `MarketRulePack` bound to this account for draft validation.
    pub regulatory_rule_pack: Option<MarketRulePack>,
    /// Account-level limit ceilings keyed by `limit_code` for binding `ConstraintSet`.
    pub limit_ceilings: BTreeMap<String, FixedDecimal>,
}

impl AccountDeskPolicy {
    pub fn validate(&self) -> ApplicationResult<()> {
        if self.policy_version == 0 {
            return Err(invalid());
        }
        require_non_empty_text(&self.default_quote_source_id)?;
        for (inst, src) in &self.instrument_quote_sources {
            require_non_empty_text(inst)?;
            require_non_empty_text(src)?;
        }
        if self.quote_staleness_seconds == 0 {
            return Err(invalid());
        }
        if !(ficant_domain::blotter::MIN_HOLD_TTL_SECONDS
            ..=ficant_domain::blotter::MAX_HOLD_TTL_SECONDS)
            .contains(&self.default_draft_hold_ttl_seconds)
        {
            return Err(invalid());
        }
        for code in &self.allowed_hedge_product_codes {
            require_non_empty_text(code)?;
        }
        for (limit_code, ceiling) in &self.limit_ceilings {
            require_non_empty_text(limit_code)?;
            if !ceiling.is_non_negative() {
                return Err(invalid());
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn active_quote_source_for(&self, instrument_code: &str) -> &str {
        self.instrument_quote_sources
            .get(instrument_code)
            .map_or(self.default_quote_source_id.as_str(), String::as_str)
    }
}

/// Incoming market quote tick keyed by `(source_id, instrument_code, quote_seq)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarketQuoteTick {
    pub source_id: String,
    pub quote_seq: u64,
    pub instrument_code: String,
    pub source_type: PriceSourceType,
    pub bid_clean_price: Option<FixedDecimal>,
    pub ask_clean_price: Option<FixedDecimal>,
    pub mid_clean_price: Option<FixedDecimal>,
    pub mid_yield_to_maturity: Option<FixedDecimal>,
    pub observed_at: MarketTime,
    pub received_at: MarketTime,
}

impl MarketQuoteTick {
    pub fn validate(&self) -> ApplicationResult<()> {
        require_non_empty_text(&self.source_id)?;
        require_non_empty_text(&self.instrument_code)?;
        if self.quote_seq == 0 {
            return Err(invalid());
        }
        if self.observed_at.instant() > self.received_at.instant() {
            return Err(invalid());
        }
        if let Some(bid) = self.bid_clean_price
            && !bid.is_positive()
        {
            return Err(invalid());
        }
        if let Some(ask) = self.ask_clean_price
            && !ask.is_positive()
        {
            return Err(invalid());
        }
        if let (Some(bid), Some(ask)) = (self.bid_clean_price, self.ask_clean_price)
            && bid > ask
        {
            return Err(invalid());
        }
        if let Some(mid) = self.mid_clean_price
            && !mid.is_positive()
        {
            return Err(invalid());
        }
        if let Some(ytm) = self.mid_yield_to_maturity
            && !ytm.is_non_negative()
        {
            return Err(invalid());
        }
        let has_usable_price = self.mid_clean_price.is_some()
            || (self.bid_clean_price.is_some() && self.ask_clean_price.is_some())
            || self.mid_yield_to_maturity.is_some();
        if !has_usable_price {
            return Err(invalid());
        }
        Ok(())
    }

    /// Canonical digest of market quote content for `(source_id, instrument_code, quote_seq)`.
    ///
    /// Deliberately excludes transport `received_at` so re-delivering the exact same quote
    /// deduplicates cleanly without extending `first_received_at`, while any change to price,
    /// yield, `source_type`, or `observed_at` triggers `HashMismatch`.
    #[must_use]
    pub fn canonical_payload_hash(&self) -> ContentHash {
        let mut bytes = Vec::new();
        append_str(&mut bytes, &self.source_id);
        append_str(&mut bytes, &self.instrument_code);
        append_u64(&mut bytes, self.quote_seq);
        append_u8(
            &mut bytes,
            match self.source_type {
                PriceSourceType::RealTrade => 1,
                PriceSourceType::ActiveQuote => 2,
                PriceSourceType::ModelValuation => 3,
                PriceSourceType::CurveInterpolation => 4,
            },
        );
        append_opt_fixed(&mut bytes, self.bid_clean_price);
        append_opt_fixed(&mut bytes, self.ask_clean_price);
        append_opt_fixed(&mut bytes, self.mid_clean_price);
        append_opt_fixed(&mut bytes, self.mid_yield_to_maturity);
        append_market_time(&mut bytes, &self.observed_at);
        ContentHash::digest(&bytes)
    }
}

/// Stored historical quote tick retaining immutable first-receipt timestamps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredMarketQuoteTick {
    pub tick: MarketQuoteTick,
    pub payload_hash: ContentHash,
    pub first_received_at: MarketTime,
    pub first_ingested_at: MarketTime,
}

/// Quote stream cursor scoped strictly to one `(source_id, instrument_code)` pair.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct QuoteCursor {
    pub source_id: String,
    pub instrument_code: String,
    pub after_quote_seq: u64,
}

/// Result of `TradeFeedReconcilerPort::pull_trade_feed_batch`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullTradeFeedBatchResult {
    pub fills: Vec<TradeFillRecord>,
    pub scanned_through_feed_seq: u64,
    pub has_more: bool,
}

/// Result of `MarketQuoteStreamPort::pull_market_quotes`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullMarketQuotesResult {
    pub ticks: Vec<MarketQuoteTick>,
    pub next_cursor: QuoteCursor,
    pub has_more: bool,
}

/// Per-`(source_id, instrument_code)` market quote history and active head map.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QuoteStoreState {
    pub history: BTreeMap<(String, String, u64), StoredMarketQuoteTick>,
    pub heads: BTreeMap<(String, String), u64>,
}

impl QuoteStoreState {
    #[must_use]
    pub fn latest_for(
        &self,
        source_id: &str,
        instrument_code: &str,
    ) -> Option<&StoredMarketQuoteTick> {
        let head_seq = *self
            .heads
            .get(&(source_id.to_owned(), instrument_code.to_owned()))?;
        self.history
            .get(&(source_id.to_owned(), instrument_code.to_owned(), head_seq))
    }
}

/// Half-open UTF-8 byte span anchoring an NLP-extracted field to `raw_source_text`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceTextAnchor {
    pub field_name: String,
    pub byte_start: u32,
    pub byte_end: u32,
    pub span_text: String,
}

/// Provenance of the verified regulatory `MarketRulePack` and bound `ConstraintSet` ceilings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleLineage {
    pub pack_id: Ulid,
    pub version: u64,
    pub source: String,
    pub content_hash: ContentHash,
    pub parsed_constraints_hash: ContentHash,
}

/// NLP or trader draft proposal submitted to `SubmitDraftEarmark`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NlpEarmarkProposal {
    pub parser_source_id: String,
    pub model_version: String,
    pub raw_source_text: String,
    pub anchors: Vec<SourceTextAnchor>,
    pub confidence_score: FixedDecimal,
    pub account_id: String,
    pub instrument_code: String,
    pub side: TradeSide,
    pub settlement_bucket: SettlementBucket,
    pub notional: Option<FixedDecimal>,
    pub quote_terms: Option<QuoteTerms>,
    pub suggested_target_state: DraftTargetState,
}

/// Full L2 draft earmark record carrying rule lineage, preview valuation, and confirmation token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftEarmarkRecord {
    pub draft_id: Ulid,
    pub draft_revision: u64,
    pub account_id: String,
    pub instrument_code: String,
    pub side: TradeSide,
    pub settlement_bucket: SettlementBucket,
    pub notional: FixedDecimal,
    pub quote_terms: QuoteTerms,
    pub target_state: DraftTargetState,
    pub status: DraftEarmarkStatus,
    pub raw_source_text: String,
    pub anchors: Vec<SourceTextAnchor>,
    pub confidence_score: FixedDecimal,
    pub parser_source_id: String,
    pub preview_coverage_status: CoverageStatus,
    pub preview_delta_pre_tax_dv01: Option<FixedDecimal>,
    pub preview_uncovered_reason: Option<UncoveredReason>,
    pub preview_delta_uncovered_notional: Option<FixedDecimal>,
    pub preview_delta_cash_t0: Option<FixedDecimal>,
    pub preview_delta_cash_t1: Option<FixedDecimal>,
    pub preview_at_seq: u64,
    pub preview_quote_seq: u64,
    pub confirmation_token: ContentHash,
    pub rule_lineage: RuleLineage,
    pub preview_quote_source_id: String,
    pub context_hash: ContentHash,
    pub confirmed_by_trader_id: Option<String>,
    pub valuation_settlement_date: NaiveDate,
}

/// Enriched instrument position view combining L1 bucket balances with L2 `PRE_TAX` valuation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValuedInstrumentPositionView {
    pub position: InstrumentPositionView,
    pub coverage_status: CoverageStatus,
    pub tax_basis: BondTaxBasis,
    pub pre_tax_dv01: Option<FixedDecimal>,
    pub valuation_clean_price_per_100: Option<FixedDecimal>,
    pub valuation_yield_to_maturity: Option<FixedDecimal>,
    pub calendar_resolution: CalendarResolutionMode,
    pub uncovered_reason: Option<UncoveredReason>,
    pub uncovered_detail: String,
    pub valuation_settlement_date: NaiveDate,
    pub active_quote_source_id: String,
    pub active_quote_seq: Option<u64>,
}

/// Account-level `PRE_TAX` DV01 risk and coverage summary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RiskAndCoverageSummary {
    pub covered_instrument_count: u32,
    pub uncovered_instrument_count: u32,
    pub covered_tax_basis: BondTaxBasis,
    pub covered_net_pre_tax_dv01: FixedDecimal,
    pub covered_gross_notional: FixedDecimal,
    pub uncovered_gross_notional: FixedDecimal,
    pub uncovered_net_notional: FixedDecimal,
    pub whole_account_hedge_allowed: bool,
}

/// Complete L2 account snapshot matching `ficant.blotter.v1.AccountBlotterSnapshot`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnrichedAccountBlotterSnapshot {
    pub snapshot_seq: u64,
    pub account_id: String,
    pub account_version: u64,
    pub opening_base: FullOpeningPositionBase,
    pub risk_summary: RiskAndCoverageSummary,
    pub funding_summary: FundingGapSummary,
    pub feed_statuses: Vec<FeedStatusView>,
    pub positions: Vec<ValuedInstrumentPositionView>,
    pub active_earmarks: Vec<EarmarkRecord>,
    pub open_pending_matches: Vec<PendingMatchRecord>,
    pub pending_drafts: Vec<DraftEarmarkRecord>,
    pub allowed_hedge_product_codes: Vec<String>,
}

/// Atomic per-account replacement image carried inside a cross-account event envelope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountChange {
    pub account_id: String,
    pub replace_snapshot: EnrichedAccountBlotterSnapshot,
    pub changed_allocations: Vec<ReconcileAllocationRecord>,
}

/// Cross-account atomic WAL event envelope with a single global `seq`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlotterEventEnvelope {
    pub seq: u64,
    pub occurred_at: MarketTime,
    pub changes: Vec<AccountChange>,
    pub event_type: String,
}

/// Snapshot fallback directive returned when `after_seq < oldest_retained_seq - 1`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotFallbackRequired {
    pub requested_after_seq: u64,
    pub oldest_retained_seq: u64,
    pub current_head_seq: u64,
    pub reason: String,
}

/// Whole-transaction paginated response for `PollEvents`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PollEventsBatch {
    pub events: Vec<BlotterEventEnvelope>,
    pub scanned_through_seq: u64,
    pub current_head_seq: u64,
    pub has_more: bool,
}

/// Futures hedge measures returned by `PreviewCoveredHedge`, supporting zero-net-exposure books.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreviewCoveredHedgeMeasures {
    pub futures_contract_dv01: FixedDecimal,
    pub raw_contracts: FixedDecimal,
    pub recommended_contracts: i64,
    pub residual_dv01: FixedDecimal,
    pub hedge_effectiveness: FixedDecimal,
    pub contract_notional: FixedDecimal,
}

/// Input to `PreviewCoveredHedge`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewCoveredHedgeInput {
    pub account_id: String,
    pub scope: PreviewHedgeScope,
    pub product_code: String,
    pub contract_code: String,
    pub ctd_dv01_per_100: FixedDecimal,
    pub conversion_factor: FixedDecimal,
    pub delivery_rule_pack: MarketRulePack,
    pub valuation_at: Option<MarketTime>,
    pub expected_snapshot_seq: Option<u64>,
}

/// Output of `PreviewCoveredHedge`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewCoveredHedgeOutput {
    pub evaluated_at_seq: u64,
    pub scope: PreviewHedgeScope,
    pub product_code: String,
    pub contract_code: String,
    pub zero_net_exposure: bool,
    pub covered_tax_basis: BondTaxBasis,
    pub covered_instrument_count: u32,
    pub uncovered_instrument_count: u32,
    pub covered_net_pre_tax_dv01: FixedDecimal,
    pub excluded_uncovered_gross_notional: FixedDecimal,
    pub measures: PreviewCoveredHedgeMeasures,
}

/// Per-account L2 metadata (catalog, calendar, units, policy, full draft records).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountMetadataState {
    pub instruments: BTreeMap<String, InstrumentCatalogEntry>,
    pub calendar: Option<CalendarBinding>,
    pub default_calendar_requirement: Option<CalendarRequirement>,
    pub units: AnalysisUnits,
    pub policy: AccountDeskPolicy,
    pub drafts: BTreeMap<Ulid, DraftEarmarkRecord>,
}

/// Complete pure in-memory L2 workspace state combining the L1 domain `BlotterState`,
/// per-account metadata/policies/drafts, and the shared per-`(source_id, instrument)` quote store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeskBlotterWorkspaceState {
    pub domain: BlotterState,
    pub accounts_meta: BTreeMap<String, AccountMetadataState>,
    pub quotes: QuoteStoreState,
}

impl DeskBlotterWorkspaceState {
    pub fn new(config: BlotterConfig) -> ApplicationResult<Self> {
        let domain = BlotterState::new(config).map_err(|err| crate::map_blotter_error(&err))?;
        Ok(Self {
            domain,
            accounts_meta: BTreeMap::new(),
            quotes: QuoteStoreState::default(),
        })
    }
}

pub(crate) fn require_non_empty_text(value: &str) -> ApplicationResult<()> {
    if value.trim().is_empty() || value != value.trim() {
        return Err(invalid());
    }
    Ok(())
}

pub(crate) const fn invalid() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}

pub(crate) fn append_u8(buf: &mut Vec<u8>, value: u8) {
    buf.push(value);
}

pub(crate) fn append_u32(buf: &mut Vec<u8>, value: u32) {
    buf.extend_from_slice(&value.to_be_bytes());
}

pub(crate) fn append_u64(buf: &mut Vec<u8>, value: u64) {
    buf.extend_from_slice(&value.to_be_bytes());
}

pub(crate) fn append_fixed(buf: &mut Vec<u8>, value: FixedDecimal) {
    buf.extend_from_slice(&value.scaled().to_be_bytes());
}

pub(crate) fn append_opt_fixed(buf: &mut Vec<u8>, value: Option<FixedDecimal>) {
    match value {
        Some(v) => {
            append_u8(buf, 1);
            append_fixed(buf, v);
        }
        None => append_u8(buf, 0),
    }
}

pub(crate) fn append_bytes(buf: &mut Vec<u8>, value: &[u8]) {
    append_u64(buf, u64::try_from(value.len()).unwrap_or(u64::MAX));
    buf.extend_from_slice(value);
}

pub(crate) fn append_str(buf: &mut Vec<u8>, value: &str) {
    append_bytes(buf, value.as_bytes());
}

pub(crate) fn append_market_time(buf: &mut Vec<u8>, time: &MarketTime) {
    buf.extend_from_slice(&time.instant().timestamp_micros().to_be_bytes());
    append_str(buf, time.market_timezone());
    append_str(buf, &time.local_trading_date().to_string());
}

pub(crate) fn append_unit_ref(buf: &mut Vec<u8>, unit: &UnitRef) {
    append_str(buf, unit.unit_id().as_str());
    append_u64(buf, unit.version().get());
}

pub(crate) fn append_quote_terms(buf: &mut Vec<u8>, quote: &QuoteTerms) {
    match quote.basis {
        Some(QuoteBasis::CleanPricePer100(p)) => {
            append_u8(buf, 1);
            append_fixed(buf, p);
        }
        Some(QuoteBasis::YieldToMaturity(y)) => {
            append_u8(buf, 2);
            append_fixed(buf, y);
        }
        None => append_u8(buf, 0),
    }
    append_opt_fixed(buf, quote.settlement_amount);
}
