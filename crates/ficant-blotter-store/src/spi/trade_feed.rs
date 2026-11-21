use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

use ficant_application::ports::{
    ApplicationResult, TradeFeedReconcilerPort, validate_spi_page_size,
};
use ficant_application::use_cases::blotter::PullTradeFeedBatchResult;
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_domain::blotter::{
    QuoteBasis, QuoteTerms, TradeFeedRecordAction, TradeFillRecord, TradeSide,
};

use crate::spi::csv_rfc4180::{
    CsvTable, parse_csv_date, parse_csv_fixed, parse_csv_market_time, parse_csv_non_empty,
    parse_csv_u64,
};

#[derive(Clone, Debug, Default)]
struct TradeFeedSourceData {
    by_feed_seq: BTreeMap<u64, TradeFillRecord>,
    by_trade_rev: BTreeMap<(String, u64), TradeFillRecord>,
}

/// In-memory open-source reference adapter for `TradeFeedReconcilerPort`.
#[derive(Debug, Default)]
pub struct MockTradeFeedReconciler {
    sources: Mutex<BTreeMap<String, TradeFeedSourceData>>,
}

impl MockTradeFeedReconciler {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends or indexes one authoritative trade fill record into both the sequence stream and
    /// historical `(external_trade_id, source_revision)` lookup table.
    pub fn push_fill(&self, fill: TradeFillRecord) -> ApplicationResult<()> {
        validate_fill_record(&fill)?;
        let mut guard = self
            .sources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let data = guard.entry(fill.source_id.clone()).or_default();
        insert_fill_checked(data, fill)
    }

    /// Registers a historical trade revision only in the `FetchTradeRevision` archive (e.g. for
    /// pre-watermark evidence or missing predecessor revision backfill tests).
    pub fn insert_historical_revision(&self, fill: TradeFillRecord) -> ApplicationResult<()> {
        validate_fill_record(&fill)?;
        let mut guard = self
            .sources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let data = guard.entry(fill.source_id.clone()).or_default();
        let key = (fill.external_trade_id.clone(), fill.source_revision);
        if let Some(existing) = data.by_trade_rev.get(&key)
            && existing.business_hash() != fill.business_hash()
        {
            return Err(ApplicationError::new(
                ApplicationErrorCategory::HashMismatch,
                false,
            ));
        }
        data.by_trade_rev.insert(key, fill);
        Ok(())
    }
}

impl TradeFeedReconcilerPort for MockTradeFeedReconciler {
    fn pull_trade_feed_batch(
        &self,
        source_id: &str,
        after_feed_seq: u64,
        max_records: u32,
    ) -> ApplicationResult<PullTradeFeedBatchResult> {
        parse_csv_non_empty(source_id)?;
        validate_spi_page_size(max_records)?;
        let guard = self
            .sources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(pull_contiguous_page(
            guard.get(source_id),
            after_feed_seq,
            max_records,
        ))
    }

    fn fetch_trade_revision(
        &self,
        source_id: &str,
        external_trade_id: &str,
        source_revision: u64,
    ) -> ApplicationResult<TradeFillRecord> {
        parse_csv_non_empty(source_id)?;
        parse_csv_non_empty(external_trade_id)?;
        if source_revision == 0 {
            return Err(validation_failed());
        }
        let guard = self
            .sources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard
            .get(source_id)
            .and_then(|src| {
                src.by_trade_rev
                    .get(&(external_trade_id.to_owned(), source_revision))
            })
            .cloned()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))
    }
}

/// RFC-4180 CSV open-source reference adapter for `TradeFeedReconcilerPort`.
///
/// Expected CSV headers:
/// `source_id,external_trade_id,source_revision,feed_seq,action,account_id,instrument_code,side,settlement_date,notional,clean_price_per_100,yield_to_maturity,settlement_amount,executed_at_rfc3339,market_timezone,batch_first_feed_seq,batch_last_feed_seq`
#[derive(Clone, Debug)]
pub struct CsvTradeFeedReconciler {
    sources: BTreeMap<String, TradeFeedSourceData>,
}

