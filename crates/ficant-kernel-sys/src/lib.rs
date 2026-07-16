use core::ffi::c_uint;

pub const ABI_VERSION: u32 = 1;

pub const STATUS_OK: u32 = 0;
pub const STATUS_INVALID_ARGUMENT: u32 = 1;
pub const STATUS_ABI_MISMATCH: u32 = 2;
pub const STATUS_BUFFER_TOO_SMALL: u32 = 3;
pub const STATUS_NO_BRACKET: u32 = 4;
pub const STATUS_NOT_CONVERGED: u32 = 5;
pub const STATUS_NON_FINITE: u32 = 6;
pub const STATUS_CALENDAR_COVERAGE_MISSING: u32 = 7;
pub const STATUS_INTERNAL_ERROR: u32 = 255;

pub const FREQUENCY_ANNUAL: u32 = 1;
pub const FREQUENCY_SEMIANNUAL: u32 = 2;
pub const DAY_COUNT_ACT_ACT_BOND_ISMA: u32 = 1;
pub const BDC_FOLLOWING: u32 = 1;
pub const MODE_YIELD_IN: u32 = 1;
pub const MODE_PRICE_IN: u32 = 2;
pub const CALENDAR_REQUIREMENT_REFERENCE_REPLAY: u32 = 1;
pub const CALENDAR_REQUIREMENT_EXACT_MARKET: u32 = 2;
pub const CALENDAR_RESOLUTION_EXACT: u32 = 1;
pub const CALENDAR_RESOLUTION_PROVISIONAL_WEEKEND_ONLY: u32 = 2;

const MAX_CASHFLOWS: usize = 4_096;

