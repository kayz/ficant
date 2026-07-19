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
pub const CURVE_INTERPOLATION_LINEAR_YIELD: u32 = 1;
pub const CGB_FUTURES_TS: u32 = 1;
pub const CGB_FUTURES_TF: u32 = 2;
pub const CGB_FUTURES_T: u32 = 3;
pub const CGB_FUTURES_TL: u32 = 4;

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

#[derive(Clone, Copy, Debug)]
pub struct YieldCurveNodeV1 {
    pub maturity_date: i32,
    pub yield_to_maturity: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct YieldCurveInputV1<'a> {
    pub valuation_date: i32,
    pub interpolation: u32,
    pub nodes: &'a [YieldCurveNodeV1],
}

#[derive(Clone, Copy, Debug, Default)]
pub struct YieldCurveResultV1 {
    pub status_code: u32,
    pub yield_to_maturity: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct CarryRollInputV1 {
    pub initial_dirty_price: f64,
    pub horizon_dirty_at_initial_yield: f64,
    pub horizon_dirty_at_rolled_yield: f64,
    pub paid_cashflows: f64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CarryRollResultV1 {
    pub status_code: u32,
    pub carry: f64,
    pub roll_down: f64,
    pub total_return: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct CgbFuturesDeliveryInputV1 {
    pub product: u32,
    pub frequency: u32,
    pub issue_date: i32,
    pub maturity_date: i32,
    pub delivery_month_first: i32,
    pub purchase_date: i32,
    pub delivery_date: i32,
    pub months_to_next_coupon: u32,
    pub remaining_coupon_count: u32,
    pub coupon_rate: f64,
    pub spot_clean_price: f64,
    pub purchase_accrued_interest: f64,
    pub delivery_accrued_interest: f64,
    pub interim_coupons: f64,
    pub futures_clean_price: f64,
    pub financing_rate: f64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CgbFuturesDeliveryResultV1 {
    pub status_code: u32,
    pub eligible: bool,
    pub conversion_factor: f64,
    pub invoice_price: f64,
    pub purchase_dirty_price: f64,
    pub gross_basis: f64,
    pub financing_cost: f64,
    pub holding_carry: f64,
    pub net_basis: f64,
    pub implied_repo_rate: f64,
    pub delivery_profit: f64,
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

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RawYieldCurveNodeV1 {
    struct_size: u32,
    abi_version: u32,
    maturity_date: i32,
    reserved: u32,
    yield_to_maturity: f64,
}

#[repr(C)]
struct RawYieldCurveInputV1 {
    struct_size: u32,
    abi_version: u32,
    valuation_date: i32,
    interpolation: u32,
    nodes: *const RawYieldCurveNodeV1,
    node_count: u32,
    reserved: u32,
}

#[repr(C)]
struct RawYieldCurveQueryV1 {
    struct_size: u32,
    abi_version: u32,
    query_date: i32,
    reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RawYieldCurveResultV1 {
    struct_size: u32,
    abi_version: u32,
    status_code: u32,
    reserved: u32,
    yield_to_maturity: f64,
}

#[repr(C)]
struct RawCarryRollInputV1 {
    struct_size: u32,
    abi_version: u32,
    initial_dirty_price: f64,
    horizon_dirty_at_initial_yield: f64,
    horizon_dirty_at_rolled_yield: f64,
    paid_cashflows: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RawCarryRollResultV1 {
    struct_size: u32,
    abi_version: u32,
    status_code: u32,
    reserved: u32,
    carry: f64,
    roll_down: f64,
    total_return: f64,
}

#[repr(C)]
struct RawCgbFuturesDeliveryInputV1 {
    struct_size: u32,
    abi_version: u32,
    product: u32,
    frequency: u32,
    issue_date: i32,
    maturity_date: i32,
    delivery_month_first: i32,
    purchase_date: i32,
    delivery_date: i32,
    months_to_next_coupon: u32,
    remaining_coupon_count: u32,
    reserved: u32,
    coupon_rate: f64,
    spot_clean_price: f64,
    purchase_accrued_interest: f64,
    delivery_accrued_interest: f64,
    interim_coupons: f64,
    futures_clean_price: f64,
    financing_rate: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RawCgbFuturesDeliveryResultV1 {
    struct_size: u32,
    abi_version: u32,
    status_code: u32,
    eligible: u32,
    conversion_factor: f64,
    invoice_price: f64,
    purchase_dirty_price: f64,
    gross_basis: f64,
    financing_cost: f64,
    holding_carry: f64,
    net_basis: f64,
    implied_repo_rate: f64,
    delivery_profit: f64,
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
    fn ficant_kernel_interpolate_yield_curve_v1(
        curve_input: *const RawYieldCurveInputV1,
        query: *const RawYieldCurveQueryV1,
        result: *mut RawYieldCurveResultV1,
    ) -> c_uint;
    fn ficant_kernel_decompose_carry_roll_v1(
        input: *const RawCarryRollInputV1,
        result: *mut RawCarryRollResultV1,
    ) -> c_uint;
    fn ficant_kernel_analyze_cgb_futures_delivery_v1(
        input: *const RawCgbFuturesDeliveryInputV1,
        result: *mut RawCgbFuturesDeliveryResultV1,
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

#[must_use]
pub fn interpolate_yield_curve(
    curve: &YieldCurveInputV1<'_>,
    query_date: i32,
) -> (u32, YieldCurveResultV1) {
    let Ok(node_count) = u32::try_from(curve.nodes.len()) else {
        return (STATUS_INVALID_ARGUMENT, YieldCurveResultV1::default());
    };
    let raw_nodes = curve
        .nodes
        .iter()
        .map(|node| RawYieldCurveNodeV1 {
            struct_size: size_of_u32::<RawYieldCurveNodeV1>(),
            abi_version: ABI_VERSION,
            maturity_date: node.maturity_date,
            reserved: 0,
            yield_to_maturity: node.yield_to_maturity,
        })
        .collect::<Vec<_>>();
    let raw_curve = RawYieldCurveInputV1 {
        struct_size: size_of_u32::<RawYieldCurveInputV1>(),
        abi_version: ABI_VERSION,
        valuation_date: curve.valuation_date,
        interpolation: curve.interpolation,
        nodes: raw_nodes.as_ptr(),
        node_count,
        reserved: 0,
    };
    let raw_query = RawYieldCurveQueryV1 {
        struct_size: size_of_u32::<RawYieldCurveQueryV1>(),
        abi_version: ABI_VERSION,
        query_date,
        reserved: 0,
    };
    let mut raw_result = RawYieldCurveResultV1 {
        struct_size: size_of_u32::<RawYieldCurveResultV1>(),
        abi_version: ABI_VERSION,
        ..RawYieldCurveResultV1::default()
    };
    // SAFETY: all pointers reference live, correctly laid-out values for the duration of the call.
    // `raw_nodes` is not mutated, and the C++ implementation reads exactly `node_count` entries.
    let status = unsafe {
        ficant_kernel_interpolate_yield_curve_v1(
            &raw const raw_curve,
            &raw const raw_query,
            &raw mut raw_result,
        )
    };
    (
        status,
        YieldCurveResultV1 {
            status_code: raw_result.status_code,
            yield_to_maturity: raw_result.yield_to_maturity,
        },
    )
}

#[must_use]
pub fn decompose_carry_roll(input: &CarryRollInputV1) -> (u32, CarryRollResultV1) {
    let raw_input = RawCarryRollInputV1 {
        struct_size: size_of_u32::<RawCarryRollInputV1>(),
        abi_version: ABI_VERSION,
        initial_dirty_price: input.initial_dirty_price,
        horizon_dirty_at_initial_yield: input.horizon_dirty_at_initial_yield,
        horizon_dirty_at_rolled_yield: input.horizon_dirty_at_rolled_yield,
        paid_cashflows: input.paid_cashflows,
    };
    let mut raw_result = RawCarryRollResultV1 {
        struct_size: size_of_u32::<RawCarryRollResultV1>(),
        abi_version: ABI_VERSION,
        ..RawCarryRollResultV1::default()
    };
    // SAFETY: both pointers reference live, correctly-laid-out values and the C++ function does
    // not retain either pointer after returning.
    let status =
        unsafe { ficant_kernel_decompose_carry_roll_v1(&raw const raw_input, &raw mut raw_result) };
    (
        status,
        CarryRollResultV1 {
            status_code: raw_result.status_code,
            carry: raw_result.carry,
            roll_down: raw_result.roll_down,
            total_return: raw_result.total_return,
        },
    )
}

#[must_use]
pub fn analyze_cgb_futures_delivery(
    input: &CgbFuturesDeliveryInputV1,
) -> (u32, CgbFuturesDeliveryResultV1) {
    let raw_input = RawCgbFuturesDeliveryInputV1 {
        struct_size: size_of_u32::<RawCgbFuturesDeliveryInputV1>(),
        abi_version: ABI_VERSION,
        product: input.product,
        frequency: input.frequency,
        issue_date: input.issue_date,
        maturity_date: input.maturity_date,
        delivery_month_first: input.delivery_month_first,
        purchase_date: input.purchase_date,
        delivery_date: input.delivery_date,
        months_to_next_coupon: input.months_to_next_coupon,
        remaining_coupon_count: input.remaining_coupon_count,
        reserved: 0,
        coupon_rate: input.coupon_rate,
        spot_clean_price: input.spot_clean_price,
        purchase_accrued_interest: input.purchase_accrued_interest,
        delivery_accrued_interest: input.delivery_accrued_interest,
        interim_coupons: input.interim_coupons,
        futures_clean_price: input.futures_clean_price,
        financing_rate: input.financing_rate,
    };
    let mut raw_result = RawCgbFuturesDeliveryResultV1 {
        struct_size: size_of_u32::<RawCgbFuturesDeliveryResultV1>(),
        abi_version: ABI_VERSION,
        ..RawCgbFuturesDeliveryResultV1::default()
    };
    // SAFETY: both pointers reference live, correctly-laid-out values for the full call and the
    // C++ implementation does not retain them.
    let status = unsafe {
        ficant_kernel_analyze_cgb_futures_delivery_v1(&raw const raw_input, &raw mut raw_result)
    };
    (
        status,
        CgbFuturesDeliveryResultV1 {
            status_code: raw_result.status_code,
            eligible: raw_result.eligible == 1,
            conversion_factor: raw_result.conversion_factor,
            invoice_price: raw_result.invoice_price,
            purchase_dirty_price: raw_result.purchase_dirty_price,
            gross_basis: raw_result.gross_basis,
            financing_cost: raw_result.financing_cost,
            holding_carry: raw_result.holding_carry,
            net_basis: raw_result.net_basis,
            implied_repo_rate: raw_result.implied_repo_rate,
            delivery_profit: raw_result.delivery_profit,
        },
    )
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
    use super::{
        RawBondInputV1, RawCalculateInputV1, RawCarryRollInputV1, RawCarryRollResultV1,
        RawCashflowV1, RawCgbFuturesDeliveryInputV1, RawCgbFuturesDeliveryResultV1, RawResultV1,
        RawYieldCurveInputV1, RawYieldCurveNodeV1, RawYieldCurveQueryV1, RawYieldCurveResultV1,
    };

    #[test]
    fn rust_layout_matches_frozen_c_header() {
        assert_eq!(core::mem::size_of::<RawBondInputV1>(), 48);
        assert_eq!(core::mem::size_of::<RawCalculateInputV1>(), 72);
        assert_eq!(core::mem::size_of::<RawResultV1>(), 88);
        assert_eq!(core::mem::size_of::<RawCashflowV1>(), 48);
        assert_eq!(core::mem::size_of::<RawYieldCurveNodeV1>(), 24);
        assert_eq!(core::mem::size_of::<RawYieldCurveInputV1>(), 32);
        assert_eq!(core::mem::size_of::<RawYieldCurveQueryV1>(), 16);
        assert_eq!(core::mem::size_of::<RawYieldCurveResultV1>(), 24);
        assert_eq!(core::mem::size_of::<RawCarryRollInputV1>(), 40);
        assert_eq!(core::mem::size_of::<RawCarryRollResultV1>(), 40);
        assert_eq!(core::mem::size_of::<RawCgbFuturesDeliveryInputV1>(), 104);
        assert_eq!(core::mem::size_of::<RawCgbFuturesDeliveryResultV1>(), 88);

        assert_eq!(core::mem::offset_of!(RawBondInputV1, struct_size), 0);
        assert_eq!(core::mem::offset_of!(RawBondInputV1, coupon_rate), 32);
        assert_eq!(core::mem::offset_of!(RawBondInputV1, face_value), 40);
        assert_eq!(core::mem::offset_of!(RawResultV1, status_code), 16);
        assert_eq!(core::mem::offset_of!(RawResultV1, accrued_interest), 24);
        assert_eq!(core::mem::offset_of!(RawCashflowV1, sequence), 8);
        assert_eq!(core::mem::offset_of!(RawCashflowV1, coupon), 24);
        assert_eq!(
            core::mem::offset_of!(RawYieldCurveNodeV1, yield_to_maturity),
            16
        );
        assert_eq!(core::mem::offset_of!(RawYieldCurveInputV1, nodes), 16);
        assert_eq!(
            core::mem::offset_of!(RawYieldCurveResultV1, yield_to_maturity),
            16
        );
        assert_eq!(
            core::mem::offset_of!(RawCgbFuturesDeliveryInputV1, coupon_rate),
            48
        );
        assert_eq!(
            core::mem::offset_of!(RawCgbFuturesDeliveryResultV1, conversion_factor),
            16
        );
        assert_eq!(
            core::mem::offset_of!(RawCarryRollInputV1, initial_dirty_price),
            8
        );
        assert_eq!(core::mem::offset_of!(RawCarryRollResultV1, carry), 16);
    }
}
