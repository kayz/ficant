use std::collections::{BTreeMap, BTreeSet};

use chrono::{NaiveDate, NaiveTime};

use crate::blotter::error::{BlotterError, BlotterResult};
use crate::blotter::wide_math::AllocationPool;
use crate::primitives::{ContentHash, FixedDecimal, MarketTime, Ulid};

/// Minimum allowed hold TTL in seconds (30 minutes).
pub const MIN_HOLD_TTL_SECONDS: u32 = 1_800;
/// Maximum allowed hold TTL in seconds (45 minutes).
pub const MAX_HOLD_TTL_SECONDS: u32 = 2_700;
/// Minimum allowed committed overdue alert threshold in seconds (60 minutes).
pub const MIN_COMMITTED_OVERDUE_SECONDS: u32 = 3_600;
/// Maximum allowed committed overdue alert threshold in seconds (90 minutes).
pub const MAX_COMMITTED_OVERDUE_SECONDS: u32 = 5_400;

/// Settlement bucket relative to account trading date.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SettlementBucket {
    TPlus0,
    TPlus1,
}

/// Trade direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TradeSide {
    Buy,
    Sell,
}

/// Lifecycle state of an earmark.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EarmarkState {
    Hold,
    Committed,
    Expired,
    Released,
    Cancelled,
    Reconciled,
}

/// Lifecycle state of a pending match record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PendingMatchState {
    Open,
    ResolvedMatched,
    ResolvedOrphanAccepted,
    Released,
    ReversedByFeed,
    ResolvedBaselineDelta,
}

/// Reason a trade fill entered or remains in the pending-match queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PendingMatchReason {
    AmbiguousCandidates,
    NotionalMismatchOrSplit,
    PriceOutOfTolerance,
    OrphanNoCommitment,
    BurstRepeat,
    ExistingPendingOnKey,
    AmendmentMismatch,
    BaselineRevisionManual,
    IncomparableBasis,
    RevisionHold,
    GapHold,
}

/// Manual resolution action for an open pending match.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PendingMatchResolutionAction {
    MatchFull,
    SplitOffset,
    AcceptOrphan,
    ReleaseReject,
    ApplyBaselineDelta,
}

/// Action carried by a raw trade feed record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TradeFeedRecordAction {
    NewFill,
    Amend,
    CancelReversal,
}

/// Precision classification of settlement cash for a trade or earmark.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FundingPrecision {
    ExactSettlementAmount,
    ExactDirtyPrice,
    CleanPriceEstimate,
    UnpricedYieldOnly,
}

/// Health state of a trade feed source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FeedHealthState {
    Healthy,
    Stale,
    GapDetected,
    PausedByOperator,
}

/// Outcome kind for a single occurrence in `IngestTradeFeed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TradeFillOutcomeKind {
    PerItemDeduplicated,
    PreWatermarkIgnored,
    AutoMatched,
    PendingMatch,
    BufferedGap,
    BufferedRevision,
    Reversed,
    BaselineReview,
    SupersededInBatch,
}

/// Quote basis for a trade or earmark.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QuoteBasis {
    CleanPricePer100(FixedDecimal),
    YieldToMaturity(FixedDecimal),
}

/// Quoted trade terms with optional explicit positive settlement cash amount.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuoteTerms {
    pub basis: Option<QuoteBasis>,
    pub settlement_amount: Option<FixedDecimal>,
}

impl QuoteTerms {
    pub fn validate(&self) -> BlotterResult<()> {
        if self.basis.is_none() && self.settlement_amount.is_none() {
            return Err(BlotterError::validation(
                "quote terms must specify clean price, yield, or settlement amount",
            ));
        }
        if let Some(basis) = self.basis {
            match basis {
                QuoteBasis::CleanPricePer100(price) => {
                    if !price.is_positive() {
                        return Err(BlotterError::validation(
                            "clean_price_per_100 must be positive",
                        ));
                    }
                }
                QuoteBasis::YieldToMaturity(ytm) => {
                    if !ytm.is_non_negative() {
                        return Err(BlotterError::validation(
                            "yield_to_maturity must be non-negative",
                        ));
                    }
                }
            }
        }
        if let Some(amount) = self.settlement_amount
            && !amount.is_positive()
        {
            return Err(BlotterError::validation(
                "explicit settlement_amount must be positive",
            ));
        }
        Ok(())
    }
}

