use crate::blotter::error::{BlotterError, BlotterResult};
use crate::primitives::FixedDecimal;

/// Unsigned 256-bit integer represented as four little-endian `u64` limbs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct U256(pub [u64; 4]);

impl Ord for U256 {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.0[3], self.0[2], self.0[1], self.0[0])
            .cmp(&(other.0[3], other.0[2], other.0[1], other.0[0]))
    }
}

impl PartialOrd for U256 {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl U256 {
    pub const ZERO: Self = Self([0, 0, 0, 0]);

    #[allow(clippy::cast_possible_truncation)]
    pub const fn from_u128(value: u128) -> Self {
        let lo = value as u64;
        let hi = (value >> 64) as u64;
        Self([lo, hi, 0, 0])
    }

    pub const fn to_u128(self) -> Option<u128> {
        if self.0[2] != 0 || self.0[3] != 0 {
            return None;
        }
        Some((self.0[0] as u128) | ((self.0[1] as u128) << 64))
    }

    const fn bit(self, index: usize) -> u128 {
        let limb = index / 64;
        let shift = index % 64;
        ((self.0[limb] >> shift) & 1) as u128
    }

    fn set_bit(&mut self, index: usize) {
        let limb = index / 64;
        let shift = index % 64;
        self.0[limb] |= 1_u64 << shift;
    }
}

/// Computes the exact full 256-bit unsigned product `a * b` in safe Rust using `u64` limbs.
#[allow(clippy::cast_possible_truncation)]
pub fn mul_u128_wide(a: u128, b: u128) -> U256 {
    let a_lo = u128::from(a as u64);
    let a_hi = a >> 64;
    let b_lo = u128::from(b as u64);
    let b_hi = b >> 64;

    let p00 = a_lo * b_lo;
    let p01 = a_lo * b_hi;
    let p10 = a_hi * b_lo;
    let p11 = a_hi * b_hi;

    let l0 = p00 as u64;

    let mid = (p00 >> 64) + u128::from(p01 as u64) + u128::from(p10 as u64);
    let l1 = mid as u64;

    let hi = (mid >> 64) + (p01 >> 64) + (p10 >> 64) + u128::from(p11 as u64);
    let l2 = hi as u64;

    let top = (hi >> 64) + (p11 >> 64);
    let l3 = top as u64;

    U256([l0, l1, l2, l3])
}

/// Divides a 256-bit unsigned integer by a non-zero 128-bit unsigned divisor.
///
/// Returns `(quotient, remainder)` satisfying `num == quotient * den + remainder`
/// and `remainder < den`.
pub fn div_rem_u256_by_u128(num: U256, den: u128) -> BlotterResult<(U256, u128)> {
    if den == 0 {
        return Err(BlotterError::validation("division by zero"));
    }

    let mut quotient = U256::ZERO;
    let mut remainder = 0_u128;

    for bit_index in (0..256).rev() {
        let carry = (remainder >> 127) != 0;
        remainder = (remainder << 1) | num.bit(bit_index);
        if carry || remainder >= den {
            remainder = remainder.wrapping_sub(den);
            quotient.set_bit(bit_index);
        }
    }

    Ok((quotient, remainder))
}

/// Computes `round_half_even(a * b / den)` for unsigned `u128` operands without intermediate
/// overflow.
///
/// Tie-breaking compares `rem` against `den - rem` (never forming `2 * rem`) and breaks exact
/// half ties to an even quotient.
pub fn wide_mul_div_u128_round_half_even(a: u128, b: u128, den: u128) -> BlotterResult<u128> {
    if den == 0 {
        return Err(BlotterError::validation("denominator must be positive"));
    }
    if a == 0 || b == 0 {
        return Ok(0);
    }

    let product = mul_u128_wide(a, b);
    let (quotient_u256, remainder) = div_rem_u256_by_u128(product, den)?;
    let mut quotient = quotient_u256
        .to_u128()
        .ok_or_else(|| BlotterError::validation("wide mul-div quotient overflowed u128"))?;

    let complement = den - remainder;
    let round_up = remainder > complement || (remainder == complement && (quotient & 1) == 1);
    if round_up {
        quotient = quotient
            .checked_add(1)
            .ok_or_else(|| BlotterError::validation("rounded quotient overflowed u128"))?;
    }

    Ok(quotient)
}

/// Computes signed `round_half_even(a * b / den)` for `FixedDecimal` values where `a` and `b`
/// share the fixed `10^12` scale and `den` is a positive scaled `FixedDecimal`.
pub fn wide_mul_div_fixed_round_half_even(
    a: FixedDecimal,
    b: FixedDecimal,
    den: FixedDecimal,
) -> BlotterResult<FixedDecimal> {
    if !den.is_positive() {
        return Err(BlotterError::validation("denominator must be positive"));
    }
    let negative = (a.scaled() < 0) != (b.scaled() < 0);
    let mag = wide_mul_div_u128_round_half_even(
        a.scaled().unsigned_abs(),
        b.scaled().unsigned_abs(),
        den.scaled().unsigned_abs(),
    )?;
    signed_fixed(mag, negative)
}

/// Evaluates `B(x) = round_half_even(C * x / N)` for cumulative allocation magnitude `x`
/// (`0 <= x <= N`, `N > 0`).
pub fn cumulative_cash_magnitude(
    total_cash_abs: u128,
    cumulative_notional_abs: u128,
    total_notional_abs: u128,
) -> BlotterResult<u128> {
    if total_notional_abs == 0 {
        return Err(BlotterError::validation("total notional must be positive"));
    }
    if cumulative_notional_abs > total_notional_abs {
        return Err(BlotterError::validation(
            "cumulative allocated notional exceeds total notional",
        ));
    }
    if cumulative_notional_abs == 0 || total_cash_abs == 0 {
        return Ok(0);
    }
    if cumulative_notional_abs == total_notional_abs {
        return Ok(total_cash_abs);
    }
    wide_mul_div_u128_round_half_even(total_cash_abs, cumulative_notional_abs, total_notional_abs)
}

/// One contiguous cumulative allocation segment for either a fill or an earmark.
///
/// Each pool maintains its own independent allocation base `(base_notional, base_signed_cash)`.
/// When a reversal leaves a non-contiguous remainder pool, the prior segment is archived in
/// history and a fresh segment is started from the actual remaining `(N_rem, C_rem)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AllocationSegment {
    pub segment_index: u32,
    pub base_notional: FixedDecimal,
    pub base_signed_cash: FixedDecimal,
    pub cumulative_allocated_notional: FixedDecimal,
    pub cumulative_allocated_cash_abs: u128,
}

