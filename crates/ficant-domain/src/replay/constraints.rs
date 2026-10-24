//! Constraint checks on simulated executions (ADR-0027; AC42).
//!
//! A check looks at the book just before one execution and at the book it would leave. A
//! constraint is breached when the measure afterwards exceeds its ceiling and is larger than
//! before, so trades that reduce an existing excess always pass.

use std::collections::BTreeMap;

use rust_decimal::{Decimal, RoundingStrategy};

use super::execution::signed_like;
use super::{ReplayAmount, SimulatedBook};
use crate::constraint::{ConstraintMeasure, ConstraintSet};
use crate::primitives::{DECIMAL_SCALE, VersionRef};
use crate::subject::SubjectStateSnapshot;
use crate::{DomainErrorCode, DomainResult};

/// What a `PolicyArtifact` declares for one constraint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConstraintHandling {
    /// The breaching execution does not happen at all.
    Reject,
    /// The execution shrinks to the largest size that breaches nothing.
    Reduce,
}

impl ConstraintHandling {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Reject => "REJECT",
            Self::Reduce => "REDUCE",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConstraintAction {
    Rejected,
    Reduced,
}

impl ConstraintAction {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Rejected => "REJECTED",
            Self::Reduced => "REDUCED",
        }
    }
}

/// How constraints changed one execution attempt (AC42).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConstraintOutcome {
    /// Signed face the attempt would have executed without constraints.
    pub unconstrained: ReplayAmount,
    /// Limit codes breached at the unconstrained size, in code order.
    pub triggered: Vec<String>,
    pub action: ConstraintAction,
}

/// One constraint as the replay enforces it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayConstraint {
    pub limit_code: String,
    pub measure: ConstraintMeasure,
    pub ceiling: ReplayAmount,
    pub handling: ConstraintHandling,
}

/// Binds the Subject's constraint set to its state snapshot and the policy's declarations.
///
/// Fails when the set or snapshot does not belong to the Subject, a ceiling is missing or
/// negative, or the policy declares no handling for one of the set's constraints.
pub fn bind_constraints(
    set: &ConstraintSet,
    subject: &VersionRef,
    state: &SubjectStateSnapshot,
    declared: &BTreeMap<String, ConstraintHandling>,
) -> DomainResult<Vec<ReplayConstraint>> {
    set.bind(subject, state)?
        .into_iter()
        .map(|bound| {
            let handling = *declared
                .get(bound.limit_code())
                .ok_or(DomainErrorCode::InvalidValue)?;
            Ok(ReplayConstraint {
                limit_code: bound.limit_code().to_owned(),
                measure: bound.measure(),
                ceiling: ReplayAmount::of(bound.ceiling()),
                handling,
            })
        })
        .collect()
}

/// The book an execution starts from, and how cash moves with its size.
pub struct ExecutionContext<'a> {
    pub book: &'a SimulatedBook,
    pub instrument: &'a VersionRef,
    /// Cash outflow per unit of face bought: (price + accrued interest) / 100.
    pub dirty_per_face: Decimal,
    /// Cost per unit of traded face (rate) and per execution (fixed fee), both non-negative.
    pub cost_per_face: Decimal,
    pub fee_per_fill: Decimal,
}