/// Exact cash pricing input optionally supplied by the application/pricing layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExactDirtyPriceCashInput {
    pub dirty_price: FixedDecimal,
    pub face_value: FixedDecimal,
}

impl ExactDirtyPriceCashInput {
    pub fn validate(self) -> BlotterResult<()> {
        if !self.dirty_price.is_positive() || !self.face_value.is_positive() {
            return Err(BlotterError::validation(
                "dirty_price and face_value must be positive",
            ));
        }
        Ok(())
    }
}

/// Opening balance for a single instrument in an account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpeningInstrumentBalance {
    pub instrument_code: String,
    pub opening_economic_notional: FixedDecimal,
    pub opening_available_notional: FixedDecimal,
    pub opening_net_t0_notional: FixedDecimal,
    pub opening_net_t1_notional: FixedDecimal,
}

/// Proven evidence that a trade existed in the opening position base.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaseEvidence {
    pub source_id: String,
    pub external_trade_id: String,
    pub original_feed_seq: u64,
    pub original_executed_at: MarketTime,
    pub old_notional: FixedDecimal,
    pub old_signed_cash: FixedDecimal,
    pub settlement_bucket: SettlementBucket,
    pub evidence_ref: String,
    pub evidence_hash: ContentHash,
    pub instrument_code: String,
    pub side: TradeSide,
    pub original_revision: u64,
}

impl BaseEvidence {
    pub fn validate(&self) -> BlotterResult<()> {
        require_non_empty(&self.source_id, "source_id")?;
        require_non_empty(&self.external_trade_id, "external_trade_id")?;
        require_non_empty(&self.instrument_code, "instrument_code")?;
        require_non_empty(&self.evidence_ref, "evidence_ref")?;
        if !self.old_notional.is_positive() {
            return Err(BlotterError::validation(
                "baseline evidence old_notional must be positive",
            ));
        }
        if self.original_revision == 0 {
            return Err(BlotterError::validation(
                "baseline evidence original_revision must be >= 1",
            ));
        }
        Ok(())
    }

    /// Checks whether this evidence satisfies the account's watermark rule:
    /// when `watermark_source_seq` is present, requires `original_feed_seq <= watermark_source_seq`;
    /// otherwise requires `original_executed_at <= watermark_time`.
    pub fn satisfies_watermark(&self, base: &OpeningPositionBase) -> bool {
        if let Some(watermark_seq) = base.watermark_source_seq {
            self.original_feed_seq > 0 && self.original_feed_seq <= watermark_seq
        } else {
            self.original_executed_at.instant() <= base.watermark_time.instant()
        }
    }

    pub fn canonical_hash(&self) -> ContentHash {
        let mut bytes = Vec::new();
        append_str(&mut bytes, &self.source_id);
        append_str(&mut bytes, &self.external_trade_id);
        append_u64(&mut bytes, self.original_feed_seq);
        append_market_time(&mut bytes, &self.original_executed_at);
        append_fixed(&mut bytes, self.old_notional);
        append_fixed(&mut bytes, self.old_signed_cash);
        append_u8(&mut bytes, bucket_tag(self.settlement_bucket));
        append_str(&mut bytes, &self.evidence_ref);
        append_bytes(&mut bytes, self.evidence_hash.as_bytes());
        append_str(&mut bytes, &self.instrument_code);
        append_u8(&mut bytes, side_tag(self.side));
        append_u64(&mut bytes, self.original_revision);
        ContentHash::digest(&bytes)
    }
}

/// Immutable opening position base imported exactly once per account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpeningPositionBase {
    pub account_id: String,
    pub base_id: Ulid,
    pub trading_date: NaiveDate,
    pub next_settlement_date: NaiveDate,
    pub effective_from: MarketTime,
    pub watermark_time: MarketTime,
    pub watermark_source_seq: Option<u64>,
    pub source_id: String,
    pub opening_cash_t0: FixedDecimal,
    pub opening_net_cash_delta_t1: FixedDecimal,
    pub balances: Vec<OpeningInstrumentBalance>,
    pub instrument_codes: Vec<String>,
    pub baseline_evidence: Vec<BaseEvidence>,
    pub source_start_after_feed_seq: u64,
}

impl OpeningPositionBase {
    pub fn settlement_date_for_bucket(&self, bucket: SettlementBucket) -> NaiveDate {
        match bucket {
            SettlementBucket::TPlus0 => self.trading_date,
            SettlementBucket::TPlus1 => self.next_settlement_date,
        }
    }