#[derive(Clone, Copy, Debug)]
pub struct BondInput {
    pub issue_date: i32,
    pub maturity_date: i32,
    pub frequency: u32,
    pub day_count_convention: u32,
    pub business_day_convention: u32,
    pub coupon_rate: f64,
    pub face_value: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct CalculateInput<'a> {
    pub settlement_date: i32,
    pub input_mode: u32,
    pub input_value: f64,
    pub calendar_requirement: u32,
    pub calendar_coverage_start: i32,
    pub calendar_coverage_end: i32,
    pub non_business_days: &'a [i32],
    pub work_weekends: &'a [i32],
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ResultV1 {
    pub cashflow_count: u32,
    pub calendar_resolution: u32,
    pub status_code: u32,
    pub accrued_interest: f64,
    pub clean_price: f64,
    pub dirty_price: f64,
    pub yield_to_maturity: f64,
    pub macaulay_duration: f64,
    pub modified_duration: f64,
    pub convexity: f64,
    pub dv01: f64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CashflowV1 {
    pub sequence: u32,
    pub nominal_date: i32,
    pub payment_date: i32,
    pub coupon: f64,
    pub principal: f64,
    pub total: f64,
}

#[repr(C)]
struct RawBondInputV1 {
    struct_size: u32,
    abi_version: u32,
    issue_date: i32,
    maturity_date: i32,
    frequency: u32,
    day_count_convention: u32,
    business_day_convention: u32,
    coupon_rate: f64,
    face_value: f64,
}

#[repr(C)]
struct RawCalculateInputV1 {
    struct_size: u32,
    abi_version: u32,
    settlement_date: i32,
    input_mode: u32,
    input_value: f64,
    calendar_requirement: u32,
    calendar_coverage_start: i32,
    calendar_coverage_end: i32,
    non_business_days: *const i32,
    non_business_days_count: u32,
    work_weekends: *const i32,
    work_weekends_count: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RawResultV1 {
    struct_size: u32,
    abi_version: u32,
    cashflow_count: u32,
    calendar_resolution: u32,
    status_code: u32,
    accrued_interest: f64,
    clean_price: f64,
    dirty_price: f64,
    yield_to_maturity: f64,
    macaulay_duration: f64,
    modified_duration: f64,
    convexity: f64,
    dv01: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RawCashflowV1 {
    struct_size: u32,
    abi_version: u32,
    sequence: u32,
    nominal_date: i32,
    payment_date: i32,
    coupon: f64,
    principal: f64,
    total: f64,
}

unsafe extern "C" {
    fn ficant_kernel_abi_version() -> c_uint;
    fn ficant_kernel_calculate_bond_v1(
        bond_input: *const RawBondInputV1,
        calculate_input: *const RawCalculateInputV1,
        result: *mut RawResultV1,
        cashflows: *mut RawCashflowV1,
        cashflow_capacity: u32,
    ) -> c_uint;
}

#[must_use]
pub fn abi_version() -> u32 {
    // SAFETY: This function has no arguments and the statically linked kernel implements the
    // frozen v1 signature.
    unsafe { ficant_kernel_abi_version() }
}

#[must_use]
pub fn calculate(bond: &BondInput, input: &CalculateInput<'_>) -> (u32, ResultV1, Vec<CashflowV1>) {
    let Ok(non_business_days_count) = u32::try_from(input.non_business_days.len()) else {
        return (STATUS_INVALID_ARGUMENT, ResultV1::default(), Vec::new());
    };
    let Ok(work_weekends_count) = u32::try_from(input.work_weekends.len()) else {
        return (STATUS_INVALID_ARGUMENT, ResultV1::default(), Vec::new());
    };
    let raw_bond = RawBondInputV1 {
        struct_size: size_of_u32::<RawBondInputV1>(),
        abi_version: ABI_VERSION,
        issue_date: bond.issue_date,
        maturity_date: bond.maturity_date,
        frequency: bond.frequency,
        day_count_convention: bond.day_count_convention,
        business_day_convention: bond.business_day_convention,
        coupon_rate: bond.coupon_rate,
        face_value: bond.face_value,
    };
    let raw_input = RawCalculateInputV1 {
        struct_size: size_of_u32::<RawCalculateInputV1>(),
        abi_version: ABI_VERSION,
        settlement_date: input.settlement_date,
        input_mode: input.input_mode,
        input_value: input.input_value,
        calendar_requirement: input.calendar_requirement,
        calendar_coverage_start: input.calendar_coverage_start,
        calendar_coverage_end: input.calendar_coverage_end,
        non_business_days: input.non_business_days.as_ptr(),
        non_business_days_count,
        work_weekends: input.work_weekends.as_ptr(),
        work_weekends_count,
    };
    let mut raw_result = initialized_result();
    // SAFETY: all pointers reference live, correctly laid-out values. The null cashflow pointer and
    // zero capacity are the ABI's documented sizing call.
    let sizing_status = unsafe {
        ficant_kernel_calculate_bond_v1(
            &raw const raw_bond,
            &raw const raw_input,
            &raw mut raw_result,
            core::ptr::null_mut(),
            0,
        )
    };
    if sizing_status != STATUS_BUFFER_TOO_SMALL && sizing_status != STATUS_OK {
        return (sizing_status, convert_result(raw_result), Vec::new());
    }
    let required = raw_result.cashflow_count as usize;
    if required > MAX_CASHFLOWS {
        return (
            STATUS_INTERNAL_ERROR,
            convert_result(raw_result),
            Vec::new(),
        );
    }
    if required == 0 {
        return (sizing_status, convert_result(raw_result), Vec::new());
    }

    let mut raw_cashflows = vec![raw_cashflow(); required];
    raw_result = initialized_result();
    let Ok(capacity) = u32::try_from(required) else {
        return (
            STATUS_INTERNAL_ERROR,
            convert_result(raw_result),
            Vec::new(),
        );
    };
    // SAFETY: raw_cashflows owns `required` initialized entries and its pointer remains stable for
    // the duration of the call. The capacity exactly matches the allocated entry count.
    let status = unsafe {
        ficant_kernel_calculate_bond_v1(
            &raw const raw_bond,
            &raw const raw_input,
            &raw mut raw_result,
            raw_cashflows.as_mut_ptr(),
            capacity,
        )
    };
    if status != STATUS_OK || raw_result.cashflow_count as usize > required {
        return (status, convert_result(raw_result), Vec::new());
    }
    let cashflows = raw_cashflows
        .into_iter()
        .take(raw_result.cashflow_count as usize)
        .map(convert_cashflow)
        .collect();
    (status, convert_result(raw_result), cashflows)
}

fn size_of_u32<T>() -> u32 {
    u32::try_from(core::mem::size_of::<T>()).unwrap_or(u32::MAX)
}

fn initialized_result() -> RawResultV1 {
    RawResultV1 {
        struct_size: size_of_u32::<RawResultV1>(),
        abi_version: ABI_VERSION,
        ..RawResultV1::default()
    }
}

fn raw_cashflow() -> RawCashflowV1 {
    RawCashflowV1 {
        struct_size: size_of_u32::<RawCashflowV1>(),
        abi_version: ABI_VERSION,
        ..RawCashflowV1::default()
    }
}

fn convert_result(value: RawResultV1) -> ResultV1 {
    ResultV1 {
        cashflow_count: value.cashflow_count,
        calendar_resolution: value.calendar_resolution,
        status_code: value.status_code,
        accrued_interest: value.accrued_interest,
        clean_price: value.clean_price,
        dirty_price: value.dirty_price,
        yield_to_maturity: value.yield_to_maturity,
        macaulay_duration: value.macaulay_duration,
        modified_duration: value.modified_duration,
        convexity: value.convexity,
        dv01: value.dv01,
    }
}

fn convert_cashflow(value: RawCashflowV1) -> CashflowV1 {
    CashflowV1 {
        sequence: value.sequence,
        nominal_date: value.nominal_date,
        payment_date: value.payment_date,
        coupon: value.coupon,
        principal: value.principal,
        total: value.total,
    }
}

#[cfg(test)]
mod tests {
    use super::{RawBondInputV1, RawCalculateInputV1, RawCashflowV1, RawResultV1};

    #[test]
    fn rust_layout_matches_frozen_c_header() {
        assert_eq!(core::mem::size_of::<RawBondInputV1>(), 48);
        assert_eq!(core::mem::size_of::<RawCalculateInputV1>(), 72);
        assert_eq!(core::mem::size_of::<RawResultV1>(), 88);
        assert_eq!(core::mem::size_of::<RawCashflowV1>(), 48);

        assert_eq!(core::mem::offset_of!(RawBondInputV1, struct_size), 0);
        assert_eq!(core::mem::offset_of!(RawBondInputV1, coupon_rate), 32);
        assert_eq!(core::mem::offset_of!(RawBondInputV1, face_value), 40);
        assert_eq!(core::mem::offset_of!(RawResultV1, status_code), 16);
        assert_eq!(core::mem::offset_of!(RawResultV1, accrued_interest), 24);
        assert_eq!(core::mem::offset_of!(RawCashflowV1, sequence), 8);
        assert_eq!(core::mem::offset_of!(RawCashflowV1, coupon), 24);
    }
}