impl ExecutionContext<'_> {
    fn cash_after(&self, face: Decimal) -> Decimal {
        let cost = if face.is_zero() {
            Decimal::ZERO
        } else {
            face.abs() * self.cost_per_face + self.fee_per_fill
        };
        self.book.cash().value() - face * self.dirty_per_face - cost
    }

    fn measure(&self, measure: ConstraintMeasure, face: Decimal) -> Decimal {
        let position = self.book.position(self.instrument).value();
        match measure {
            ConstraintMeasure::InstrumentFace => (position + face).abs(),
            ConstraintMeasure::PortfolioFace => {
                let total: Decimal = self
                    .book
                    .positions()
                    .map(|(_, face)| face.value().abs())
                    .sum();
                total - position.abs() + (position + face).abs()
            }
            ConstraintMeasure::BorrowedCash => (-self.cash_after(face)).max(Decimal::ZERO),
        }
    }

    fn breaches(&self, constraint: &ReplayConstraint, face: Decimal) -> bool {
        let after = self.measure(constraint.measure, face);
        after > constraint.ceiling.value()
            && after > self.measure(constraint.measure, Decimal::ZERO)
    }

    /// Largest magnitude in the direction of `face` this constraint allows.
    fn allowed_magnitude(&self, constraint: &ReplayConstraint, face: Decimal) -> Decimal {
        let before = self.measure(constraint.measure, Decimal::ZERO);
        let limit = constraint.ceiling.value().max(before);
        let position = self.book.position(self.instrument).value();
        let allowed = match constraint.measure {
            ConstraintMeasure::InstrumentFace | ConstraintMeasure::PortfolioFace => {
                // Moving toward zero first frees |position|, then grows the measure again.
                let unwinding =
                    !position.is_zero() && position.is_sign_negative() != face.is_sign_negative();
                let freed = if unwinding {
                    position.abs() * Decimal::TWO
                } else {
                    Decimal::ZERO
                };
                limit - before + freed
            }
            ConstraintMeasure::BorrowedCash => {
                let per_face = self.dirty_per_face + self.cost_per_face;
                if face.is_sign_negative() || per_face <= Decimal::ZERO {
                    // A smaller sale only borrows more; shrinking cannot repair it.
                    Decimal::ZERO
                } else {
                    (limit + self.book.cash().value() - self.fee_per_fill) / per_face
                }
            }
        };
        allowed
            .max(Decimal::ZERO)
            .round_dp_with_strategy(DECIMAL_SCALE, RoundingStrategy::ToZero)
    }
}