impl AllocationSegment {
    pub fn new(
        segment_index: u32,
        base_notional: FixedDecimal,
        base_signed_cash: FixedDecimal,
    ) -> BlotterResult<Self> {
        if segment_index == 0 || !base_notional.is_positive() {
            return Err(BlotterError::validation(
                "allocation segment requires segment_index >= 1 and positive base_notional",
            ));
        }
        Ok(Self {
            segment_index,
            base_notional,
            base_signed_cash,
            cumulative_allocated_notional: FixedDecimal::ZERO,
            cumulative_allocated_cash_abs: 0,
        })
    }

    /// Remaining unallocated notional in this segment.
    pub fn remaining_notional(&self) -> BlotterResult<FixedDecimal> {
        self.base_notional
            .checked_sub(self.cumulative_allocated_notional)
            .map_err(BlotterError::from)
    }

    /// Allocates the next slice of `slice_notional` from this segment and returns the exact
    /// signed cash slice `sign(base_signed_cash) * (B(x_i) - B(x_{i-1}))`.
    pub fn allocate_slice(&mut self, slice_notional: FixedDecimal) -> BlotterResult<FixedDecimal> {
        if !slice_notional.is_positive() {
            return Err(BlotterError::validation(
                "allocation slice notional must be positive",
            ));
        }
        let next_cum_notional = self
            .cumulative_allocated_notional
            .checked_add(slice_notional)
            .map_err(BlotterError::from)?;
        if next_cum_notional > self.base_notional {
            return Err(BlotterError::validation(
                "allocation slice exceeds segment remaining notional",
            ));
        }

        let total_cash_abs = self.base_signed_cash.scaled().unsigned_abs();
        let next_cum_cash_abs = cumulative_cash_magnitude(
            total_cash_abs,
            next_cum_notional.scaled().unsigned_abs(),
            self.base_notional.scaled().unsigned_abs(),
        )?;
        let slice_cash_abs = next_cum_cash_abs
            .checked_sub(self.cumulative_allocated_cash_abs)
            .ok_or_else(|| BlotterError::validation("cumulative allocation monotonicity failed"))?;

        self.cumulative_allocated_notional = next_cum_notional;
        self.cumulative_allocated_cash_abs = next_cum_cash_abs;

        signed_fixed(slice_cash_abs, self.base_signed_cash.scaled() < 0)
    }
}