    pub fn bucket_for_settlement_date(&self, date: NaiveDate) -> Option<SettlementBucket> {
        if date == self.trading_date {
            Some(SettlementBucket::TPlus0)
        } else if date == self.next_settlement_date {
            Some(SettlementBucket::TPlus1)
        } else {
            None
        }
    }

    pub fn canonical_hash(&self) -> ContentHash {
        let mut bytes = Vec::new();
        append_str(&mut bytes, &self.account_id);
        append_str(&mut bytes, self.base_id.as_str());
        append_str(&mut bytes, &self.trading_date.to_string());
        append_str(&mut bytes, &self.next_settlement_date.to_string());
        append_market_time(&mut bytes, &self.effective_from);
        append_market_time(&mut bytes, &self.watermark_time);
        match self.watermark_source_seq {
            Some(seq) => {
                append_u8(&mut bytes, 1);
                append_u64(&mut bytes, seq);
            }
            None => append_u8(&mut bytes, 0),
        }
        append_str(&mut bytes, &self.source_id);
        append_fixed(&mut bytes, self.opening_cash_t0);
        append_fixed(&mut bytes, self.opening_net_cash_delta_t1);
        append_u64(&mut bytes, self.source_start_after_feed_seq);

        let mut sorted_balances = self.balances.clone();
        sorted_balances.sort_by(|a, b| a.instrument_code.cmp(&b.instrument_code));
        append_u64(
            &mut bytes,
            u64::try_from(sorted_balances.len()).unwrap_or(u64::MAX),
        );
        for bal in &sorted_balances {
            append_str(&mut bytes, &bal.instrument_code);
            append_fixed(&mut bytes, bal.opening_economic_notional);
            append_fixed(&mut bytes, bal.opening_available_notional);
            append_fixed(&mut bytes, bal.opening_net_t0_notional);
            append_fixed(&mut bytes, bal.opening_net_t1_notional);
        }

        let mut sorted_instruments = self.instrument_codes.clone();
        sorted_instruments.sort();
        append_u64(
            &mut bytes,
            u64::try_from(sorted_instruments.len()).unwrap_or(u64::MAX),
        );
        for code in &sorted_instruments {
            append_str(&mut bytes, code);
        }

        let mut sorted_evidence = self.baseline_evidence.clone();
        sorted_evidence.sort_by(|a, b| {
            (&a.source_id, &a.external_trade_id, a.original_revision).cmp(&(
                &b.source_id,
                &b.external_trade_id,
                b.original_revision,
            ))
        });
        append_u64(
            &mut bytes,
            u64::try_from(sorted_evidence.len()).unwrap_or(u64::MAX),
        );
        for ev in &sorted_evidence {
            append_bytes(&mut bytes, ev.canonical_hash().as_bytes());
        }

        ContentHash::digest(&bytes)
    }
}

/// Signed notional + signed cash adjustment relative to a proven baseline trade.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaselineDelta {
    /// Signed notional delta in the original trade's direction (`new_notional - old_notional`).
    pub delta_notional: FixedDecimal,
    /// Signed account cash delta (`new_signed_cash - old_signed_cash`).
    pub delta_cash: FixedDecimal,
    pub settlement_bucket: SettlementBucket,
    pub evidence: BaseEvidence,
    pub reason: String,
}

/// Four-field matching key `K = (account_id, instrument_code, side, settlement_date)`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MatchKey {
    pub account_id: String,
    pub instrument_code: String,
    pub side: TradeSide,
    pub settlement_date: NaiveDate,
}

/// Inclusive range of missing `feed_seq` values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SeqRange {
    pub first: u64,
    pub last: u64,
}

/// Per-bucket balance projection for an instrument.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BucketBalanceView {
    pub settlement_bucket: SettlementBucket,
    pub settlement_date: NaiveDate,
    pub available_notional: FixedDecimal,
    pub hold_buy_notional: FixedDecimal,
    pub hold_sell_notional: FixedDecimal,
    pub committed_buy_unreconciled_notional: FixedDecimal,
    pub committed_sell_unreconciled_notional: FixedDecimal,
    pub reconciled_buy_notional: FixedDecimal,
    pub reconciled_sell_notional: FixedDecimal,
    pub pending_match_buy_unresolved_notional: FixedDecimal,
    pub pending_match_sell_unresolved_notional: FixedDecimal,
    pub sell_ambiguity_locked: bool,
    pub negative_available_alert: bool,
}

