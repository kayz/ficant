use crate::market::{FactSource, require_text};
use crate::primitives::{DecimalValue, MarketTime, OwnerRef, Ulid, VersionRef};
use crate::{DomainErrorCode, DomainResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Valuation {
    valuation_id: Ulid,
    instrument: VersionRef,
    owner: OwnerRef,
    source: FactSource,
    valuation_at: MarketTime,
    method: String,
    rule_pack: VersionRef,
    values: Vec<DecimalValue>,
    value_roles: Vec<ValuationValueRole>,
    supersedes_id: Option<Ulid>,
}

/// The financial measure carried by one externally supplied valuation value.
///
/// The domain deliberately has no unspecified variant. An omitted public role
/// vector is normalized by [`Valuation::new`] to `Price` for every value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValuationValueRole {
    Price,
    Yield,
    RemainingYears,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValuationInput {
    pub valuation_id: Ulid,
    pub instrument: VersionRef,
    pub owner: OwnerRef,
    pub source: FactSource,
    pub valuation_at: MarketTime,
    pub method: String,
    pub rule_pack: VersionRef,
    pub values: Vec<DecimalValue>,
    pub supersedes_id: Option<Ulid>,
}

impl Valuation {
    pub fn new(input: ValuationInput) -> DomainResult<Self> {
        Self::new_with_value_roles(input, Vec::new())
    }

    /// Creates a valuation with explicit per-value measure roles.
    ///
    /// # Errors
    ///
    /// Returns `InvalidValue` when the value list is empty or an explicit role
    /// vector does not have exactly one role for every value.
    pub fn new_with_value_roles(
        input: ValuationInput,
        value_roles: Vec<ValuationValueRole>,
    ) -> DomainResult<Self> {
        let ValuationInput {
            valuation_id,
            instrument,
            owner,
            source,
            valuation_at,
            method,
            rule_pack,
            values,
            supersedes_id,
        } = input;
        require_text(&method)?;
        if values.is_empty() {
            return Err(DomainErrorCode::InvalidValue);
        }
        let value_roles = if value_roles.is_empty() {
            vec![ValuationValueRole::Price; values.len()]
        } else if value_roles.len() == values.len() {
            value_roles
        } else {
            return Err(DomainErrorCode::InvalidValue);
        };
        Ok(Self {
            valuation_id,
            instrument,
            owner,
            source,
            valuation_at,
            method,
            rule_pack,
            values,
            value_roles,
            supersedes_id,
        })
    }

    pub fn id(&self) -> &Ulid {
        &self.valuation_id
    }

    pub fn instrument(&self) -> &VersionRef {
        &self.instrument
    }

    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    pub fn source(&self) -> &FactSource {
        &self.source
    }

    pub fn valuation_at(&self) -> &MarketTime {
        &self.valuation_at
    }

    pub fn method(&self) -> &str {
        &self.method
    }

    pub fn rule_pack(&self) -> &VersionRef {
        &self.rule_pack
    }

    pub fn values(&self) -> &[DecimalValue] {
        &self.values
    }

    pub fn value_roles(&self) -> &[ValuationValueRole] {
        &self.value_roles
    }

    /// Returns whether this valuation needs the typed v2 canonical/storage
    /// encoding. All-PRICE input is canonically identical to legacy input.
    pub fn has_typed_value_roles(&self) -> bool {
        self.value_roles
            .iter()
            .any(|role| *role != ValuationValueRole::Price)
    }

    pub fn supersedes_id(&self) -> Option<&Ulid> {
        self.supersedes_id.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveDate, TimeZone, Utc};

    use super::*;
    use crate::market::FactSource;
    use crate::primitives::{UnitRef, Version};

    #[test]
    fn omitted_and_explicit_all_price_roles_normalize_identically() {
        let input = valuation_input(vec![decimal("10125", 2), decimal("10130", 2)]);
        let legacy = Valuation::new(input.clone()).unwrap();
        let explicit = Valuation::new_with_value_roles(
            input,
            vec![ValuationValueRole::Price, ValuationValueRole::Price],
        )
        .unwrap();

        assert_eq!(legacy, explicit);
        assert_eq!(legacy.value_roles(), &[ValuationValueRole::Price; 2]);
        assert!(!legacy.has_typed_value_roles());
    }

    #[test]
    fn typed_roles_require_exact_cardinality_and_remain_bound_to_values() {
        let input = valuation_input(vec![decimal("10125", 2), decimal("2575", 4)]);
        assert_eq!(
            Valuation::new_with_value_roles(input.clone(), vec![ValuationValueRole::Price])
                .unwrap_err(),
            DomainErrorCode::InvalidValue
        );

        let typed = Valuation::new_with_value_roles(
            input,
            vec![ValuationValueRole::Price, ValuationValueRole::Yield],
        )
        .unwrap();
        assert_eq!(
            typed.value_roles(),
            &[ValuationValueRole::Price, ValuationValueRole::Yield]
        );
        assert!(typed.has_typed_value_roles());
    }

    fn valuation_input(values: Vec<DecimalValue>) -> ValuationInput {
        ValuationInput {
            valuation_id: id('V'),
            instrument: VersionRef::new(id('K'), Version::new(1).unwrap()),
            owner: OwnerRef::new(id('T'), id('Y')),
            source: FactSource::new("test-source", "valuation", 1).unwrap(),
            valuation_at: MarketTime::new(
                Utc.with_ymd_and_hms(2026, 8, 21, 1, 0, 0).unwrap(),
                "Asia/Shanghai",
                NaiveDate::from_ymd_opt(2026, 8, 21).unwrap(),
            )
            .unwrap(),
            method: "external".to_owned(),
            rule_pack: VersionRef::new(id('R'), Version::new(1).unwrap()),
            values,
            supersedes_id: None,
        }
    }

    fn decimal(coefficient: &str, scale: u32) -> DecimalValue {
        DecimalValue::new(
            coefficient,
            scale,
            UnitRef::new(id('P'), Version::new(1).unwrap()),
        )
        .unwrap()
    }

    fn id(suffix: char) -> Ulid {
        Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
    }
}
