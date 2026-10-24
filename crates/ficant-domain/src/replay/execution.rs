//! Simulated execution with a participation cap (ADR-0025 §6; AC41).

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use super::{ConstraintOutcome, ReplayAmount};
use crate::market::{Bar, PriceSourceType};
use crate::primitives::VersionRef;
use crate::{DomainErrorCode, DomainResult};

/// What happens to the part of a trade a bar could not absorb.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RemainderHandling {
    /// Drop it; the policy decides again at its next decision time from the actual book.
    Cancel,
    /// Retry it in the instrument's next bar until filled or no bar is left.
    Carry,
}

impl RemainderHandling {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Cancel => "CANCEL",
            Self::Carry => "CARRY",
        }
    }
}

/// Execution rules a `PolicyArtifact` declares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutionRules {
    participation_cap: ReplayAmount,
    remainder: RemainderHandling,
}

impl ExecutionRules {
    /// `participation_cap` is the largest share of a bar's traded face a simulated fill may take,
    /// in (0, 1].
    pub fn new(
        participation_cap: ReplayAmount,
        remainder: RemainderHandling,
    ) -> DomainResult<Self> {
        if participation_cap.value() <= Decimal::ZERO || participation_cap.value() > Decimal::ONE {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            participation_cap,
            remainder,
        })
    }

    pub const fn participation_cap(&self) -> ReplayAmount {
        self.participation_cap
    }

    pub const fn remainder(&self) -> RemainderHandling {
        self.remainder
    }
}

/// Which price a simulated fill used (I9 travels separately as the snapshot's price source type).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FillPriceBasis {
    Vwap,
    Close,
}

impl FillPriceBasis {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Vwap => "VWAP",
            Self::Close => "CLOSE",
        }
    }
}

/// Why (part of) a requested trade did not fill.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NoFillReason {
    /// The execution bar had no trades (AC41).
    NoPriceInBar,
    /// The snapshot has no further bar for this instrument.
    NoLaterBar,
    /// The bar's traded face times the participation cap was smaller than the request (AC41).
    ParticipationCap,
    /// A constraint of the Subject's constraint set would have been breached (AC42).
    Constraint,
}

impl NoFillReason {
    pub const fn code(self) -> &'static str {
        match self {
            Self::NoPriceInBar => "NO_PRICE_IN_BAR",
            Self::NoLaterBar => "NO_LATER_BAR",
            Self::ParticipationCap => "PARTICIPATION_CAP",
            Self::Constraint => "CONSTRAINT",
        }
    }
}

/// What became of the part of a request that one execution did not fill.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RemainderOutcome {
    Cancelled,
    Carried,
}

impl RemainderOutcome {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Cancelled => "CANCELLED",
            Self::Carried => "CARRIED",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Remainder {
    /// Signed face left after this execution.
    pub quantity: ReplayAmount,
    pub reason: NoFillReason,
    pub outcome: RemainderOutcome,
}

/// One simulated execution attempt in one bar. Never an order (AC44).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimulatedFill {
    pub decision_time: DateTime<Utc>,
    pub instrument: VersionRef,
    /// Signed face this attempt tried to execute.
    pub requested: ReplayAmount,
    /// Signed face actually executed; zero when nothing filled.
    pub quantity: ReplayAmount,
    /// Bar traded face × participation cap, less what earlier attempts in the same bar used;
    /// `None` when there was no bar to execute in.
    pub limit: Option<ReplayAmount>,
    /// When the attempt became part of the book: end of the execution bar, or, with no bar left,
    /// the end of the previous attempt (the decision time for the first one).
    pub effective_at: DateTime<Utc>,
    pub execution_bar: Option<String>,
    pub price: Option<ReplayAmount>,
    pub basis: Option<FillPriceBasis>,
    /// Price source type of the snapshot the attempt executed against (I9, AC41).
    pub price_source: PriceSourceType,
    /// Set when nothing executed.
    pub no_fill: Option<NoFillReason>,
    /// Set when less than `requested` executed.
    pub remainder: Option<Remainder>,
    /// Set when a constraint rejected or reduced the attempt (AC42).
    pub constraint: Option<ConstraintOutcome>,
}

impl SimulatedFill {
    pub fn filled(&self) -> bool {
        self.no_fill.is_none()
    }
}