/// Projected instrument position view across T+0 and T+1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstrumentPositionView {
    pub account_id: String,
    pub instrument_code: String,
    pub position_version: u64,
    pub economic_net_notional: FixedDecimal,
    pub available_t0_notional: FixedDecimal,
    pub available_t1_notional: FixedDecimal,
    pub bucket_t0: BucketBalanceView,
    pub bucket_t1: BucketBalanceView,
}

/// Full earmark record with independent allocation pool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EarmarkRecord {
    pub earmark_id: Ulid,
    pub earmark_revision: u64,
    pub account_id: String,
    pub instrument_code: String,
    pub side: TradeSide,
    pub settlement_bucket: SettlementBucket,
    pub settlement_date: NaiveDate,
    pub notional: FixedDecimal,
    pub unreconciled_notional: FixedDecimal,
    pub quote_terms: Option<QuoteTerms>,
    pub locked_settlement_cash: Option<FixedDecimal>,
    pub unreconciled_settlement_cash: Option<FixedDecimal>,
    pub funding_precision: FundingPrecision,
    pub state: EarmarkState,
    pub created_by: String,
    pub created_at: MarketTime,
    pub expires_at: Option<MarketTime>,
    pub committed_at: Option<MarketTime>,
    pub overdue_alert: bool,
    pub pre_close_alert: bool,
    pub reopened_by_reversal: bool,
    pub note: String,
    pub created_seq: u64,
    pub updated_seq: u64,
    pub confirmed_by_trader_id: Option<String>,
    pub cancel_actor_id: Option<String>,
    pub cancel_reason: Option<String>,
    pub allocation_pool: AllocationPool,
}

/// Allocation linking a trade fill revision to an earmark, orphan acceptance, or baseline delta.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconcileAllocationRecord {
    pub allocation_id: Ulid,
    pub account_id: String,
    pub instrument_code: String,
    pub side: TradeSide,
    pub settlement_bucket: SettlementBucket,
    pub source_id: String,
    pub external_trade_id: String,
    pub source_revision: u64,
    pub earmark_id: Option<Ulid>,
    pub pending_match_id: Option<Ulid>,
    pub allocated_notional: FixedDecimal,
    pub allocated_fill_cash: Option<FixedDecimal>,
    pub fill_funding_precision: FundingPrecision,
    pub fill_segment_index: Option<u32>,
    pub earmark_segment_index: Option<u32>,
    pub reversed_by_revision: Option<u64>,
    pub released_earmark_cash: Option<FixedDecimal>,
    pub baseline_delta: Option<BaselineDelta>,
    pub actor_id: String,
}

/// Pending match record awaiting manual resolution or feed recovery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingMatchRecord {
    pub pending_match_id: Ulid,
    pub pending_revision: u64,
    pub account_id: String,
    pub instrument_code: String,
    pub side: TradeSide,
    pub settlement_bucket: SettlementBucket,
    pub settlement_date: NaiveDate,
    pub source_id: String,
    pub external_trade_id: String,
    pub source_revision: u64,
    pub original_notional: FixedDecimal,
    pub unresolved_notional: FixedDecimal,
    pub prior_baseline_notional: Option<FixedDecimal>,
    pub quote_terms: Option<QuoteTerms>,
    pub executed_at: MarketTime,
    pub state: PendingMatchState,
    pub reason: PendingMatchReason,
    pub candidate_earmark_ids: Vec<Ulid>,
    pub created_seq: u64,
    pub updated_seq: u64,
    pub unresolved_signed_cash: Option<FixedDecimal>,
    pub funding_precision: FundingPrecision,
    pub evidence: Option<BaseEvidence>,
    pub applied_delta: Option<BaselineDelta>,
    pub resolution_actor_id: Option<String>,
    pub resolution_reason: Option<String>,
    pub cross_key_or_bucket_anomaly: bool,
    pub allocation_pool: AllocationPool,
}

