//! L1 constraint shapes (ADR-0026).
//!
//! A [`ConstraintSet`] is parsed from the exact regulatory `RulePack` a Subject version binds; it
//! says what each limit measures. The ceilings are Subject state and come from
//! [`SubjectStateSnapshot`]; [`ConstraintSet::bind`] joins the two by `limit_code`. Neither side
//! carries market rule values in code (ADR-0013).

use crate::primitives::{DecimalValue, VersionRef};
use crate::subject::SubjectStateSnapshot;
use crate::{DomainErrorCode, DomainResult};

/// What a constraint measures on a book.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConstraintMeasure {
    /// Absolute face amount of each single instrument position.
    InstrumentFace,
    /// Sum of absolute face amounts across all positions.
    PortfolioFace,
    /// Cash borrowed to fund positions: `max(0, -cash)`.
    BorrowedCash,
}

impl ConstraintMeasure {
    pub const fn code(self) -> &'static str {
        match self {
            Self::InstrumentFace => "INSTRUMENT_FACE",
            Self::PortfolioFace => "PORTFOLIO_FACE",
            Self::BorrowedCash => "BORROWED_CASH",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ConstraintDefinition {
    limit_code: String,
    measure: ConstraintMeasure,
}

impl ConstraintDefinition {
    pub fn new(limit_code: impl Into<String>, measure: ConstraintMeasure) -> DomainResult<Self> {
        let limit_code = limit_code.into();
        if limit_code.trim().is_empty() || limit_code != limit_code.trim() {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            limit_code,
            measure,
        })
    }

    pub fn limit_code(&self) -> &str {
        &self.limit_code
    }

    pub const fn measure(&self) -> ConstraintMeasure {
        self.measure
    }
}

/// The constraints of one exact regulatory `RulePack` version, ordered by `limit_code`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConstraintSet {
    rule_pack: VersionRef,
    definitions: Vec<ConstraintDefinition>,
}

impl ConstraintSet {
    /// Fails on an empty set or a repeated `limit_code`.
    pub fn new(
        rule_pack: VersionRef,
        mut definitions: Vec<ConstraintDefinition>,
    ) -> DomainResult<Self> {
        definitions.sort();
        if definitions.is_empty()
            || definitions
                .windows(2)
                .any(|pair| pair[0].limit_code == pair[1].limit_code)
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            rule_pack,
            definitions,
        })
    }

    pub fn rule_pack(&self) -> &VersionRef {
        &self.rule_pack
    }

    pub fn definitions(&self) -> &[ConstraintDefinition] {
        &self.definitions
    }

    /// Joins every definition with its ceiling in `state`.
    ///
    /// Fails when the snapshot belongs to another Subject than `subject`, a definition has no
    /// ceiling, or a ceiling is negative. Ceilings the set does not define are ignored.
    pub fn bind(
        &self,
        subject: &VersionRef,
        state: &SubjectStateSnapshot,
    ) -> DomainResult<Vec<BoundConstraint>> {
        if state.subject_ref() != subject {
            return Err(DomainErrorCode::BrokenLineage);
        }
        self.definitions
            .iter()
            .map(|definition| {
                let ceiling = state
                    .limit_ceilings()
                    .iter()
                    .find(|ceiling| ceiling.limit_code() == definition.limit_code)
                    .ok_or(DomainErrorCode::BrokenLineage)?
                    .ceiling();
                if ceiling.coefficient().starts_with('-') {
                    return Err(DomainErrorCode::InvalidValue);
                }
                Ok(BoundConstraint {
                    definition: definition.clone(),
                    ceiling: ceiling.clone(),
                })
            })
            .collect()
    }
}

/// A constraint with the ceiling the Subject's state snapshot gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundConstraint {
    definition: ConstraintDefinition,
    ceiling: DecimalValue,
}

impl BoundConstraint {
    pub fn limit_code(&self) -> &str {
        self.definition.limit_code()
    }

    pub const fn measure(&self) -> ConstraintMeasure {
        self.definition.measure()
    }

    pub fn ceiling(&self) -> &DecimalValue {
        &self.ceiling
    }
}