/// One execution attempt of `remaining` face in `bar`, before constraints.
///
/// The bar fills at most `traded face × participation cap`, less the face `used` by attempts
/// already executed in it, at its VWAP, else its close; a bar
/// without trades fills nothing. The shortfall is marked cancelled or carried as the rules
/// declare; the caller decides whether a carried shortfall still has a bar to go to.
pub fn attempt_in_bar(
    decision_time: DateTime<Utc>,
    instrument: &VersionRef,
    remaining: ReplayAmount,
    bar: &Bar,
    used: ReplayAmount,
    rules: ExecutionRules,
    price_source: PriceSourceType,
) -> SimulatedFill {
    let remaining = remaining.value();
    let priced = match (bar.vwap(), bar.prices()) {
        (Some(vwap), _) => Some((ReplayAmount::of(vwap), FillPriceBasis::Vwap)),
        (None, Some(prices)) => Some((ReplayAmount::of(&prices.close), FillPriceBasis::Close)),
        (None, None) => None,
    };
    let limit = (ReplayAmount::of(bar.volume()).value() * rules.participation_cap().value()
        - used.value())
    .max(Decimal::ZERO);
    let executed = if priced.is_some() {
        signed_like(remaining, remaining.abs().min(limit))
    } else {
        Decimal::ZERO
    };
    let left = remaining - executed;
    let shortfall_reason = if priced.is_some() {
        NoFillReason::ParticipationCap
    } else {
        NoFillReason::NoPriceInBar
    };
    let outcome = match rules.remainder() {
        RemainderHandling::Cancel => RemainderOutcome::Cancelled,
        RemainderHandling::Carry => RemainderOutcome::Carried,
    };
    SimulatedFill {
        decision_time,
        instrument: instrument.clone(),
        requested: ReplayAmount::from_decimal(remaining),
        quantity: ReplayAmount::from_decimal(executed),
        limit: Some(ReplayAmount::from_decimal(limit)),
        effective_at: bar.bar_end(),
        execution_bar: Some(bar.source_record_id().to_owned()),
        price: priced.map(|(price, _)| price),
        basis: priced.map(|(_, basis)| basis),
        price_source,
        no_fill: executed.is_zero().then_some(shortfall_reason),
        remainder: (!left.is_zero()).then_some(Remainder {
            quantity: ReplayAmount::from_decimal(left),
            reason: shortfall_reason,
            outcome,
        }),
        constraint: None,
    }
}

/// The record of a trade (or its carried remainder) that has no bar left to execute in.
/// `effective_at` is the end of the previous attempt, or the decision time for a new trade.
pub fn no_later_bar(
    decision_time: DateTime<Utc>,
    instrument: &VersionRef,
    remaining: ReplayAmount,
    effective_at: DateTime<Utc>,
    price_source: PriceSourceType,
) -> SimulatedFill {
    SimulatedFill {
        decision_time,
        instrument: instrument.clone(),
        requested: remaining,
        quantity: ReplayAmount::ZERO,
        limit: None,
        effective_at,
        execution_bar: None,
        price: None,
        basis: None,
        price_source,
        no_fill: Some(NoFillReason::NoLaterBar),
        remainder: Some(Remainder {
            quantity: remaining,
            reason: NoFillReason::NoLaterBar,
            outcome: RemainderOutcome::Cancelled,
        }),
        constraint: None,
    }
}

impl SimulatedFill {
    /// Applies a constraint decision: the attempt executes `allowed` instead, and whatever it no
    /// longer executes is cancelled, because a constraint does not relax in the next bar.
    pub fn constrain(&mut self, allowed: ReplayAmount, outcome: ConstraintOutcome) {
        self.quantity = allowed;
        self.no_fill = allowed.is_zero().then_some(NoFillReason::Constraint);
        let left = self.requested.minus(allowed);
        self.remainder = (!left.is_zero()).then_some(Remainder {
            quantity: left,
            reason: NoFillReason::Constraint,
            outcome: RemainderOutcome::Cancelled,
        });
        self.constraint = Some(outcome);
    }
}

pub(crate) fn signed_like(sign_of: Decimal, magnitude: Decimal) -> Decimal {
    if sign_of.is_sign_negative() {
        -magnitude
    } else {
        magnitude
    }
}