/// Funding gap & pending official cash summary (16 fields matching `interfaces.md`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FundingGapSummary {
    pub opening_cash_t0: FixedDecimal,
    pub opening_net_cash_delta_t1: FixedDecimal,
    pub net_settlement_flow_t0: FixedDecimal,
    pub net_settlement_flow_t1: FixedDecimal,
    pub projected_cash_t0: FixedDecimal,
    pub projected_cash_t1: FixedDecimal,
    pub funding_gap_t0: FixedDecimal,
    pub funding_gap_t1: FixedDecimal,
    pub clean_price_estimated_funding_count: u32,
    pub unpriced_funding_earmark_count: u32,
    pub funding_is_complete_exact: bool,
    pub pending_match_cash_t0: FixedDecimal,
    pub pending_match_cash_t1: FixedDecimal,
    pub pending_match_count: u32,
    pub pending_unknown_cash_count: u32,
    pub feed_incomplete: bool,
}

/// Projected view of a trade feed source's health, cursors, and pause bits.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedStatusView {
    pub source_id: String,
    pub feed_status_revision: u64,
    pub health_state: FeedHealthState,
    pub earmarks_paused: bool,
    pub paused_by_gap: bool,
    pub paused_by_staleness: bool,
    pub paused_by_operator: bool,
    pub last_heartbeat_at: Option<MarketTime>,
    pub contiguous_feed_seq: u64,
    pub max_seen_feed_seq: u64,
    pub missing_feed_ranges: Vec<SeqRange>,
    pub open_pending_match_count: u32,
    pub status_reason: String,
}

/// Incoming trade fill record from a feed adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TradeFillRecord {
    pub source_id: String,
    pub external_trade_id: String,
    pub source_revision: u64,
    pub feed_seq: u64,
    pub action: TradeFeedRecordAction,
    pub account_id: String,
    pub instrument_code: String,
    pub side: TradeSide,
    pub settlement_date: NaiveDate,
    pub notional: FixedDecimal,
    pub quote_terms: Option<QuoteTerms>,
    pub executed_at: MarketTime,
    pub batch_first_feed_seq: u64,
    pub batch_last_feed_seq: u64,
    /// Optional pre-computed exact dirty-price input supplied by the application/pricing layer.
    pub exact_dirty_price_input: Option<ExactDirtyPriceCashInput>,
}

impl TradeFillRecord {
    /// Canonical hash of the full raw packet (including `feed_seq` and source batch bounds).
    pub fn payload_hash(&self) -> ContentHash {
        let mut bytes = Vec::new();
        append_u64(&mut bytes, self.feed_seq);
        append_u64(&mut bytes, self.batch_first_feed_seq);
        append_u64(&mut bytes, self.batch_last_feed_seq);
        append_bytes(&mut bytes, self.business_hash().as_bytes());
        ContentHash::digest(&bytes)
    }

    /// Canonical hash of the business payload of `(source_id, external_trade_id, source_revision)`
    /// independent of transport `feed_seq` and source batch bounds.
    pub fn business_hash(&self) -> ContentHash {
        let mut bytes = Vec::new();
        append_str(&mut bytes, &self.source_id);
        append_str(&mut bytes, &self.external_trade_id);
        append_u64(&mut bytes, self.source_revision);
        append_u8(&mut bytes, action_tag(self.action));
        append_str(&mut bytes, &self.account_id);
        append_str(&mut bytes, &self.instrument_code);
        append_u8(&mut bytes, side_tag(self.side));
        append_str(&mut bytes, &self.settlement_date.to_string());
        append_fixed(&mut bytes, self.notional);
        append_quote_terms(&mut bytes, self.quote_terms.as_ref());
        append_market_time(&mut bytes, &self.executed_at);
        match self.exact_dirty_price_input {
            Some(input) => {
                append_u8(&mut bytes, 1);
                append_fixed(&mut bytes, input.dirty_price);
                append_fixed(&mut bytes, input.face_value);
            }
            None => append_u8(&mut bytes, 0),
        }
        ContentHash::digest(&bytes)
    }
}

/// Per-item outcome returned by `IngestTradeFeed`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TradeFillIngestOutcome {
    pub source_id: String,
    pub external_trade_id: String,
    pub source_revision: u64,
    pub feed_seq: u64,
    pub kind: TradeFillOutcomeKind,
    pub pending_match_id: Option<Ulid>,
    pub allocation_ids: Vec<Ulid>,
    pub effective_revision: Option<u64>,
}

/// Stored raw feed packet keyed by `(source_id, feed_seq)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredRawPacket {
    pub feed_seq: u64,
    pub external_trade_id: String,
    pub source_revision: u64,
    pub payload_hash: ContentHash,
    pub business_hash: ContentHash,
    pub batch_first_feed_seq: u64,
    pub batch_last_feed_seq: u64,
    pub record: TradeFillRecord,
}