/// Independent cumulative allocation pool carrying its active segment and archived history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AllocationPool {
    pub active_segment: Option<AllocationSegment>,
    pub segment_history: Vec<AllocationSegment>,
}

impl AllocationPool {
    pub fn new(base_notional: FixedDecimal, base_signed_cash: FixedDecimal) -> BlotterResult<Self> {
        Ok(Self {
            active_segment: Some(AllocationSegment::new(1, base_notional, base_signed_cash)?),
            segment_history: Vec::new(),
        })
    }

    pub const fn empty() -> Self {
        Self {
            active_segment: None,
            segment_history: Vec::new(),
        }
    }

    pub fn active_segment_index(&self) -> Option<u32> {
        self.active_segment
            .as_ref()
            .map(|segment| segment.segment_index)
    }

    /// Allocates `slice_notional` from the active segment.
    pub fn allocate_slice(
        &mut self,
        slice_notional: FixedDecimal,
    ) -> BlotterResult<(u32, FixedDecimal)> {
        let segment = self
            .active_segment
            .as_mut()
            .ok_or_else(|| BlotterError::validation("allocation pool has no active segment"))?;
        let index = segment.segment_index;
        let cash = segment.allocate_slice(slice_notional)?;
        Ok((index, cash))
    }

    /// Archives the current segment (if any) and opens a fresh segment anchored at the actual
    /// remaining `(remaining_notional, remaining_signed_cash)`.
    pub fn open_new_segment_from_remainder(
        &mut self,
        remaining_notional: FixedDecimal,
        remaining_signed_cash: FixedDecimal,
    ) -> BlotterResult<u32> {
        let next_index = if let Some(prev) = self.active_segment.take() {
            let next = prev
                .segment_index
                .checked_add(1)
                .ok_or_else(|| BlotterError::validation("allocation segment index overflow"))?;
            self.segment_history.push(prev);
            next
        } else {
            u32::try_from(self.segment_history.len())
                .ok()
                .and_then(|len| len.checked_add(1))
                .ok_or_else(|| BlotterError::validation("allocation segment index overflow"))?
        };

        if remaining_notional.is_positive() {
            self.active_segment = Some(AllocationSegment::new(
                next_index,
                remaining_notional,
                remaining_signed_cash,
            )?);
            Ok(next_index)
        } else {
            Ok(next_index)
        }
    }
}

pub(crate) fn signed_fixed(magnitude: u128, negative: bool) -> BlotterResult<FixedDecimal> {
    if magnitude == 0 {
        return Ok(FixedDecimal::ZERO);
    }
    if !negative {
        let value = i128::try_from(magnitude)
            .map_err(|_| BlotterError::validation("positive FixedDecimal overflow"))?;
        return Ok(FixedDecimal::from_scaled(value));
    }
    if magnitude == i128::MIN.unsigned_abs() {
        return Ok(FixedDecimal::from_scaled(i128::MIN));
    }
    let value = i128::try_from(magnitude)
        .ok()
        .and_then(i128::checked_neg)
        .ok_or_else(|| BlotterError::validation("negative FixedDecimal overflow"))?;
    Ok(FixedDecimal::from_scaled(value))
}
