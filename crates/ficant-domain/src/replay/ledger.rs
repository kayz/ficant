//! Replay accounting: book, equity and per-bar P&L breakdown (ADR-0025; AC40).
//!
//! Equity = cash + Σ position × (clean mark + accrued interest) / 100. Each step's total P&L is
//! the change in equity, computed on its own; the four components are computed separately and
//! must add up to it exactly, or the step fails.

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::{Decimal, RoundingStrategy};

use super::{ExecutionContext, ReplayAmount, SimulatedBook, SimulatedFill};
use crate::market::{Bar, BarSettlement};
use crate::primitives::{ContentHash, DECIMAL_SCALE, VersionRef};
use crate::subject::FundingTier;
use crate::{DomainErrorCode, DomainResult};

/// Cost model that charges nothing; its parameters are only bound into the result identity.
pub const COST_NONE: &str = "ficant.cost.none";
/// Cost model `|executed face| × face_rate_bp / 10000 + fee_per_fill` per execution.
pub const COST_RATE_PLUS_FIXED: &str = "ficant.cost.rate-plus-fixed";

const BASIS_POINTS: Decimal = Decimal::from_parts(10_000, 0, 0, false, 0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CostModel {
    None,
    RatePlusFixed {
        face_rate_bp: ReplayAmount,
        fee_per_fill: ReplayAmount,
    },
}

impl CostModel {
    /// Selects a built-in cost model. Unknown names, and missing, extra or negative parameters of
    /// `ficant.cost.rate-plus-fixed`, are rejected.
    pub fn from_declaration(
        name: &str,
        parameters: &BTreeMap<String, ReplayAmount>,
    ) -> DomainResult<Self> {
        match name {
            COST_NONE => Ok(Self::None),
            COST_RATE_PLUS_FIXED => {
                let keys: Vec<&str> = parameters.keys().map(String::as_str).collect();
                if keys != ["face_rate_bp", "fee_per_fill"] {
                    return Err(DomainErrorCode::InvalidValue);
                }
                let face_rate_bp = parameters["face_rate_bp"];
                let fee_per_fill = parameters["fee_per_fill"];
                if face_rate_bp.value().is_sign_negative()
                    || fee_per_fill.value().is_sign_negative()
                {
                    return Err(DomainErrorCode::InvalidValue);
                }
                Ok(Self::RatePlusFixed {
                    face_rate_bp,
                    fee_per_fill,
                })
            }
            _ => Err(DomainErrorCode::InvalidValue),
        }
    }

    /// Signed cost (zero or negative) of executing `executed` face.
    pub fn cost(&self, executed: ReplayAmount) -> ReplayAmount {
        match self {
            Self::None => ReplayAmount::ZERO,
            Self::RatePlusFixed { .. } if executed.is_zero() => ReplayAmount::ZERO,
            Self::RatePlusFixed {
                face_rate_bp,
                fee_per_fill,
            } => ReplayAmount::from_decimal(
                -(executed.value().abs() * face_rate_bp.value() / BASIS_POINTS
                    + fee_per_fill.value()),
            ),
        }
    }
}

/// Funding terms for one replay, taken from the bound Subject's funding tier (AC40).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayFunding {
    rule_pack: VersionRef,
    tier: FundingTier,
    annual_rate: ReplayAmount,
    day_basis: u32,
}

impl ReplayFunding {
    /// `annual_rate` is a fraction (0.018 for 1.8%); `day_basis` is the days-per-year divisor.
    pub fn new(
        rule_pack: VersionRef,
        tier: FundingTier,
        annual_rate: ReplayAmount,
        day_basis: u32,
    ) -> DomainResult<Self> {
        if day_basis == 0 {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            rule_pack,
            tier,
            annual_rate,
            day_basis,
        })
    }

    pub fn rule_pack(&self) -> &VersionRef {
        &self.rule_pack
    }

    pub const fn tier(&self) -> FundingTier {
        self.tier
    }

    pub const fn annual_rate(&self) -> ReplayAmount {
        self.annual_rate
    }

    pub const fn day_basis(&self) -> u32 {
        self.day_basis
    }
}