/// Processing status of a stored `(external_trade_id, source_revision)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TradeRevisionLifecycle {
    BufferedGap,
    RevisionHold,
    PreWatermarkIgnored,
    SupersededInBatch,
    Applied,
    BaselineReview,
    Reversed,
}

/// Stored canonical trade revision keyed by `(external_trade_id, source_revision)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredTradeRevision {
    pub business_hash: ContentHash,
    pub canonical_feed_seq: u64,
    pub alias_feed_seqs: Vec<u64>,
    pub record: TradeFillRecord,
    pub lifecycle: TradeRevisionLifecycle,
}

/// Per-trade revision chain tracking inside a source feed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TradeChainState {
    pub latest_effective_revision: Option<u64>,
    pub original_new_fill: Option<TradeFillRecord>,
    pub baseline_original_evidence: Option<BaseEvidence>,
    pub held_revisions: BTreeSet<u64>,
    pub missing_predecessor_revisions: BTreeSet<u64>,
}

/// Registered source batch boundaries `[first, last]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceBatchRecord {
    pub first_feed_seq: u64,
    pub last_feed_seq: u64,
    pub released: bool,
}

/// Internal state of a single trade feed source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceFeedState {
    pub source_id: String,
    pub feed_status_revision: u64,
    pub initial_after_feed_seq: u64,
    pub contiguous_feed_seq: u64,
    pub max_seen_feed_seq: u64,
    pub missing_feed_ranges: Vec<SeqRange>,
    pub paused_by_gap: bool,
    pub paused_by_staleness: bool,
    pub paused_by_operator: bool,
    pub last_heartbeat_at: Option<MarketTime>,
    pub status_reason: String,
    pub raw_packets: BTreeMap<u64, StoredRawPacket>,
    pub trade_revisions: BTreeMap<(String, u64), StoredTradeRevision>,
    pub trade_chains: BTreeMap<String, TradeChainState>,
    pub source_batches: BTreeMap<(u64, u64), SourceBatchRecord>,
}

impl SourceFeedState {
    pub fn new(source_id: String, start_after_feed_seq: u64) -> Self {
        Self {
            source_id,
            feed_status_revision: 1,
            initial_after_feed_seq: start_after_feed_seq,
            contiguous_feed_seq: start_after_feed_seq,
            max_seen_feed_seq: start_after_feed_seq,
            missing_feed_ranges: Vec::new(),
            paused_by_gap: false,
            paused_by_staleness: false,
            paused_by_operator: false,
            last_heartbeat_at: None,
            status_reason: String::new(),
            raw_packets: BTreeMap::new(),
            trade_revisions: BTreeMap::new(),
            trade_chains: BTreeMap::new(),
            source_batches: BTreeMap::new(),
        }
    }

    pub fn earmarks_paused(&self) -> bool {
        self.paused_by_gap || self.paused_by_staleness || self.paused_by_operator
    }

    pub fn health_state(&self) -> FeedHealthState {
        if self.paused_by_operator {
            FeedHealthState::PausedByOperator
        } else if !self.missing_feed_ranges.is_empty() || self.paused_by_gap {
            FeedHealthState::GapDetected
        } else if self.paused_by_staleness {
            FeedHealthState::Stale
        } else {
            FeedHealthState::Healthy
        }
    }

    pub fn has_incomplete_batches_or_revision_holds(&self) -> bool {
        !self.missing_feed_ranges.is_empty()
            || self.source_batches.values().any(|batch| !batch.released)
            || self
                .trade_chains
                .values()
                .any(|chain| !chain.held_revisions.is_empty())
    }

    pub fn view(&self, open_pending_match_count: u32) -> FeedStatusView {
        FeedStatusView {
            source_id: self.source_id.clone(),
            feed_status_revision: self.feed_status_revision,
            health_state: self.health_state(),
            earmarks_paused: self.earmarks_paused(),
            paused_by_gap: self.paused_by_gap,
            paused_by_staleness: self.paused_by_staleness,
            paused_by_operator: self.paused_by_operator,
            last_heartbeat_at: self.last_heartbeat_at.clone(),
            contiguous_feed_seq: self.contiguous_feed_seq,
            max_seen_feed_seq: self.max_seen_feed_seq,
            missing_feed_ranges: self.missing_feed_ranges.clone(),
            open_pending_match_count,
            status_reason: self.status_reason.clone(),
        }
    }
}

