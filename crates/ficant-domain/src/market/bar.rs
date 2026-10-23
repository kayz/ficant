//! Canonical Bar (ADR-0023): one instrument, one settlement type and one frequency over one
//! half-open time interval. Minute and daily bars share this structure and differ only by
//! [`BarFrequency`].

use std::cmp::Ordering;

use chrono::{DateTime, Duration, NaiveDate, Timelike, Utc};
use chrono_tz::Tz;

use crate::primitives::{DecimalValue, VersionRef};

/// Supported bar frequencies. The set is closed so every consumer interprets intervals the same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BarFrequency {
    M1,
    M5,
    M15,
    M30,
    H1,
    D1,
}

impl BarFrequency {
    pub const ALL: [Self; 6] = [Self::M1, Self::M5, Self::M15, Self::M30, Self::H1, Self::D1];

    pub const fn code(self) -> &'static str {
        match self {
            Self::M1 => "M1",
            Self::M5 => "M5",
            Self::M15 => "M15",
            Self::M30 => "M30",
            Self::H1 => "H1",
            Self::D1 => "D1",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|value| value.code() == code)
    }

    /// Interval length in minutes for intraday frequencies; `None` for the daily session bar.
    pub const fn interval_minutes(self) -> Option<u32> {
        match self {
            Self::M1 => Some(1),
            Self::M5 => Some(5),
            Self::M15 => Some(15),
            Self::M30 => Some(30),
            Self::H1 => Some(60),
            Self::D1 => None,
        }
    }
}

/// Settlement type of the trades summarized by a bar. Different settlement types trade at
/// different prices, so they never share a bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BarSettlement {
    T0,
    T1,
    Unspecified,
}

impl BarSettlement {
    pub const ALL: [Self; 3] = [Self::T0, Self::T1, Self::Unspecified];

    pub const fn code(self) -> &'static str {
        match self {
            Self::T0 => "T0",
            Self::T1 => "T1",
            Self::Unspecified => "UNSPECIFIED",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|value| value.code() == code)
    }
}

/// Why a bar violates the canonical rules. Data adapters report these per source row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BarRuleViolation {
    /// Interval does not match the frequency or is not aligned to the market-time grid.
    MisalignedInterval,
    /// Prices are partially missing, high/low do not bound open/close, or units differ.
    InconsistentOhlc,
    /// Volume or another amount is negative.
    NegativeVolume,
    /// The bar is visible before it ends (`observed_at > visible_at`, I1).
    ObservedAfterVisible,
    /// A value could not be represented (bad timezone, empty identifier, ...).
    InvalidValue,
}