/// Coupon data for held bonds. Values are per 100 face; the implementation owns the bond terms,
/// day count and settlement-date rules (ADR-0013 keeps those out of the core).
pub trait BondCarry {
    /// Content hash of the data behind this source, bound into the result identity.
    fn identity(&self) -> ContentHash;

    /// Accrued interest for a position marked or traded on `trading_date`.
    fn accrued_interest(
        &self,
        instrument: &VersionRef,
        settlement: BarSettlement,
        trading_date: NaiveDate,
    ) -> Option<ReplayAmount>;

    /// Coupon cash paid to a holder between the end of `after` and the end of `through`.
    fn coupon_paid(
        &self,
        instrument: &VersionRef,
        settlement: BarSettlement,
        after: NaiveDate,
        through: NaiveDate,
    ) -> Option<ReplayAmount>;
}

/// One instrument's part of a step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstrumentPnl {
    pub position_start: ReplayAmount,
    pub position_end: ReplayAmount,
    /// Clean mark at the end of the step.
    pub mark: ReplayAmount,
    /// Accrued interest per 100 face on the step's trading date.
    pub accrued_interest: ReplayAmount,
    pub price: ReplayAmount,
    pub coupon: ReplayAmount,
    pub transaction_cost: ReplayAmount,
}

/// P&L of one replay step (one `bar_end` on the replay's bar grid).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BarPnl {
    pub bar_start: DateTime<Utc>,
    pub bar_end: DateTime<Utc>,
    /// When every bar of the step was visible; the row is fixed from then on.
    pub known_at: DateTime<Utc>,
    pub trading_date: NaiveDate,
    pub instruments: BTreeMap<VersionRef, InstrumentPnl>,
    pub funding: ReplayAmount,
    /// Equity change over the step, computed independently of the components.
    pub total: ReplayAmount,
    pub cash: ReplayAmount,
    pub equity: ReplayAmount,
}

impl BarPnl {
    pub fn price(&self) -> ReplayAmount {
        self.sum(|item| item.price)
    }

    pub fn coupon(&self) -> ReplayAmount {
        self.sum(|item| item.coupon)
    }

    pub fn transaction_cost(&self) -> ReplayAmount {
        self.sum(|item| item.transaction_cost)
    }

    /// Price + coupon + funding + transaction cost.
    pub fn components_total(&self) -> ReplayAmount {
        ReplayAmount::from_decimal(
            self.price().value()
                + self.coupon().value()
                + self.funding.value()
                + self.transaction_cost().value(),
        )
    }

    fn sum(&self, field: impl Fn(&InstrumentPnl) -> ReplayAmount) -> ReplayAmount {
        ReplayAmount::from_decimal(
            self.instruments
                .values()
                .map(|item| field(item).value())
                .sum(),
        )
    }
}

/// The replay's book and its P&L, advanced one step at a time.
pub struct ReplayLedger<'a> {
    carry: &'a dyn BondCarry,
    funding: ReplayFunding,
    costs: CostModel,
    settlement: BarSettlement,
    book: SimulatedBook,
    marks: BTreeMap<VersionRef, Decimal>,
    accrued: BTreeMap<VersionRef, Decimal>,
    date: Option<NaiveDate>,
    equity: Decimal,
    rows: Vec<BarPnl>,
    open: Option<OpenStep>,
}

#[derive(Default)]
struct Parts {
    price: Decimal,
    coupon: Decimal,
    cost: Decimal,
}

impl<'a> ReplayLedger<'a> {
    pub fn new(
        carry: &'a dyn BondCarry,
        funding: ReplayFunding,
        costs: CostModel,
        settlement: BarSettlement,
    ) -> Self {
        Self {
            carry,
            funding,
            costs,
            settlement,
            book: SimulatedBook::default(),
            marks: BTreeMap::new(),
            accrued: BTreeMap::new(),
            date: None,
            equity: Decimal::ZERO,
            rows: Vec::new(),
            open: None,
        }
    }

