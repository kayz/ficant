use chrono::{NaiveDate, TimeDelta};
use ficant_application::ports::{
    BondAnalyticsEngine, CarryRollEngine, FuturesDeliveryEngine, FuturesHedgeEngine,
    YieldCurveEngine,
};
use ficant_domain::analytics::{
    ABI_VERSION, AnalyticsError, AnalyticsMeasures, AnalyticsMode, BondAnalyticsInput,
    BondAnalyticsResult, BusinessDayConvention, CalendarRequirement, CalendarResolution,
    CouponFrequency, DayCountConvention, DerivedCashflow, FixedDecimal,
};
use ficant_domain::curves::{
    CarryRollInput, CarryRollMeasures, CarryRollResult, YieldCurveInterpolation, YieldCurvePoint,
    YieldCurveQuery,
};
use ficant_domain::futures_delivery::{
    CgbFuturesProduct, FuturesDeliverableInput, FuturesDeliveryMeasures, FuturesDeliveryResult,
};
use ficant_domain::futures_hedge::{FuturesHedgeInput, FuturesHedgeMeasures, FuturesHedgeResult};

#[derive(Clone, Copy, Debug, Default)]
pub struct NativeBondAnalyticsEngine;

#[derive(Clone, Copy, Debug, Default)]
pub struct NativeYieldCurveEngine;

#[derive(Clone, Copy, Debug, Default)]
pub struct NativeCarryRollEngine;

#[derive(Clone, Copy, Debug, Default)]
pub struct NativeFuturesDeliveryEngine;

#[derive(Clone, Copy, Debug, Default)]
pub struct NativeFuturesHedgeEngine;

impl FuturesHedgeEngine for NativeFuturesHedgeEngine {
    fn calculate(&self, input: &FuturesHedgeInput) -> Result<FuturesHedgeResult, AnalyticsError> {
        if ficant_kernel_sys::abi_version() != ABI_VERSION {
            return Err(AnalyticsError::AbiMismatch);
        }
        let (status, native) = ficant_kernel_sys::calculate_cgb_futures_hedge(
            &ficant_kernel_sys::CgbFuturesHedgeInputV1 {
                product: match input.product() {
                    CgbFuturesProduct::TwoYear => ficant_kernel_sys::CGB_FUTURES_TS,
                    CgbFuturesProduct::FiveYear => ficant_kernel_sys::CGB_FUTURES_TF,
                    CgbFuturesProduct::TenYear => ficant_kernel_sys::CGB_FUTURES_T,
                    CgbFuturesProduct::ThirtyYear => ficant_kernel_sys::CGB_FUTURES_TL,
                },
                target_dv01: decimal_to_f64(input.target_dv01())?,
                ctd_dv01_per_100: decimal_to_f64(input.ctd_dv01_per_100())?,
                conversion_factor: decimal_to_f64(input.conversion_factor())?,
            },
        );
        map_status(status)?;
        if native.status_code != ficant_kernel_sys::STATUS_OK {
            return Err(AnalyticsError::Internal);
        }
        let measures = FuturesHedgeMeasures::new(
            decimal_from_f64(native.futures_contract_dv01)?,
            decimal_from_f64(native.raw_contracts)?,
            native.recommended_contracts,
            decimal_from_f64(native.residual_dv01)?,
            decimal_from_f64(native.hedge_effectiveness)?,
        )
        .map_err(|_| AnalyticsError::Internal)?;
        Ok(FuturesHedgeResult::new(input.clone(), measures))
    }
}

