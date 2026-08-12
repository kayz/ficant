use crate::{DomainErrorCode, DomainResult};

pub const DECIMAL_SCALE: u32 = 12;
pub(crate) const FIXED_DECIMAL_FACTOR: i128 = 1_000_000_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FixedDecimal(i128);

impl FixedDecimal {
    pub const ZERO: Self = Self(0);
    pub const ONE: Self = Self(FIXED_DECIMAL_FACTOR);

    #[must_use]
    pub const fn from_scaled(value: i128) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn scaled(self) -> i128 {
        self.0
    }

    #[must_use]
    pub const fn is_positive(self) -> bool {
        self.0 > 0
    }

    #[must_use]
    pub const fn is_non_negative(self) -> bool {
        self.0 >= 0
    }

    pub fn checked_add(self, other: Self) -> DomainResult<Self> {
        self.0
            .checked_add(other.0)
            .map(Self)
            .ok_or(DomainErrorCode::InvalidValue)
    }

    pub fn checked_sub(self, other: Self) -> DomainResult<Self> {
        self.0
            .checked_sub(other.0)
            .map(Self)
            .ok_or(DomainErrorCode::InvalidValue)
    }

    /// Multiplies two fixed decimals without applying an implicit market rounding rule.
    ///
    /// Values that cannot be represented exactly at the fixed scale fail closed.
    pub fn checked_mul(self, other: Self) -> DomainResult<Self> {
        let product = self
            .0
            .checked_mul(other.0)
            .ok_or(DomainErrorCode::InvalidValue)?;
        if product % FIXED_DECIMAL_FACTOR != 0 {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self(product / FIXED_DECIMAL_FACTOR))
    }
}