    /// Settled positions and cash.
    pub fn book(&self) -> &SimulatedBook {
        &self.book
    }

    pub fn rows(&self) -> &[BarPnl] {
        &self.rows
    }

    pub fn into_parts(self) -> (SimulatedBook, Vec<BarPnl>) {
        (self.book, self.rows)
    }

    /// Closes one step in one call: the bars sharing one `bar_end`, and the executions effective
    /// at it, in order.
    ///
    /// Fails when the step is inconsistent, a held bond has no coupon data for the date, or the
    /// components do not add up to the equity change.
    pub fn close_step(&mut self, bars: &[&Bar], fills: &[&SimulatedFill]) -> DomainResult<()> {
        self.open_step(bars)?;
        for fill in fills {
            self.settle(fill)?;
        }
        self.finish_step()
    }

    /// Opens one step: rolls the date (funding, coupon carry) and marks positions held into it.
    /// Executions are then settled one by one with [`Self::settle`], each seeing the book the
    /// previous ones left, and [`Self::finish_step`] books the row.
    ///
    /// Fails when the bars do not form one step, a step is already open, the date goes back, or a
    /// held bond has no coupon data for the date.
    pub fn open_step(&mut self, bars: &[&Bar]) -> DomainResult<()> {
        let first = bars.first().ok_or(DomainErrorCode::InvalidValue)?;
        let (bar_start, bar_end, date) = (
            first.bar_start(),
            first.bar_end(),
            first.local_trading_date(),
        );
        if self.open.is_some()
            || bars.iter().any(|bar| {
                bar.bar_end() != bar_end
                    || bar.bar_start() != bar_start
                    || bar.local_trading_date() != date
            })
            || self.date.is_some_and(|previous| previous > date)
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        let known_at = bars
            .iter()
            .map(|bar| bar.visible_at())
            .max()
            .unwrap_or(bar_end);
        let start: BTreeMap<VersionRef, Decimal> = self
            .book
            .positions()
            .map(|(instrument, face)| (instrument.clone(), face.value()))
            .collect();
        let mut parts: BTreeMap<VersionRef, Parts> = BTreeMap::new();
        let funding = self.roll_date(date, &start, &mut parts)?;
        let closes: BTreeMap<VersionRef, Decimal> = bars
            .iter()
            .filter_map(|bar| {
                bar.prices().map(|prices| {
                    (
                        bar.instrument().clone(),
                        ReplayAmount::of(&prices.close).value(),
                    )
                })
            })
            .collect();
        for (instrument, face) in &start {
            let previous = *self
                .marks
                .get(instrument)
                .ok_or(DomainErrorCode::BrokenLineage)?;
            let mark = closes.get(instrument).copied().unwrap_or(previous);
            parts.entry(instrument.clone()).or_default().price +=
                *face * (mark - previous) / Decimal::ONE_HUNDRED;
        }
        self.open = Some(OpenStep {
            bar_start,
            bar_end,
            date,
            known_at,
            records: bars
                .iter()
                .map(|bar| (bar.instrument().clone(), bar.source_record_id().to_owned()))
                .collect(),
            start,
            parts,
            funding,
            closes,
        });
        Ok(())
    }

