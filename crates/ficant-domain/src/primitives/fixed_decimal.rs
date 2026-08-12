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

    /// Multiplies the represented value by an integer without changing scale.
    pub fn checked_mul_integer(self, multiplier: i128) -> DomainResult<Self> {
        self.0
            .checked_mul(multiplier)
            .map(Self)
            .ok_or(DomainErrorCode::InvalidValue)
    }

    /// Divides two fixed decimals and rounds the result to the fixed scale using
    /// round-half-to-even.
    ///
    /// The long-division implementation never forms `self * scale`, so valid
    /// large operands are not rejected because of an intermediate overflow.
    pub fn checked_div_round_ties_even(self, divisor: Self) -> DomainResult<Self> {
        if divisor.0 == 0 {
            return Err(DomainErrorCode::InvalidValue);
        }

        let negative = (self.0 < 0) != (divisor.0 < 0);
        let numerator = self.0.unsigned_abs();
        let denominator = divisor.0.unsigned_abs();
        let factor = FIXED_DECIMAL_FACTOR as u128;

        let integer = numerator / denominator;
        let mut remainder = numerator % denominator;
        let mut magnitude = integer
            .checked_mul(factor)
            .ok_or(DomainErrorCode::InvalidValue)?;
        let mut fractional = 0_u128;
        for _ in 0..DECIMAL_SCALE {
            let (digit, next_remainder) = decimal_digit(remainder, denominator);
            fractional = fractional
                .checked_mul(10)
                .and_then(|value| value.checked_add(u128::from(digit)))
                .ok_or(DomainErrorCode::InvalidValue)?;
            remainder = next_remainder;
        }
        magnitude = magnitude
            .checked_add(fractional)
            .ok_or(DomainErrorCode::InvalidValue)?;

        let distance_to_denominator = denominator - remainder;
        let round_up = remainder > distance_to_denominator
            || (remainder == distance_to_denominator && magnitude % 2 == 1);
        if round_up {
            magnitude = magnitude
                .checked_add(1)
                .ok_or(DomainErrorCode::InvalidValue)?;
        }

        signed_magnitude(magnitude, negative).map(Self)
    }
}

fn decimal_digit(remainder: u128, denominator: u128) -> (u8, u128) {
    let mut digit = 0_u8;
    let mut accumulated = 0_u128;
    for _ in 0..10 {
        // `accumulated + remainder` would overflow for valid u128 operands.
        // Compare against the complement and subtract the denominator first.
        if accumulated >= denominator - remainder {
            accumulated -= denominator - remainder;
            digit += 1;
        } else {
            accumulated += remainder;
        }
    }
    (digit, accumulated)
}

fn signed_magnitude(magnitude: u128, negative: bool) -> DomainResult<i128> {
    if !negative {
        return i128::try_from(magnitude).map_err(|_| DomainErrorCode::InvalidValue);
    }
    if magnitude == i128::MIN.unsigned_abs() {
        return Ok(i128::MIN);
    }
    i128::try_from(magnitude)
        .ok()
        .and_then(i128::checked_neg)
        .ok_or(DomainErrorCode::InvalidValue)
}
