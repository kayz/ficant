use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

use ficant_application::ports::{ApplicationResult, MarketQuoteStreamPort, validate_spi_page_size};
use ficant_application::use_cases::blotter::{
    MarketQuoteTick, PullMarketQuotesResult, QuoteCursor,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_domain::market::PriceSourceType;

use crate::spi::csv_rfc4180::{
    CsvTable, parse_csv_fixed, parse_csv_market_time, parse_csv_non_empty, parse_csv_u64,
};

type QuotePairMap = BTreeMap<(String, String), BTreeMap<u64, MarketQuoteTick>>;

/// In-memory open-source reference adapter for `MarketQuoteStreamPort`.
#[derive(Debug, Default)]
pub struct MockMarketQuoteStream {
    ticks: Mutex<QuotePairMap>,
}

impl MockMarketQuoteStream {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_tick(&self, tick: MarketQuoteTick) -> ApplicationResult<()> {
        tick.validate()?;
        let mut guard = self
            .ticks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        insert_tick_checked(&mut guard, tick)
    }
}

impl MarketQuoteStreamPort for MockMarketQuoteStream {
    fn pull_market_quotes(
        &self,
        cursor: &QuoteCursor,
        max_ticks: u32,
    ) -> ApplicationResult<PullMarketQuotesResult> {
        validate_cursor_and_page(cursor, max_ticks)?;
        let guard = self
            .ticks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(pull_quotes_page(&guard, cursor, max_ticks))
    }
}

/// RFC-4180 CSV open-source reference adapter for `MarketQuoteStreamPort`.
///
/// Expected CSV headers:
/// `source_id,quote_seq,instrument_code,source_type,bid_clean_price,ask_clean_price,mid_clean_price,mid_yield_to_maturity,observed_at_rfc3339,received_at_rfc3339,market_timezone`
#[derive(Clone, Debug)]
pub struct CsvMarketQuoteStream {
    ticks: QuotePairMap,
}

impl CsvMarketQuoteStream {
    pub fn from_csv_str(csv_text: &str) -> ApplicationResult<Self> {
        let table = CsvTable::parse(csv_text)?;
        table.require_headers(&[
            "source_id",
            "quote_seq",
            "instrument_code",
            "source_type",
            "bid_clean_price",
            "ask_clean_price",
            "mid_clean_price",
            "mid_yield_to_maturity",
            "observed_at_rfc3339",
            "received_at_rfc3339",
            "market_timezone",
        ])?;

        let mut ticks: QuotePairMap = BTreeMap::new();
        for row in table.rows() {
            let source_id = parse_csv_non_empty(table.field(row, "source_id")?)?;
            let quote_seq = parse_csv_u64(table.field(row, "quote_seq")?)?;
            let instrument_code = parse_csv_non_empty(table.field(row, "instrument_code")?)?;
            let source_type = parse_price_source_type(table.field(row, "source_type")?)?;
            let bid_clean_price = table
                .optional_field(row, "bid_clean_price")
                .map(parse_csv_fixed)
                .transpose()?;
            let ask_clean_price = table
                .optional_field(row, "ask_clean_price")
                .map(parse_csv_fixed)
                .transpose()?;
            let mid_clean_price = table
                .optional_field(row, "mid_clean_price")
                .map(parse_csv_fixed)
                .transpose()?;
            let mid_yield_to_maturity = table
                .optional_field(row, "mid_yield_to_maturity")
                .map(parse_csv_fixed)
                .transpose()?;
            let market_tz = parse_csv_non_empty(table.field(row, "market_timezone")?)?;
            let observed_at =
                parse_csv_market_time(table.field(row, "observed_at_rfc3339")?, &market_tz)?;
            let received_at =
                parse_csv_market_time(table.field(row, "received_at_rfc3339")?, &market_tz)?;

            let tick = MarketQuoteTick {
                source_id,
                quote_seq,
                instrument_code,
                source_type,
                bid_clean_price,
                ask_clean_price,
                mid_clean_price,
                mid_yield_to_maturity,
                observed_at,
                received_at,
            };
            tick.validate()?;
            insert_tick_checked(&mut ticks, tick)?;
        }

        Ok(Self { ticks })
    }

    pub fn from_csv_file(path: impl AsRef<Path>) -> ApplicationResult<Self> {
        let raw = fs::read_to_string(path).map_err(|_| validation_failed())?;
        Self::from_csv_str(&raw)
    }
}

impl MarketQuoteStreamPort for CsvMarketQuoteStream {
    fn pull_market_quotes(
        &self,
        cursor: &QuoteCursor,
        max_ticks: u32,
    ) -> ApplicationResult<PullMarketQuotesResult> {
        validate_cursor_and_page(cursor, max_ticks)?;
        Ok(pull_quotes_page(&self.ticks, cursor, max_ticks))
    }
}

fn validate_cursor_and_page(cursor: &QuoteCursor, max_ticks: u32) -> ApplicationResult<()> {
    parse_csv_non_empty(&cursor.source_id)?;
    parse_csv_non_empty(&cursor.instrument_code)?;
    validate_spi_page_size(max_ticks)?;
    Ok(())
}

fn insert_tick_checked(map: &mut QuotePairMap, tick: MarketQuoteTick) -> ApplicationResult<()> {
    let pair = (tick.source_id.clone(), tick.instrument_code.clone());
    let per_pair = map.entry(pair).or_default();
    if let Some(existing) = per_pair.get(&tick.quote_seq)
        && existing.canonical_payload_hash() != tick.canonical_payload_hash()
    {
        return Err(ApplicationError::new(
            ApplicationErrorCategory::HashMismatch,
            false,
        ));
    }
    per_pair.insert(tick.quote_seq, tick);
    Ok(())
}

fn pull_quotes_page(
    map: &QuotePairMap,
    cursor: &QuoteCursor,
    max_ticks: u32,
) -> PullMarketQuotesResult {
    let pair = (cursor.source_id.clone(), cursor.instrument_code.clone());
    let Some(series) = map.get(&pair) else {
        return PullMarketQuotesResult {
            ticks: Vec::new(),
            next_cursor: cursor.clone(),
            has_more: false,
        };
    };

    let limit = usize::try_from(max_ticks).unwrap_or(usize::MAX);
    let mut ticks = Vec::new();
    let mut has_more = false;

    for (&seq, tick) in series.range((cursor.after_quote_seq.saturating_add(1))..) {
        if ticks.len() >= limit {
            has_more = true;
            break;
        }
        ticks.push(tick.clone());
        let _ = seq;
    }

    let next_after = ticks
        .last()
        .map_or(cursor.after_quote_seq, |last| last.quote_seq);
    PullMarketQuotesResult {
        ticks,
        next_cursor: QuoteCursor {
            source_id: cursor.source_id.clone(),
            instrument_code: cursor.instrument_code.clone(),
            after_quote_seq: next_after,
        },
        has_more,
    }
}

fn parse_price_source_type(text: &str) -> ApplicationResult<PriceSourceType> {
    match text {
        "REAL_TRADE" => Ok(PriceSourceType::RealTrade),
        "ACTIVE_QUOTE" => Ok(PriceSourceType::ActiveQuote),
        "MODEL_VALUATION" => Ok(PriceSourceType::ModelValuation),
        "CURVE_INTERPOLATION" => Ok(PriceSourceType::CurveInterpolation),
        _ => Err(validation_failed()),
    }
}

const fn validation_failed() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}