/// Open, high, low and close clean prices, all in one price unit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BarPrices {
    pub open: DecimalValue,
    pub high: DecimalValue,
    pub low: DecimalValue,
    pub close: DecimalValue,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BarInput {
    pub source_record_id: String,
    pub instrument: VersionRef,
    pub frequency: BarFrequency,
    pub settlement: BarSettlement,
    pub market_timezone: String,
    pub local_trading_date: NaiveDate,
    pub bar_start: DateTime<Utc>,
    pub bar_end: DateTime<Utc>,
    pub visible_at: DateTime<Utc>,
    /// `None` when the interval had no trades; then `volume` must be zero.
    pub prices: Option<BarPrices>,
    pub volume: DecimalValue,
    pub turnover: Option<DecimalValue>,
    pub trade_count: Option<u64>,
    pub vwap: Option<DecimalValue>,
    pub close_yield: Option<DecimalValue>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bar {
    input: BarInput,
}

impl Bar {
    /// Validates one canonical bar.
    ///
    /// Trading-session membership needs the market calendar and is checked by the ingestion
    /// boundary; this constructor checks everything that follows from the bar itself.
    pub fn new(input: BarInput) -> Result<Self, BarRuleViolation> {
        if input.source_record_id.trim().is_empty()
            || input.source_record_id != input.source_record_id.trim()
        {
            return Err(BarRuleViolation::InvalidValue);
        }
        let timezone = input
            .market_timezone
            .parse::<Tz>()
            .map_err(|_| BarRuleViolation::InvalidValue)?;
        validate_interval(&input, timezone)?;
        if input.bar_end > input.visible_at {
            return Err(BarRuleViolation::ObservedAfterVisible);
        }
        validate_amounts(&input)?;
        if let Some(prices) = &input.prices {
            validate_prices(prices, &input)?;
        } else if !is_zero(&input.volume) || input.vwap.is_some() || input.close_yield.is_some() {
            return Err(BarRuleViolation::InconsistentOhlc);
        }
        Ok(Self { input })
    }

    pub fn source_record_id(&self) -> &str {
        &self.input.source_record_id
    }

    pub fn instrument(&self) -> &VersionRef {
        &self.input.instrument
    }

    pub const fn frequency(&self) -> BarFrequency {
        self.input.frequency
    }

    pub const fn settlement(&self) -> BarSettlement {
        self.input.settlement
    }

    pub fn market_timezone(&self) -> &str {
        &self.input.market_timezone
    }

    pub const fn local_trading_date(&self) -> NaiveDate {
        self.input.local_trading_date
    }

    pub const fn bar_start(&self) -> DateTime<Utc> {
        self.input.bar_start
    }

    pub const fn bar_end(&self) -> DateTime<Utc> {
        self.input.bar_end
    }

    /// The bar describes the market as of the end of its interval.
    pub const fn observed_at(&self) -> DateTime<Utc> {
        self.input.bar_end
    }

    pub const fn visible_at(&self) -> DateTime<Utc> {
        self.input.visible_at
    }

    pub fn prices(&self) -> Option<&BarPrices> {
        self.input.prices.as_ref()
    }

    pub fn volume(&self) -> &DecimalValue {
        &self.input.volume
    }

    pub fn turnover(&self) -> Option<&DecimalValue> {
        self.input.turnover.as_ref()
    }

    pub const fn trade_count(&self) -> Option<u64> {
        self.input.trade_count
    }

    pub fn vwap(&self) -> Option<&DecimalValue> {
        self.input.vwap.as_ref()
    }

    pub fn close_yield(&self) -> Option<&DecimalValue> {
        self.input.close_yield.as_ref()
    }
}

fn validate_interval(input: &BarInput, timezone: Tz) -> Result<(), BarRuleViolation> {
    if input.bar_start >= input.bar_end {
        return Err(BarRuleViolation::MisalignedInterval);
    }
    let local_start = input.bar_start.with_timezone(&timezone);
    if local_start.date_naive() != input.local_trading_date {
        return Err(BarRuleViolation::MisalignedInterval);
    }
    match input.frequency.interval_minutes() {
        Some(minutes) => {
            if input.bar_end - input.bar_start != Duration::minutes(i64::from(minutes))
                || local_start.second() != 0
                || local_start.nanosecond() != 0
                || !(local_start.hour() * 60 + local_start.minute()).is_multiple_of(minutes)
            {
                return Err(BarRuleViolation::MisalignedInterval);
            }
        }
        None => {
            // A daily bar spans one trading session, which never crosses the local date.
            if input.bar_end.with_timezone(&timezone).date_naive() != input.local_trading_date {
                return Err(BarRuleViolation::MisalignedInterval);
            }
        }
    }
    Ok(())
}

fn validate_amounts(input: &BarInput) -> Result<(), BarRuleViolation> {
    if is_negative(&input.volume) || input.turnover.as_ref().is_some_and(is_negative) {
        return Err(BarRuleViolation::NegativeVolume);
    }
    Ok(())
}

fn validate_prices(prices: &BarPrices, input: &BarInput) -> Result<(), BarRuleViolation> {
    let unit = prices.open.unit();
    let all = [&prices.open, &prices.high, &prices.low, &prices.close];
    if all
        .iter()
        .any(|value| value.unit() != unit || is_negative(value))
        || input.vwap.as_ref().is_some_and(|vwap| vwap.unit() != unit)
    {
        return Err(BarRuleViolation::InconsistentOhlc);
    }
    let not_above = |left: &DecimalValue, right: &DecimalValue| {
        left.compare(right)
            .map(|ordering| ordering != Ordering::Greater)
            .map_err(|_| BarRuleViolation::InvalidValue)
    };
    if !not_above(&prices.low, &prices.open)?
        || !not_above(&prices.low, &prices.close)?
        || !not_above(&prices.open, &prices.high)?
        || !not_above(&prices.close, &prices.high)?
    {
        return Err(BarRuleViolation::InconsistentOhlc);
    }
    if let Some(vwap) = &input.vwap
        && (!not_above(&prices.low, vwap)? || !not_above(vwap, &prices.high)?)
    {
        return Err(BarRuleViolation::InconsistentOhlc);
    }
    Ok(())
}

fn is_negative(value: &DecimalValue) -> bool {
    value.coefficient().starts_with('-')
}

fn is_zero(value: &DecimalValue) -> bool {
    value.coefficient() == "0"
}