impl CsvTradeFeedReconciler {
    pub fn from_csv_str(csv_text: &str) -> ApplicationResult<Self> {
        let table = CsvTable::parse(csv_text)?;
        table.require_headers(&[
            "source_id",
            "external_trade_id",
            "source_revision",
            "feed_seq",
            "action",
            "account_id",
            "instrument_code",
            "side",
            "settlement_date",
            "notional",
            "clean_price_per_100",
            "yield_to_maturity",
            "settlement_amount",
            "executed_at_rfc3339",
            "market_timezone",
            "batch_first_feed_seq",
            "batch_last_feed_seq",
        ])?;

        let mut sources: BTreeMap<String, TradeFeedSourceData> = BTreeMap::new();
        for row in table.rows() {
            let source_id = parse_csv_non_empty(table.field(row, "source_id")?)?;
            let external_trade_id = parse_csv_non_empty(table.field(row, "external_trade_id")?)?;
            let source_revision = parse_csv_u64(table.field(row, "source_revision")?)?;
            let feed_seq = parse_csv_u64(table.field(row, "feed_seq")?)?;
            let action = parse_csv_action(table.field(row, "action")?)?;
            let account_id = parse_csv_non_empty(table.field(row, "account_id")?)?;
            let instrument_code = parse_csv_non_empty(table.field(row, "instrument_code")?)?;
            let side = parse_csv_side(table.field(row, "side")?)?;
            let settlement_date = parse_csv_date(table.field(row, "settlement_date")?)?;
            let notional = parse_csv_fixed(table.field(row, "notional")?)?;

            let clean_opt = table
                .optional_field(row, "clean_price_per_100")
                .map(parse_csv_fixed)
                .transpose()?;
            let ytm_opt = table
                .optional_field(row, "yield_to_maturity")
                .map(parse_csv_fixed)
                .transpose()?;
            let settle_opt = table
                .optional_field(row, "settlement_amount")
                .map(parse_csv_fixed)
                .transpose()?;

            let basis = match (clean_opt, ytm_opt) {
                (Some(_), Some(_)) => return Err(validation_failed()),
                (Some(c), None) => Some(QuoteBasis::CleanPricePer100(c)),
                (None, Some(y)) => Some(QuoteBasis::YieldToMaturity(y)),
                (None, None) => None,
            };
            let quote_terms = if basis.is_some() || settle_opt.is_some() {
                let qt = QuoteTerms {
                    basis,
                    settlement_amount: settle_opt,
                };
                qt.validate().map_err(|_| validation_failed())?;
                Some(qt)
            } else {
                None
            };

            let market_tz = parse_csv_non_empty(table.field(row, "market_timezone")?)?;
            let executed_at =
                parse_csv_market_time(table.field(row, "executed_at_rfc3339")?, &market_tz)?;
            let batch_first_feed_seq = parse_csv_u64(table.field(row, "batch_first_feed_seq")?)?;
            let batch_last_feed_seq = parse_csv_u64(table.field(row, "batch_last_feed_seq")?)?;

            let fill = TradeFillRecord {
                source_id: source_id.clone(),
                external_trade_id,
                source_revision,
                feed_seq,
                action,
                account_id,
                instrument_code,
                side,
                settlement_date,
                notional,
                quote_terms,
                executed_at,
                batch_first_feed_seq,
                batch_last_feed_seq,
                exact_dirty_price_input: None,
            };
            validate_fill_record(&fill)?;
            let data = sources.entry(source_id).or_default();
            insert_fill_checked(data, fill)?;
        }

        Ok(Self { sources })
    }

    pub fn from_csv_file(path: impl AsRef<Path>) -> ApplicationResult<Self> {
        let raw = fs::read_to_string(path).map_err(|_| validation_failed())?;
        Self::from_csv_str(&raw)
    }
}

impl TradeFeedReconcilerPort for CsvTradeFeedReconciler {
    fn pull_trade_feed_batch(
        &self,
        source_id: &str,
        after_feed_seq: u64,
        max_records: u32,
    ) -> ApplicationResult<PullTradeFeedBatchResult> {
        parse_csv_non_empty(source_id)?;
        validate_spi_page_size(max_records)?;
        Ok(pull_contiguous_page(
            self.sources.get(source_id),
            after_feed_seq,
            max_records,
        ))
    }

    fn fetch_trade_revision(
        &self,
        source_id: &str,
        external_trade_id: &str,
        source_revision: u64,
    ) -> ApplicationResult<TradeFillRecord> {
        parse_csv_non_empty(source_id)?;
        parse_csv_non_empty(external_trade_id)?;
        if source_revision == 0 {
            return Err(validation_failed());
        }
        self.sources
            .get(source_id)
            .and_then(|src| {
                src.by_trade_rev
                    .get(&(external_trade_id.to_owned(), source_revision))
            })
            .cloned()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))
    }
}