/// Checks one execution of signed `face` against every constraint.
///
/// Returns the face that may execute and, when any constraint is breached at the full size,
/// what happened: rejected if any breached constraint is declared `REJECT` (or shrinking cannot
/// repair the breach), otherwise reduced to the largest size that breaches none.
pub fn check_execution(
    constraints: &[ReplayConstraint],
    context: &ExecutionContext<'_>,
    face: ReplayAmount,
) -> (ReplayAmount, Option<ConstraintOutcome>) {
    let face = face.value();
    if face.is_zero() {
        return (ReplayAmount::ZERO, None);
    }
    let breached: Vec<&ReplayConstraint> = constraints
        .iter()
        .filter(|constraint| context.breaches(constraint, face))
        .collect();
    if breached.is_empty() {
        return (ReplayAmount::from_decimal(face), None);
    }
    let mut triggered: Vec<String> = breached
        .iter()
        .map(|constraint| constraint.limit_code.clone())
        .collect();
    triggered.sort();
    triggered.dedup();
    let rejected = |triggered| {
        (
            ReplayAmount::ZERO,
            Some(ConstraintOutcome {
                unconstrained: ReplayAmount::from_decimal(face),
                triggered,
                action: ConstraintAction::Rejected,
            }),
        )
    };
    if breached
        .iter()
        .any(|constraint| constraint.handling == ConstraintHandling::Reject)
    {
        return rejected(triggered);
    }
    let magnitude = breached
        .iter()
        .map(|constraint| context.allowed_magnitude(constraint, face))
        .fold(face.abs(), Decimal::min);
    let allowed = signed_like(face, magnitude);
    if allowed.is_zero()
        || constraints
            .iter()
            .any(|constraint| context.breaches(constraint, allowed))
    {
        return rejected(triggered);
    }
    (
        ReplayAmount::from_decimal(allowed),
        Some(ConstraintOutcome {
            unconstrained: ReplayAmount::from_decimal(face),
            triggered,
            action: ConstraintAction::Reduced,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::{Ulid, Version};

    fn bond() -> VersionRef {
        VersionRef::new(
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F20").unwrap(),
            Version::new(1).unwrap(),
        )
    }

    fn other_bond() -> VersionRef {
        VersionRef::new(
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F21").unwrap(),
            Version::new(1).unwrap(),
        )
    }

    fn amount(text: &str) -> ReplayAmount {
        ReplayAmount::parse(text).unwrap()
    }

    fn constraint(
        measure: ConstraintMeasure,
        ceiling: &str,
        handling: ConstraintHandling,
    ) -> ReplayConstraint {
        ReplayConstraint {
            limit_code: measure.code().to_owned(),
            measure,
            ceiling: amount(ceiling),
            handling,
        }
    }

    fn book(positions: &[(VersionRef, &str)], cash: &str) -> SimulatedBook {
        let mut book = SimulatedBook::default();
        for (instrument, face) in positions {
            book.add_position(instrument, amount(face).value());
        }
        book.add_cash(amount(cash).value());
        book
    }

    fn check(
        constraints: &[ReplayConstraint],
        book: &SimulatedBook,
        face: &str,
        fee: &str,
    ) -> (ReplayAmount, Option<ConstraintOutcome>) {
        let instrument = bond();
        let context = ExecutionContext {
            book,
            instrument: &instrument,
            dirty_per_face: amount("1").value(),
            cost_per_face: Decimal::ZERO,
            fee_per_fill: amount(fee).value(),
        };
        check_execution(constraints, &context, amount(face))
    }

    #[test]
    fn reducing_an_existing_excess_always_passes() {
        // 10m held against a 6m ceiling: selling 3m leaves 7m, still above, but lower.
        let limit = [constraint(
            ConstraintMeasure::InstrumentFace,
            "6000000",
            ConstraintHandling::Reject,
        )];
        let held = book(&[(bond(), "10000000")], "-10000000");
        assert_eq!(
            check(&limit, &held, "-3000000", "0"),
            (amount("-3000000"), None)
        );
        // Buying more from above the ceiling is refused.
        let (allowed, outcome) = check(&limit, &held, "1", "0");
        assert_eq!(allowed, ReplayAmount::ZERO);
        assert_eq!(outcome.unwrap().action, ConstraintAction::Rejected);
    }

    #[test]
    fn crossing_through_zero_is_reduced_on_the_far_side() {
        // Long 4m, ceiling 6m: selling 15m would end short 11m; the most allowed is 4m + 6m = 10m.
        let limit = [constraint(
            ConstraintMeasure::InstrumentFace,
            "6000000",
            ConstraintHandling::Reduce,
        )];
        let held = book(&[(bond(), "4000000")], "0");
        let (allowed, outcome) = check(&limit, &held, "-15000000", "0");
        assert_eq!(allowed, amount("-10000000"));
        assert_eq!(outcome.unwrap().action, ConstraintAction::Reduced);
    }

    #[test]
    fn portfolio_face_counts_the_other_positions() {
        // 7m of another bond, total ceiling 10m: buying 5m of this one is reduced to 3m.
        let limit = [constraint(
            ConstraintMeasure::PortfolioFace,
            "10000000",
            ConstraintHandling::Reduce,
        )];
        let held = book(&[(other_bond(), "7000000")], "0");
        assert_eq!(check(&limit, &held, "5000000", "0").0, amount("3000000"));
    }

    #[test]
    fn borrowed_cash_counts_the_fee_and_a_sale_that_borrows_cannot_be_shrunk() {
        // Cash -900, ceiling 1,000, fee 50, 1 cash per face: a buy may add 50 face at most.
        let limit = [constraint(
            ConstraintMeasure::BorrowedCash,
            "1000",
            ConstraintHandling::Reduce,
        )];
        let held = book(&[(bond(), "100")], "-900");
        assert_eq!(check(&limit, &held, "80", "50").0, amount("50"));
        // Selling 10 face brings in 10 but pays 50: borrowing grows past the ceiling, and a
        // smaller sale would only borrow more, so it is rejected even though REDUCE is declared.
        let tight = book(&[(bond(), "100")], "-990");
        let (allowed, outcome) = check(&limit, &tight, "-10", "50");
        assert_eq!(allowed, ReplayAmount::ZERO);
        assert_eq!(outcome.unwrap().action, ConstraintAction::Rejected);
    }
}
