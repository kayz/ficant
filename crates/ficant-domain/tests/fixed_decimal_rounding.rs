use ficant_domain::DomainErrorCode;
use ficant_domain::primitives::FixedDecimal;

const SCALE: i128 = 1_000_000_000_000;

#[test]
fn division_rounds_signed_values_to_twelve_places_with_ties_to_even() {
    assert_eq!(
        fixed(3 * SCALE)
            .checked_div_round_ties_even(fixed(1_060_000_000_000))
            .unwrap(),
        fixed(2_830_188_679_245)
    );
    assert_eq!(
        fixed(SCALE)
            .checked_div_round_ties_even(fixed(3 * SCALE))
            .unwrap(),
        fixed(333_333_333_333)
    );

    // These ratios land exactly halfway between adjacent scaled integers.
    assert_eq!(
        fixed(5)
            .checked_div_round_ties_even(fixed(2 * SCALE))
            .unwrap(),
        fixed(2)
    );
    assert_eq!(
        fixed(7)
            .checked_div_round_ties_even(fixed(2 * SCALE))
            .unwrap(),
        fixed(4)
    );
    assert_eq!(
        fixed(-5)
            .checked_div_round_ties_even(fixed(2 * SCALE))
            .unwrap(),
        fixed(-2)
    );
    assert_eq!(
        fixed(-7)
            .checked_div_round_ties_even(fixed(2 * SCALE))
            .unwrap(),
        fixed(-4)
    );
}

#[test]
fn exact_integer_multiplication_preserves_scale_and_fails_on_overflow() {
    assert_eq!(
        fixed(1_234_000_000_000).checked_mul_integer(365).unwrap(),
        fixed(450_410_000_000_000)
    );
    assert_eq!(
        FixedDecimal::from_scaled(i128::MAX).checked_mul_integer(2),
        Err(DomainErrorCode::InvalidValue)
    );
    assert_eq!(
        FixedDecimal::from_scaled(i128::MIN).checked_mul_integer(-1),
        Err(DomainErrorCode::InvalidValue)
    );
}

#[test]
fn division_rejects_zero_and_unrepresentable_results_without_panicking() {
    assert_eq!(
        FixedDecimal::ONE.checked_div_round_ties_even(FixedDecimal::ZERO),
        Err(DomainErrorCode::InvalidValue)
    );
    assert_eq!(
        FixedDecimal::from_scaled(i128::MIN)
            .checked_div_round_ties_even(FixedDecimal::from_scaled(-SCALE)),
        Err(DomainErrorCode::InvalidValue)
    );
    assert_eq!(
        FixedDecimal::from_scaled(i128::MIN)
            .checked_div_round_ties_even(FixedDecimal::ONE)
            .unwrap(),
        FixedDecimal::from_scaled(i128::MIN)
    );
}

const fn fixed(value: i128) -> FixedDecimal {
    FixedDecimal::from_scaled(value)
}