impl FuturesDeliveryEngine for NativeFuturesDeliveryEngine {
    fn calculate(
        &self,
        input: &FuturesDeliverableInput,
    ) -> Result<FuturesDeliveryResult, AnalyticsError> {
        if ficant_kernel_sys::abi_version() != ABI_VERSION {
            return Err(AnalyticsError::AbiMismatch);
        }
        let terms = input.terms();
        let (status, native) = ficant_kernel_sys::analyze_cgb_futures_delivery(
            &ficant_kernel_sys::CgbFuturesDeliveryInputV1 {
                product: match input.product() {
                    CgbFuturesProduct::TwoYear => ficant_kernel_sys::CGB_FUTURES_TS,
                    CgbFuturesProduct::FiveYear => ficant_kernel_sys::CGB_FUTURES_TF,
                    CgbFuturesProduct::TenYear => ficant_kernel_sys::CGB_FUTURES_T,
                    CgbFuturesProduct::ThirtyYear => ficant_kernel_sys::CGB_FUTURES_TL,
                },
                frequency: match terms.frequency() {
                    CouponFrequency::Annual => ficant_kernel_sys::FREQUENCY_ANNUAL,
                    CouponFrequency::Semiannual => ficant_kernel_sys::FREQUENCY_SEMIANNUAL,
                },
                issue_date: epoch_days(terms.issue_date())?,
                maturity_date: epoch_days(terms.maturity_date())?,
                delivery_month_first: epoch_days(input.delivery_month_first())?,
                purchase_date: epoch_days(input.purchase_date())?,
                delivery_date: epoch_days(input.delivery_date())?,
                coupon_rate: decimal_to_f64(terms.coupon_rate())?,
                spot_clean_price: decimal_to_f64(input.spot_clean_price())?,
                futures_clean_price: decimal_to_f64(input.futures_clean_price())?,
                financing_rate: decimal_to_f64(input.financing_rate())?,
            },
        );
        map_status(status)?;
        if native.status_code != ficant_kernel_sys::STATUS_OK || !native.eligible {
            return Err(AnalyticsError::InvalidInput);
        }
        let conversion_factor = decimal_from_f64(native.conversion_factor)?;
        let purchase_accrued_interest = decimal_from_f64(native.purchase_accrued_interest)?;
        let delivery_accrued_interest = decimal_from_f64(native.delivery_accrued_interest)?;
        let interim_coupons = decimal_from_f64(native.interim_coupons)?;
        let invoice_price = decimal_from_f64(native.invoice_price)?;
        let purchase_dirty_price = decimal_from_f64(native.purchase_dirty_price)?;
        let gross_basis = decimal_from_f64(native.gross_basis)?;
        let financing_cost = decimal_from_f64(native.financing_cost)?;
        let holding_carry = decimal_from_f64(native.holding_carry)?;
        let net_basis = gross_basis
            .checked_sub(holding_carry)
            .map_err(|_| AnalyticsError::Internal)?;
        let delivery_profit = FixedDecimal::ZERO
            .checked_sub(net_basis)
            .map_err(|_| AnalyticsError::Internal)?;
        verify_native_measure(native.net_basis, net_basis)?;
        verify_native_measure(native.delivery_profit, delivery_profit)?;
        let measures = FuturesDeliveryMeasures::new(
            native.months_to_next_coupon,
            native.remaining_coupon_count,
            conversion_factor,
            purchase_accrued_interest,
            delivery_accrued_interest,
            interim_coupons,
            invoice_price,
            purchase_dirty_price,
            gross_basis,
            financing_cost,
            holding_carry,
            net_basis,
            decimal_from_f64(native.implied_repo_rate)?,
            delivery_profit,
        )
        .map_err(|_| AnalyticsError::Internal)?;
        Ok(FuturesDeliveryResult::new(input.clone(), measures))
    }
}

impl YieldCurveEngine for NativeYieldCurveEngine {
    fn interpolate(&self, query: &YieldCurveQuery) -> Result<YieldCurvePoint, AnalyticsError> {
        if ficant_kernel_sys::abi_version() != ABI_VERSION {
            return Err(AnalyticsError::AbiMismatch);
        }
        let curve = query.curve();
        let nodes = curve
            .nodes()
            .iter()
            .map(|node| {
                Ok(ficant_kernel_sys::YieldCurveNodeV1 {
                    maturity_date: epoch_days(node.maturity_date())?,
                    yield_to_maturity: decimal_to_f64(node.yield_to_maturity())?,
                })
            })
            .collect::<Result<Vec<_>, AnalyticsError>>()?;
        let input = ficant_kernel_sys::YieldCurveInputV1 {
            valuation_date: epoch_days(curve.valuation_date())?,
            interpolation: match curve.interpolation() {
                YieldCurveInterpolation::LinearYield => {
                    ficant_kernel_sys::CURVE_INTERPOLATION_LINEAR_YIELD
                }
            },
            nodes: &nodes,
        };
        let (status, result) =
            ficant_kernel_sys::interpolate_yield_curve(&input, epoch_days(query.query_date())?);
        map_status(status)?;
        if result.status_code != ficant_kernel_sys::STATUS_OK {
            return Err(AnalyticsError::Internal);
        }
        YieldCurvePoint::new(query.clone(), decimal_from_f64(result.yield_to_maturity)?)
            .map_err(|_| AnalyticsError::Internal)
    }
}