    /// The context a constraint check needs for an execution of `instrument` at `price` in the
    /// open step.
    ///
    /// Fails without an open step or without accrued interest for the bond on the step's date.
    pub fn execution_context<'c>(
        &'c self,
        instrument: &'c VersionRef,
        price: ReplayAmount,
    ) -> DomainResult<ExecutionContext<'c>> {
        let date = self
            .open
            .as_ref()
            .ok_or(DomainErrorCode::InvalidValue)?
            .date;
        let accrued = self
            .carry
            .accrued_interest(instrument, self.settlement, date)
            .ok_or(DomainErrorCode::BrokenLineage)?
            .value();
        let (cost_per_face, fee_per_fill) = match self.costs {
            CostModel::None => (Decimal::ZERO, Decimal::ZERO),
            CostModel::RatePlusFixed {
                face_rate_bp,
                fee_per_fill,
            } => (face_rate_bp.value() / BASIS_POINTS, fee_per_fill.value()),
        };
        Ok(ExecutionContext {
            book: &self.book,
            instrument,
            dirty_per_face: (price.value() + accrued) / Decimal::ONE_HUNDRED,
            cost_per_face,
            fee_per_fill,
        })
    }

    /// Settles one execution attempt of the open step; attempts that executed nothing are
    /// ignored.
    ///
    /// Fails without an open step, or when the attempt is not effective at the step's end in one
    /// of its bars.
    pub fn settle(&mut self, fill: &SimulatedFill) -> DomainResult<()> {
        let mut open = self.open.take().ok_or(DomainErrorCode::InvalidValue)?;
        let result = if fill.quantity.is_zero() {
            Ok(())
        } else {
            self.execute(fill, &mut open)
        };
        self.open = Some(open);
        result
    }

    /// Books the row of the open step.
    ///
    /// Fails without an open step, or when the components do not add up to the equity change.
    pub fn finish_step(&mut self) -> DomainResult<()> {
        let OpenStep {
            bar_start,
            bar_end,
            date,
            known_at,
            start,
            parts,
            funding,
            closes,
            ..
        } = self.open.take().ok_or(DomainErrorCode::InvalidValue)?;
        for (instrument, close) in closes {
            self.marks.insert(instrument, close);
        }
        let equity = self.equity_now()?;
        let total = equity - self.equity;
        let components = funding
            + parts
                .values()
                .map(|part| part.price + part.coupon + part.cost)
                .sum::<Decimal>();
        if total != components {
            return Err(DomainErrorCode::InvalidValue);
        }
        let instruments = self.instrument_rows(&start, &parts);
        self.equity = equity;
        self.rows.push(BarPnl {
            bar_start,
            bar_end,
            known_at,
            trading_date: date,
            instruments,
            funding: ReplayAmount::from_decimal(funding),
            total: ReplayAmount::from_decimal(total),
            cash: self.book.cash(),
            equity: ReplayAmount::from_decimal(equity),
        });
        Ok(())
    }

    /// Cash plus every position at its clean mark and accrued interest.
    fn equity_now(&self) -> DomainResult<Decimal> {
        let mut equity = self.book.cash().value();
        for (instrument, face) in self.book.positions() {
            let mark = self
                .marks
                .get(instrument)
                .ok_or(DomainErrorCode::BrokenLineage)?;
            let accrued = self
                .accrued
                .get(instrument)
                .ok_or(DomainErrorCode::BrokenLineage)?;
            equity += face.value() * (mark + accrued) / Decimal::ONE_HUNDRED;
        }
        Ok(equity)
    }

    /// One row per instrument held or traded in the step; forgets accrued interest of
    /// instruments that are now flat.
    fn instrument_rows(
        &mut self,
        start: &BTreeMap<VersionRef, Decimal>,
        parts: &BTreeMap<VersionRef, Parts>,
    ) -> BTreeMap<VersionRef, InstrumentPnl> {
        let mut instruments = BTreeMap::new();
        for (instrument, part) in parts {
            let decimal = |values: &BTreeMap<VersionRef, Decimal>| {
                ReplayAmount::from_decimal(values.get(instrument).copied().unwrap_or_default())
            };
            instruments.insert(
                instrument.clone(),
                InstrumentPnl {
                    position_start: decimal(start),
                    position_end: self.book.position(instrument),
                    mark: decimal(&self.marks),
                    accrued_interest: decimal(&self.accrued),
                    price: ReplayAmount::from_decimal(part.price),
                    coupon: ReplayAmount::from_decimal(part.coupon),
                    transaction_cost: ReplayAmount::from_decimal(part.cost),
                },
            );
            if self.book.position(instrument).is_zero() {
                self.accrued.remove(instrument);
            }
        }
        instruments
    }

    /// On the first step of a new trading date: charges funding on yesterday's borrowed cash and
    /// books coupon carry on overnight positions. Returns the (signed) funding amount.
    fn roll_date(
        &mut self,
        date: NaiveDate,
        start: &BTreeMap<VersionRef, Decimal>,
        parts: &mut BTreeMap<VersionRef, Parts>,
    ) -> DomainResult<Decimal> {
        let previous = self.date.replace(date);
        let Some(previous) = previous.filter(|previous| *previous != date) else {
            return Ok(Decimal::ZERO);
        };
        let days = Decimal::from((date - previous).num_days());
        let borrowed = (-self.book.cash().value()).max(Decimal::ZERO);
        // Rounded to the platform decimal scale so every booked amount is a finite decimal and
        // the components add up to the equity change exactly.
        let funding = (-(borrowed * self.funding.annual_rate.value() * days
            / Decimal::from(self.funding.day_basis)))
        .round_dp_with_strategy(DECIMAL_SCALE, RoundingStrategy::MidpointNearestEven);
        self.book.add_cash(funding);
        for (instrument, face) in start {
            let before = *self
                .accrued
                .get(instrument)
                .ok_or(DomainErrorCode::BrokenLineage)?;
            let after = self
                .carry
                .accrued_interest(instrument, self.settlement, date)
                .ok_or(DomainErrorCode::BrokenLineage)?
                .value();
            let paid = self
                .carry
                .coupon_paid(instrument, self.settlement, previous, date)
                .ok_or(DomainErrorCode::BrokenLineage)?
                .value();
            self.book.add_cash(*face * paid / Decimal::ONE_HUNDRED);
            parts.entry(instrument.clone()).or_default().coupon +=
                *face * (after - before + paid) / Decimal::ONE_HUNDRED;
            self.accrued.insert(instrument.clone(), after);
        }
        Ok(funding)
    }

    /// Settles one execution at dirty price and charges its cost.
    fn execute(&mut self, fill: &SimulatedFill, open: &mut OpenStep) -> DomainResult<()> {
        let in_step = open.records.iter().any(|(instrument, record)| {
            instrument == &fill.instrument && Some(record.as_str()) == fill.execution_bar.as_deref()
        });
        let price = fill.price.ok_or(DomainErrorCode::InvalidValue)?.value();
        if fill.effective_at != open.bar_end || !in_step {
            return Err(DomainErrorCode::InvalidValue);
        }
        let accrued = self
            .carry
            .accrued_interest(&fill.instrument, self.settlement, open.date)
            .ok_or(DomainErrorCode::BrokenLineage)?
            .value();
        let face = fill.quantity.value();
        let mark = open.closes.get(&fill.instrument).copied().unwrap_or(price);
        let cost = self.costs.cost(fill.quantity).value();
        self.book
            .add_cash(-(face * (price + accrued) / Decimal::ONE_HUNDRED) + cost);
        self.book.add_position(&fill.instrument, face);
        self.accrued.insert(fill.instrument.clone(), accrued);
        self.marks.entry(fill.instrument.clone()).or_insert(mark);
        let part = open.parts.entry(fill.instrument.clone()).or_default();
        part.price += face * (mark - price) / Decimal::ONE_HUNDRED;
        part.cost += cost;
        Ok(())
    }
}

/// A step between [`ReplayLedger::open_step`] and [`ReplayLedger::finish_step`].
struct OpenStep {
    bar_start: DateTime<Utc>,
    bar_end: DateTime<Utc>,
    date: NaiveDate,
    known_at: DateTime<Utc>,
    records: Vec<(VersionRef, String)>,
    start: BTreeMap<VersionRef, Decimal>,
    parts: BTreeMap<VersionRef, Parts>,
    funding: Decimal,
    closes: BTreeMap<VersionRef, Decimal>,
}
