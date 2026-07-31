use std::cmp::Ordering;

use rust_decimal::Decimal;

use crate::primitives::{Ulid, Version};
use crate::{DomainErrorCode, DomainResult};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnitRef {
    unit_id: Ulid,
    version: Version,
}

impl UnitRef {
    pub fn new(unit_id: Ulid, version: Version) -> Self {
        Self { unit_id, version }
    }

    pub fn unit_id(&self) -> &Ulid {
        &self.unit_id
    }

    pub fn version(&self) -> Version {
        self.version
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecimalValue {
    coefficient: String,
    scale: u32,
    unit: UnitRef,
}

impl DecimalValue {
    pub fn new(coefficient: impl Into<String>, scale: u32, unit: UnitRef) -> DomainResult<Self> {
        const MAX_PRECISION: usize = 28;
        const MAX_SCALE: u32 = 28;

        let coefficient = coefficient.into();
        let (negative, unsigned) = match coefficient.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, coefficient.strip_prefix('+').unwrap_or(&coefficient)),
        };
        if unsigned.is_empty()
            || !unsigned.bytes().all(|byte| byte.is_ascii_digit())
            || unsigned.len() > MAX_PRECISION
            || scale > MAX_SCALE
        {
            return Err(DomainErrorCode::InvalidValue);
        }

        let mut digits = unsigned.trim_start_matches('0').to_owned();
        if digits.is_empty() {
            digits.push('0');
        }
        let mut normalized_scale = scale;
        while normalized_scale > 0 && digits.ends_with('0') {
            digits.pop();
            normalized_scale -= 1;
        }
        if digits.is_empty() {
            digits.push('0');
        }
        let coefficient = if negative && digits != "0" {
            format!("-{digits}")
        } else {
            digits
        };
        coefficient
            .parse::<i128>()
            .map_err(|_| DomainErrorCode::InvalidValue)?;

        Ok(Self {
            coefficient,
            scale: normalized_scale,
            unit,
        })
    }

    pub fn coefficient(&self) -> &str {
        &self.coefficient
    }

    pub fn scale(&self) -> u32 {
        self.scale
    }

    pub fn unit(&self) -> &UnitRef {
        &self.unit
    }

    pub fn is_positive(&self) -> bool {
        self.coefficient
            .parse::<i128>()
            .is_ok_and(|coefficient| coefficient > 0)
    }

    /// Adds two values only when their exact `UnitRef` agrees.
    ///
    /// # Errors
    ///
    /// Returns `InvalidUnit` for different units and `InvalidValue` for decimal overflow.
    pub fn checked_add(&self, other: &Self) -> DomainResult<Self> {
        if self.unit != other.unit {
            return Err(DomainErrorCode::InvalidUnit);
        }
        let sum = self
            .as_decimal()
            .checked_add(other.as_decimal())
            .ok_or(DomainErrorCode::InvalidValue)?;
        Self::new(sum.mantissa().to_string(), sum.scale(), self.unit.clone())
    }

    pub(crate) fn compare(&self, other: &Self) -> DomainResult<Ordering> {
        if self.unit != other.unit {
            return Err(DomainErrorCode::InvalidUnit);
        }
        Ok(self.as_decimal().cmp(&other.as_decimal()))
    }

    fn as_decimal(&self) -> Decimal {
        let coefficient = self
            .coefficient
            .parse::<i128>()
            .expect("validated decimal coefficient must fit in i128");
        Decimal::from_i128_with_scale(coefficient, self.scale)
    }
}