/// Pulls ONLY the contiguous deliverable prefix starting at `after_feed_seq + 1`.
///
/// Stops immediately before any missing sequence gap so `scanned_through_feed_seq` never skips
/// over a hole. On an empty page, `scanned_through_feed_seq` stays equal to `after_feed_seq`.
fn pull_contiguous_page(
    source: Option<&TradeFeedSourceData>,
    after_feed_seq: u64,
    max_records: u32,
) -> PullTradeFeedBatchResult {
    let Some(data) = source else {
        return PullTradeFeedBatchResult {
            fills: Vec::new(),
            scanned_through_feed_seq: after_feed_seq,
            has_more: false,
        };
    };

    let limit = usize::try_from(max_records).unwrap_or(usize::MAX);
    let mut fills = Vec::new();
    let mut expected_seq = after_feed_seq.saturating_add(1);
    let mut scanned = after_feed_seq;

    while fills.len() < limit {
        let Some(rec) = data.by_feed_seq.get(&expected_seq) else {
            break;
        };
        fills.push(rec.clone());
        scanned = expected_seq;
        expected_seq = expected_seq.saturating_add(1);
    }

    let has_more = data.by_feed_seq.contains_key(&expected_seq);
    PullTradeFeedBatchResult {
        fills,
        scanned_through_feed_seq: scanned,
        has_more,
    }
}

fn validate_fill_record(fill: &TradeFillRecord) -> ApplicationResult<()> {
    parse_csv_non_empty(&fill.source_id)?;
    parse_csv_non_empty(&fill.external_trade_id)?;
    parse_csv_non_empty(&fill.account_id)?;
    parse_csv_non_empty(&fill.instrument_code)?;
    if fill.source_revision == 0 || fill.feed_seq == 0 {
        return Err(validation_failed());
    }
    if fill.batch_first_feed_seq == 0
        || fill.batch_first_feed_seq > fill.batch_last_feed_seq
        || fill.feed_seq < fill.batch_first_feed_seq
        || fill.feed_seq > fill.batch_last_feed_seq
    {
        return Err(validation_failed());
    }
    if fill.action != TradeFeedRecordAction::CancelReversal && !fill.notional.is_positive() {
        return Err(validation_failed());
    }
    if fill.action == TradeFeedRecordAction::CancelReversal && !fill.notional.is_non_negative() {
        return Err(validation_failed());
    }
    if let Some(qt) = &fill.quote_terms {
        qt.validate().map_err(|_| validation_failed())?;
    }
    Ok(())
}

fn insert_fill_checked(
    data: &mut TradeFeedSourceData,
    fill: TradeFillRecord,
) -> ApplicationResult<()> {
    if let Some(existing) = data.by_feed_seq.get(&fill.feed_seq)
        && existing.payload_hash() != fill.payload_hash()
    {
        return Err(ApplicationError::new(
            ApplicationErrorCategory::HashMismatch,
            false,
        ));
    }
    let rev_key = (fill.external_trade_id.clone(), fill.source_revision);
    if let Some(existing_rev) = data.by_trade_rev.get(&rev_key)
        && existing_rev.business_hash() != fill.business_hash()
    {
        return Err(ApplicationError::new(
            ApplicationErrorCategory::HashMismatch,
            false,
        ));
    }
    data.by_feed_seq.insert(fill.feed_seq, fill.clone());
    data.by_trade_rev.entry(rev_key).or_insert(fill);
    Ok(())
}

fn parse_csv_action(text: &str) -> ApplicationResult<TradeFeedRecordAction> {
    match text {
        "NEW_FILL" => Ok(TradeFeedRecordAction::NewFill),
        "AMEND" => Ok(TradeFeedRecordAction::Amend),
        "CANCEL_REVERSAL" => Ok(TradeFeedRecordAction::CancelReversal),
        _ => Err(validation_failed()),
    }
}

fn parse_csv_side(text: &str) -> ApplicationResult<TradeSide> {
    match text {
        "BUY" => Ok(TradeSide::Buy),
        "SELL" => Ok(TradeSide::Sell),
        _ => Err(validation_failed()),
    }
}

const fn validation_failed() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}