/// Minimal draft marker held in L1 account state (full rule/DV01 preview lives in L2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftPresenceRecord {
    pub draft_id: Ulid,
    pub instrument_code: String,
    pub created_seq: u64,
}

/// Per-instrument mutable ledger state inside an account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstrumentPositionState {
    pub instrument_code: String,
    pub position_version: u64,
    pub opening_economic_notional: FixedDecimal,
    pub opening_available_notional: FixedDecimal,
    pub opening_net_t0_notional: FixedDecimal,
    pub opening_net_t1_notional: FixedDecimal,
    pub reconciled_buy_t0: FixedDecimal,
    pub reconciled_buy_t1: FixedDecimal,
    pub reconciled_sell_t0: FixedDecimal,
    pub reconciled_sell_t1: FixedDecimal,
}

impl InstrumentPositionState {
    pub fn from_opening(balance: &OpeningInstrumentBalance) -> Self {
        Self {
            instrument_code: balance.instrument_code.clone(),
            position_version: 1,
            opening_economic_notional: balance.opening_economic_notional,
            opening_available_notional: balance.opening_available_notional,
            opening_net_t0_notional: balance.opening_net_t0_notional,
            opening_net_t1_notional: balance.opening_net_t1_notional,
            reconciled_buy_t0: FixedDecimal::ZERO,
            reconciled_buy_t1: FixedDecimal::ZERO,
            reconciled_sell_t0: FixedDecimal::ZERO,
            reconciled_sell_t1: FixedDecimal::ZERO,
        }
    }

    pub fn zero(instrument_code: String) -> Self {
        Self {
            instrument_code,
            position_version: 1,
            opening_economic_notional: FixedDecimal::ZERO,
            opening_available_notional: FixedDecimal::ZERO,
            opening_net_t0_notional: FixedDecimal::ZERO,
            opening_net_t1_notional: FixedDecimal::ZERO,
            reconciled_buy_t0: FixedDecimal::ZERO,
            reconciled_buy_t1: FixedDecimal::ZERO,
            reconciled_sell_t0: FixedDecimal::ZERO,
            reconciled_sell_t1: FixedDecimal::ZERO,
        }
    }
}

/// Recorded receipt for the account's single `ImportOpeningBase`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpeningImportReceipt {
    pub actor_id: String,
    pub idempotency_key: String,
    pub request_hash: ContentHash,
    pub committed_seq: u64,
}

/// Complete domain state for a single account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountState {
    pub account_id: String,
    pub account_version: u64,
    pub opening_base: OpeningPositionBase,
    pub import_receipt: OpeningImportReceipt,
    pub positions: BTreeMap<String, InstrumentPositionState>,
    pub earmarks: BTreeMap<Ulid, EarmarkRecord>,
    pub pending_matches: BTreeMap<Ulid, PendingMatchRecord>,
    pub allocations: BTreeMap<Ulid, ReconcileAllocationRecord>,
    pub drafts: BTreeMap<Ulid, DraftPresenceRecord>,
}

/// Immutable snapshot of one account at a specific `snapshot_seq`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountBlotterSnapshot {
    pub snapshot_seq: u64,
    pub account_id: String,
    pub account_version: u64,
    pub opening_base: OpeningPositionBase,
    pub funding_summary: FundingGapSummary,
    pub feed_status: FeedStatusView,
    pub positions: Vec<InstrumentPositionView>,
    pub active_earmarks: Vec<EarmarkRecord>,
    pub open_pending_matches: Vec<PendingMatchRecord>,
}

/// Pure policy knobs supplied to the synchronous domain engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlotterConfig {
    /// Allowed clean price difference for automatic 1:1 matching (in price per 100 units).
    pub clean_price_tolerance: FixedDecimal,
    /// Allowed yield-to-maturity difference for automatic 1:1 matching (in decimal yield units).
    pub yield_tolerance: FixedDecimal,
    /// Overdue alert threshold for active `COMMITTED` earmarks (`3600..=5400` seconds).
    pub committed_overdue_seconds: u32,
    /// Local market time at or after which active `COMMITTED` earmarks trigger `pre_close_alert`.
    pub pre_close_alert_time: NaiveTime,
    /// Optional feed staleness threshold in seconds (`None` disables automatic staleness pause).
    pub feed_staleness_seconds: Option<u32>,
}

