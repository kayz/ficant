use chrono::{NaiveDate, TimeDelta};
use ficant_application::ports::BondAnalyticsEngine;
use ficant_domain::analytics::{
    ABI_VERSION, AnalyticsError, AnalyticsMeasures, AnalyticsMode, BondAnalyticsInput,
    BondAnalyticsResult, BusinessDayConvention, CalendarRequirement, CalendarResolution,
    CouponFrequency, DayCountConvention, DerivedCashflow, FixedDecimal,
};

#[derive(Clone, Copy, Debug, Default)]
pub struct NativeBondAnalyticsEngine;

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
