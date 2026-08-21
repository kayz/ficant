use ficant_domain::DomainErrorCode;
use ficant_domain::analytics::FixedDecimal;
use ficant_domain::research::{SensitivityDirection, bond_position_key_rate_dv01, key_rate_dv01};

const SCALE: i128 = 1_000_000_000_000;

#[test]
fn odd_central_difference_rounds_once_after_signed_position_scaling() {
    let base = fixed(0);
    let up = fixed(0);
    let down = fixed(5);

    assert_eq!(
        key_rate_dv01(
            base,
            up,
            down,
            FixedDecimal::ONE,
            SensitivityDirection::Central,
        ),
        Err(DomainErrorCode::InvalidValue),
        "the frozen registered-face helper remains exact-only"
    );
    assert_eq!(
        bond_position_key_rate_dv01(
            base,
            up,
            down,
            FixedDecimal::ONE,
            SensitivityDirection::Central,
            fixed(700_000_000_000),
            FixedDecimal::ONE,
        )
        .unwrap(),
        fixed(2),
        "2.5 registered-face units times 0.7 must round from the final 1.75, not from 2.5"
    );
}

#[test]
fn final_rounding_is_signed_half_even_and_exact_cases_do_not_move() {
    for (difference, quantity, expected) in [
        (5, SCALE, 2),
        (7, SCALE, 4),
        (5, -SCALE, -2),
        (7, -SCALE, -4),
        (4, SCALE, 2),
        (4, -SCALE, -2),
    ] {
        assert_eq!(
            bond_position_key_rate_dv01(
                fixed(0),
                fixed(0),
                fixed(difference),
                FixedDecimal::ONE,
                SensitivityDirection::Central,
                fixed(quantity),
                FixedDecimal::ONE,
            )
            .unwrap(),
            fixed(expected)
        );
    }
}

#[test]
fn direction_formula_and_zero_quantity_are_preserved() {
    let base = fixed(10);
    let up = fixed(6);
    let down = fixed(14);
    for direction in [
        SensitivityDirection::Central,
        SensitivityDirection::Up,
        SensitivityDirection::Down,
    ] {
        assert_eq!(
            bond_position_key_rate_dv01(
                base,
                up,
                down,
                FixedDecimal::ONE,
                direction,
                FixedDecimal::ONE,
                FixedDecimal::ONE,
            )
            .unwrap(),
            fixed(4)
        );
        assert_eq!(
            bond_position_key_rate_dv01(
                base,
                up,
                down,
                FixedDecimal::ONE,
                direction,
                FixedDecimal::ZERO,
                FixedDecimal::ONE,
            )
            .unwrap(),
            FixedDecimal::ZERO
        );
    }
}

#[test]
fn cross_cancellation_avoids_false_overflow_but_true_overflow_fails_closed() {
    assert_eq!(
        bond_position_key_rate_dv01(
            fixed(0),
            fixed(0),
            fixed(i128::MAX),
            fixed(i128::MAX),
            SensitivityDirection::Central,
            fixed(2 * SCALE),
            FixedDecimal::ONE,
        )
        .unwrap(),
        FixedDecimal::ONE
    );

    assert_eq!(
        bond_position_key_rate_dv01(
            fixed(i128::MAX),
            fixed(0),
            fixed(0),
            fixed(1),
            SensitivityDirection::Up,
            fixed(i128::MAX),
            fixed(1),
        ),
        Err(DomainErrorCode::InvalidValue)
    );
}

#[test]
fn zero_or_negative_denominators_fail_closed() {
    for bump in [fixed(0), fixed(-1)] {
        assert_eq!(
            bond_position_key_rate_dv01(
                fixed(0),
                fixed(0),
                fixed(1),
                bump,
                SensitivityDirection::Central,
                FixedDecimal::ONE,
                FixedDecimal::ONE,
            ),
            Err(DomainErrorCode::InvalidValue)
        );
    }
    for face in [fixed(0), fixed(-1)] {
        assert_eq!(
            bond_position_key_rate_dv01(
                fixed(0),
                fixed(0),
                fixed(1),
                FixedDecimal::ONE,
                SensitivityDirection::Central,
                FixedDecimal::ONE,
                face,
            ),
            Err(DomainErrorCode::InvalidValue)
        );
    }
}

const fn fixed(value: i128) -> FixedDecimal {
    FixedDecimal::from_scaled(value)
}