impl BlotterConfig {
    pub fn validate(&self) -> BlotterResult<()> {
        if !self.clean_price_tolerance.is_non_negative() || !self.yield_tolerance.is_non_negative()
        {
            return Err(BlotterError::validation(
                "matching tolerances must be non-negative",
            ));
        }
        if !(MIN_COMMITTED_OVERDUE_SECONDS..=MAX_COMMITTED_OVERDUE_SECONDS)
            .contains(&self.committed_overdue_seconds)
        {
            return Err(BlotterError::validation(
                "committed_overdue_seconds must be within 3600..=5400",
            ));
        }
        if let Some(secs) = self.feed_staleness_seconds
            && secs == 0
        {
            return Err(BlotterError::validation(
                "feed_staleness_seconds must be positive when configured",
            ));
        }
        Ok(())
    }
}

impl Default for BlotterConfig {
    fn default() -> Self {
        Self {
            // Default tolerances: 0.05 clean price per 100, 0.0005 yield (5 bp)
            clean_price_tolerance: FixedDecimal::from_scaled(50_000_000_000),
            yield_tolerance: FixedDecimal::from_scaled(500_000_000),
            committed_overdue_seconds: MIN_COMMITTED_OVERDUE_SECONDS,
            pre_close_alert_time: NaiveTime::from_hms_opt(16, 30, 0)
                .expect("16:30:00 is a valid NaiveTime"),
            feed_staleness_seconds: Some(300),
        }
    }
}

/// Deterministic ULID generator interface for pure synchronous state transitions.
pub trait BlotterIdGenerator {
    fn next_ulid(&mut self) -> Ulid;
}

/// Pure in-memory deterministic ULID generator (zero randomness, zero wall-clock I/O).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeterministicIdGenerator {
    timestamp_ms: u64,
    counter: u128,
}

impl DeterministicIdGenerator {
    pub const fn new(timestamp_ms: u64, initial_counter: u128) -> Self {
        Self {
            timestamp_ms,
            counter: initial_counter,
        }
    }
}

impl Default for DeterministicIdGenerator {
    fn default() -> Self {
        Self::new(1_700_000_000_000, 1)
    }
}

impl BlotterIdGenerator for DeterministicIdGenerator {
    fn next_ulid(&mut self) -> Ulid {
        let id = ulid::Ulid::from_parts(self.timestamp_ms, self.counter);
        self.counter = self.counter.saturating_add(1);
        Ulid::new(id.to_string()).expect("generated ulid must be valid canonical ULID")
    }
}

pub(crate) fn require_non_empty(value: &str, field: &str) -> BlotterResult<()> {
    if value.trim().is_empty() || value != value.trim() {
        return Err(BlotterError::validation(format!(
            "{field} must be non-empty without leading/trailing whitespace"
        )));
    }
    Ok(())
}

pub(crate) fn bucket_tag(bucket: SettlementBucket) -> u8 {
    match bucket {
        SettlementBucket::TPlus0 => 1,
        SettlementBucket::TPlus1 => 2,
    }
}

pub(crate) fn side_tag(side: TradeSide) -> u8 {
    match side {
        TradeSide::Buy => 1,
        TradeSide::Sell => 2,
    }
}

pub(crate) fn action_tag(action: TradeFeedRecordAction) -> u8 {
    match action {
        TradeFeedRecordAction::NewFill => 1,
        TradeFeedRecordAction::Amend => 2,
        TradeFeedRecordAction::CancelReversal => 3,
    }
}

pub(crate) fn append_u8(buf: &mut Vec<u8>, value: u8) {
    buf.push(value);
}

pub(crate) fn append_u64(buf: &mut Vec<u8>, value: u64) {
    buf.extend_from_slice(&value.to_be_bytes());
}

pub(crate) fn append_fixed(buf: &mut Vec<u8>, value: FixedDecimal) {
    buf.extend_from_slice(&value.scaled().to_be_bytes());
}

pub(crate) fn append_bytes(buf: &mut Vec<u8>, value: &[u8]) {
    buf.extend_from_slice(&u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
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

pub(crate) fn append_quote_terms(buf: &mut Vec<u8>, quote: Option<&QuoteTerms>) {
    match quote {
        Some(q) => {
            append_u8(buf, 1);
            match q.basis {
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
            match q.settlement_amount {
                Some(a) => {
                    append_u8(buf, 1);
                    append_fixed(buf, a);
                }
                None => append_u8(buf, 0),
            }
        }
        None => append_u8(buf, 0),
    }
}