impl CarryRollEngine for NativeCarryRollEngine {
    fn calculate(&self, input: &CarryRollInput) -> Result<CarryRollResult, AnalyticsError> {
        let initial_curve_query = YieldCurveQuery::new(
            input.curve().clone(),
            input
                .initial_curve_query_date()
                .map_err(|_| AnalyticsError::InvalidInput)?,
        )
        .map_err(|_| AnalyticsError::InvalidInput)?;
        let rolled_curve_query = YieldCurveQuery::new(
            input.curve().clone(),
            input
                .rolled_curve_query_date()
                .map_err(|_| AnalyticsError::InvalidInput)?,
        )
        .map_err(|_| AnalyticsError::InvalidInput)?;
        let initial_yield = NativeYieldCurveEngine
            .interpolate(&initial_curve_query)?
            .yield_to_maturity();
        let rolled_yield = NativeYieldCurveEngine
            .interpolate(&rolled_curve_query)?
            .yield_to_maturity();
        if !initial_yield.is_positive() || !rolled_yield.is_positive() {
            return Err(AnalyticsError::InvalidInput);
        }

        let initial = NativeBondAnalyticsEngine.calculate(&bond_input(
            input,
            input.initial_settlement(),
            initial_yield,
        )?)?;
        let horizon_at_initial = NativeBondAnalyticsEngine.calculate(&bond_input(
            input,
            input.horizon_settlement(),
            initial_yield,
        )?)?;
        let horizon_at_rolled = NativeBondAnalyticsEngine.calculate(&bond_input(
            input,
            input.horizon_settlement(),
            rolled_yield,
        )?)?;
        let paid_cashflows = initial
            .cashflows()
            .iter()
            .filter(|cashflow| cashflow.payment_date() <= input.horizon_settlement())
            .try_fold(FixedDecimal::ZERO, |total, cashflow| {
                total.checked_add(cashflow.total())
            })
            .map_err(|_| AnalyticsError::Internal)?;
        let initial_dirty = initial.measures().dirty_price();
        let horizon_initial_dirty = horizon_at_initial.measures().dirty_price();
        let horizon_rolled_dirty = horizon_at_rolled.measures().dirty_price();
        let (status, native) =
            ficant_kernel_sys::decompose_carry_roll(&ficant_kernel_sys::CarryRollInputV1 {
                initial_dirty_price: decimal_to_f64(initial_dirty)?,
                horizon_dirty_at_initial_yield: decimal_to_f64(horizon_initial_dirty)?,
                horizon_dirty_at_rolled_yield: decimal_to_f64(horizon_rolled_dirty)?,
                paid_cashflows: decimal_to_f64(paid_cashflows)?,
            });
        map_status(status)?;
        if native.status_code != ficant_kernel_sys::STATUS_OK {
            return Err(AnalyticsError::Internal);
        }

        let carry = horizon_initial_dirty
            .checked_add(paid_cashflows)
            .and_then(|value| value.checked_sub(initial_dirty))
            .map_err(|_| AnalyticsError::Internal)?;
        let roll_down = horizon_rolled_dirty
            .checked_sub(horizon_initial_dirty)
            .map_err(|_| AnalyticsError::Internal)?;
        let total_return = carry
            .checked_add(roll_down)
            .map_err(|_| AnalyticsError::Internal)?;
        verify_native_measure(native.carry, carry)?;
        verify_native_measure(native.roll_down, roll_down)?;
        verify_native_measure(native.total_return, total_return)?;
        let measures = CarryRollMeasures::new(
            initial_yield,
            rolled_yield,
            initial_dirty,
            horizon_initial_dirty,
            horizon_rolled_dirty,
            paid_cashflows,
            carry,
            roll_down,
            total_return,
        )
        .map_err(|_| AnalyticsError::Internal)?;
        Ok(CarryRollResult::new(input.clone(), measures))
    }
}

