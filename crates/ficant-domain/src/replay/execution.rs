//! Simulated execution with a participation cap (ADR-0025 §6; AC41).

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use super::ReplayAmount;
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
}

impl NoFillReason {
    pub const fn code(self) -> &'static str {
        match self {
            Self::NoPriceInBar => "NO_PRICE_IN_BAR",
            Self::NoLaterBar => "NO_LATER_BAR",
            Self::ParticipationCap => "PARTICIPATION_CAP",
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
    /// Bar traded face × participation cap; `None` when there was no bar to execute in.
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
}

impl SimulatedFill {
    pub fn filled(&self) -> bool {
        self.no_fill.is_none()
    }
}

/// Simulates executing `quantity` of `instrument` decided at `decision_time`.
///
/// `bars` are the instrument's bars starting at or after the decision, in time order; the first
/// is the execution bar. Each bar fills at most `traded face × participation cap` at its VWAP,
/// else its close; a bar without trades fills nothing. What a bar cannot absorb is cancelled or
/// carried to the next bar as the rules declare.
pub fn simulate_trade(
    decision_time: DateTime<Utc>,
    instrument: &VersionRef,
    quantity: ReplayAmount,
    bars: &[&Bar],
    rules: ExecutionRules,
    price_source: PriceSourceType,
) -> Vec<SimulatedFill> {
    let mut attempts = Vec::new();
    let mut remaining = quantity.value();
    let mut last_effective = decision_time;
    for bar in bars {
        let priced = match (bar.vwap(), bar.prices()) {
            (Some(vwap), _) => Some((ReplayAmount::of(vwap), FillPriceBasis::Vwap)),
            (None, Some(prices)) => Some((ReplayAmount::of(&prices.close), FillPriceBasis::Close)),
            (None, None) => None,
        };
        let limit = ReplayAmount::of(bar.volume()).value() * rules.participation_cap().value();
        let executed = if priced.is_some() {
            let size = remaining.abs().min(limit);
            if remaining.is_sign_negative() {
                -size
            } else {
                size
            }
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
        attempts.push(SimulatedFill {
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
        });
        last_effective = bar.bar_end();
        if left.is_zero() || outcome == RemainderOutcome::Cancelled {
            return attempts;
        }
        remaining = left;
    }
    let left = ReplayAmount::from_decimal(remaining);
    attempts.push(SimulatedFill {
        decision_time,
        instrument: instrument.clone(),
        requested: left,
        quantity: ReplayAmount::ZERO,
        limit: None,
        effective_at: last_effective,
        execution_bar: None,
        price: None,
        basis: None,
        price_source,
        no_fill: Some(NoFillReason::NoLaterBar),
        remainder: Some(Remainder {
            quantity: left,
            reason: NoFillReason::NoLaterBar,
            outcome: RemainderOutcome::Cancelled,
        }),
    });
    attempts
}