fn bond_input(
    input: &CarryRollInput,
    settlement_date: NaiveDate,
    yield_to_maturity: FixedDecimal,
) -> Result<BondAnalyticsInput, AnalyticsError> {
    BondAnalyticsInput::new(
        input.owner().clone(),
        input.bond().clone(),
        input.rule_pack().clone(),
        input.snapshot().clone(),
        input.valuation_at().clone(),
        settlement_date,
        input.calendar_requirement(),
        input.calendar().clone(),
        input.terms().clone(),
        AnalyticsMode::YieldIn,
        yield_to_maturity,
    )
    .map_err(|_| AnalyticsError::InvalidInput)
}

fn verify_native_measure(native: f64, exact: FixedDecimal) -> Result<(), AnalyticsError> {
    let observed = decimal_from_f64(native)?;
    if observed.scaled().abs_diff(exact.scaled()) > 2 {
        return Err(AnalyticsError::Internal);
    }
    Ok(())
}

impl BondAnalyticsEngine for NativeBondAnalyticsEngine {
    fn calculate(&self, input: &BondAnalyticsInput) -> Result<BondAnalyticsResult, AnalyticsError> {
        if ficant_kernel_sys::abi_version() != ABI_VERSION {
            return Err(AnalyticsError::AbiMismatch);
        }
        let terms = input.terms();
        let non_business_days = epoch_days_all(input.calendar().non_business_days())?;
        let work_weekends = epoch_days_all(input.calendar().work_weekends())?;
        let bond = ficant_kernel_sys::BondInput {
            issue_date: epoch_days(terms.issue_date())?,
            maturity_date: epoch_days(terms.maturity_date())?,
            frequency: match terms.frequency() {
                CouponFrequency::Annual => ficant_kernel_sys::FREQUENCY_ANNUAL,
                CouponFrequency::Semiannual => ficant_kernel_sys::FREQUENCY_SEMIANNUAL,
            },
            day_count_convention: match terms.day_count() {
                DayCountConvention::ActActBondIsma => {
                    ficant_kernel_sys::DAY_COUNT_ACT_ACT_BOND_ISMA
                }
            },
            business_day_convention: match terms.business_day() {
                BusinessDayConvention::Following => ficant_kernel_sys::BDC_FOLLOWING,
            },
            coupon_rate: decimal_to_f64(terms.coupon_rate())?,
            face_value: decimal_to_f64(terms.face_amount())?,
        };
        let calculate = ficant_kernel_sys::CalculateInput {
            settlement_date: epoch_days(input.settlement_date())?,
            input_mode: match input.mode() {
                AnalyticsMode::YieldIn => ficant_kernel_sys::MODE_YIELD_IN,
                AnalyticsMode::PriceIn => ficant_kernel_sys::MODE_PRICE_IN,
            },
            input_value: decimal_to_f64(input.input_value())?,
            calendar_requirement: match input.calendar_requirement() {
                CalendarRequirement::ReferenceReplay => {
                    ficant_kernel_sys::CALENDAR_REQUIREMENT_REFERENCE_REPLAY
                }
                CalendarRequirement::ExactMarket => {
                    ficant_kernel_sys::CALENDAR_REQUIREMENT_EXACT_MARKET
                }
            },
            calendar_coverage_start: epoch_days(input.calendar().coverage_start())?,
            calendar_coverage_end: epoch_days(input.calendar().coverage_end())?,
            non_business_days: &non_business_days,
            work_weekends: &work_weekends,
        };
        let (status, result, cashflows) = ficant_kernel_sys::calculate(&bond, &calculate);
        map_status(status)?;
        if result.status_code != ficant_kernel_sys::STATUS_OK
            || result.cashflow_count as usize != cashflows.len()
        {
            return Err(AnalyticsError::Internal);
        }
        let resolution = match result.calendar_resolution {
            ficant_kernel_sys::CALENDAR_RESOLUTION_EXACT => CalendarResolution::Exact,
            ficant_kernel_sys::CALENDAR_RESOLUTION_PROVISIONAL_WEEKEND_ONLY => {
                CalendarResolution::ProvisionalWeekendOnly
            }
            _ => return Err(AnalyticsError::Internal),
        };
        let cashflows = cashflows
            .into_iter()
            .map(|cashflow| {
                DerivedCashflow::new(
                    cashflow.sequence,
                    date_from_epoch_days(cashflow.nominal_date)?,
                    date_from_epoch_days(cashflow.payment_date)?,
                    decimal_from_f64(cashflow.coupon)?,
                    decimal_from_f64(cashflow.principal)?,
                    decimal_from_f64(cashflow.total)?,
                )
                .map_err(|_| AnalyticsError::Internal)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let measures = AnalyticsMeasures::new(
            decimal_from_f64(result.accrued_interest)?,
            decimal_from_f64(result.clean_price)?,
            decimal_from_f64(result.yield_to_maturity)?,
            decimal_from_f64(result.macaulay_duration)?,
            decimal_from_f64(result.modified_duration)?,
            decimal_from_f64(result.convexity)?,
            decimal_from_f64(result.dv01)?,
        )
        .map_err(|_| AnalyticsError::Internal)?;
        let analytics = BondAnalyticsResult::new(input.clone(), resolution, cashflows, measures)
            .map_err(|_| AnalyticsError::Internal)?;
        let native_dirty = decimal_from_f64(result.dirty_price)?;
        if analytics
            .measures()
            .dirty_price()
            .scaled()
            .abs_diff(native_dirty.scaled())
            > 1
        {
            return Err(AnalyticsError::Internal);
        }
        Ok(analytics)
    }
}

fn map_status(status: u32) -> Result<(), AnalyticsError> {
    match status {
        ficant_kernel_sys::STATUS_OK => Ok(()),
        ficant_kernel_sys::STATUS_INVALID_ARGUMENT => Err(AnalyticsError::InvalidInput),
        ficant_kernel_sys::STATUS_ABI_MISMATCH => Err(AnalyticsError::AbiMismatch),
        ficant_kernel_sys::STATUS_BUFFER_TOO_SMALL => Err(AnalyticsError::BufferTooSmall),
        ficant_kernel_sys::STATUS_NO_BRACKET => Err(AnalyticsError::NoBracket),
        ficant_kernel_sys::STATUS_NOT_CONVERGED => Err(AnalyticsError::NotConverged),
        ficant_kernel_sys::STATUS_NON_FINITE => Err(AnalyticsError::NonFinite),
        ficant_kernel_sys::STATUS_CALENDAR_COVERAGE_MISSING => {
            Err(AnalyticsError::CalendarCoverageMissing)
        }
        _ => Err(AnalyticsError::Internal),
    }
}

fn epoch() -> NaiveDate {
    NaiveDate::from_ymd_opt(1970, 1, 1).expect("Unix epoch is a valid date")
}

fn epoch_days(date: NaiveDate) -> Result<i32, AnalyticsError> {
    i32::try_from(date.signed_duration_since(epoch()).num_days())
        .map_err(|_| AnalyticsError::InvalidInput)
}

fn epoch_days_all(dates: &[NaiveDate]) -> Result<Vec<i32>, AnalyticsError> {
    dates.iter().copied().map(epoch_days).collect()
}

fn date_from_epoch_days(days: i32) -> Result<NaiveDate, AnalyticsError> {
    epoch()
        .checked_add_signed(TimeDelta::days(i64::from(days)))
        .ok_or(AnalyticsError::Internal)
}

#[allow(clippy::cast_precision_loss)]
fn decimal_to_f64(value: FixedDecimal) -> Result<f64, AnalyticsError> {
    let converted = value.scaled() as f64 / 1_000_000_000_000_f64;
    if converted.is_finite() {
        Ok(converted)
    } else {
        Err(AnalyticsError::NonFinite)
    }
}

#[allow(clippy::cast_possible_truncation)]
fn decimal_from_f64(value: f64) -> Result<FixedDecimal, AnalyticsError> {
    const SCALE: f64 = 1_000_000_000_000_f64;
    const I128_MIN_AS_F64: f64 = -170_141_183_460_469_231_731_687_303_715_884_105_728.0;
    const I128_MAX_AS_F64: f64 = 170_141_183_460_469_231_731_687_303_715_884_105_727.0;
    if !value.is_finite() {
        return Err(AnalyticsError::NonFinite);
    }
    let scaled = (value * SCALE).round_ties_even();
    if !scaled.is_finite() || !(I128_MIN_AS_F64..=I128_MAX_AS_F64).contains(&scaled) {
        return Err(AnalyticsError::NonFinite);
    }
    Ok(FixedDecimal::from_scaled(scaled as i128))
}
