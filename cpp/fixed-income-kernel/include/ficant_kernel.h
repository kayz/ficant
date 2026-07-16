#ifndef FICANT_KERNEL_H
#define FICANT_KERNEL_H

#include <stdint.h>

/* ── ABI version ─────────────────────────────────────────────────── */
#define FICANT_KERNEL_ABI_VERSION UINT32_C(1)

/* ── Status codes ────────────────────────────────────────────────── */
#define FICANT_KERNEL_STATUS_OK                       UINT32_C(0)
#define FICANT_KERNEL_STATUS_INVALID_ARGUMENT         UINT32_C(1)
#define FICANT_KERNEL_STATUS_ABI_MISMATCH             UINT32_C(2)
#define FICANT_KERNEL_STATUS_BUFFER_TOO_SMALL         UINT32_C(3)
#define FICANT_KERNEL_STATUS_NO_BRACKET               UINT32_C(4)
#define FICANT_KERNEL_STATUS_NOT_CONVERGED            UINT32_C(5)
#define FICANT_KERNEL_STATUS_NON_FINITE               UINT32_C(6)
#define FICANT_KERNEL_STATUS_CALENDAR_COVERAGE_MISSING UINT32_C(7)
#define FICANT_KERNEL_STATUS_INTERNAL_ERROR           UINT32_C(255)

/* ── Coupon frequency ────────────────────────────────────────────── */
#define FICANT_KERNEL_FREQUENCY_ANNUAL     UINT32_C(1)
#define FICANT_KERNEL_FREQUENCY_SEMIANNUAL UINT32_C(2)

/* ── Day-count convention ────────────────────────────────────────── */
#define FICANT_KERNEL_DAY_COUNT_ACT_ACT_BOND_ISMA UINT32_C(1)

/* ── Business-day convention ─────────────────────────────────────── */
#define FICANT_KERNEL_BDC_FOLLOWING UINT32_C(1)

/* ── Input mode ──────────────────────────────────────────────────── */
#define FICANT_KERNEL_MODE_YIELD_IN UINT32_C(1)
#define FICANT_KERNEL_MODE_PRICE_IN UINT32_C(2)

/* ── Calendar requirement ────────────────────────────────────────── */
#define FICANT_KERNEL_CALENDAR_REQUIREMENT_REFERENCE_REPLAY UINT32_C(1)
#define FICANT_KERNEL_CALENDAR_REQUIREMENT_EXACT_MARKET     UINT32_C(2)

/* ── Calendar resolution (output) ────────────────────────────────── */
#define FICANT_KERNEL_CALENDAR_RESOLUTION_EXACT                  UINT32_C(1)
#define FICANT_KERNEL_CALENDAR_RESOLUTION_PROVISIONAL_WEEKEND_ONLY UINT32_C(2)

/* ── Visibility ──────────────────────────────────────────────────── */
#if defined(_WIN32)
#if defined(FICANT_KERNEL_BUILD)
#define FICANT_KERNEL_API __declspec(dllexport)
#else
#define FICANT_KERNEL_API __declspec(dllimport)
#endif
#elif defined(__GNUC__)
#define FICANT_KERNEL_API __attribute__((visibility("default")))
#else
#define FICANT_KERNEL_API
#endif

#ifdef __cplusplus
extern "C" {
#endif

/* ── Bond-descriptor input struct ────────────────────────────────── */
typedef struct {
    uint32_t struct_size;
    uint32_t abi_version;
    int32_t  issue_date;               /* epoch days */
    int32_t  maturity_date;            /* epoch days */
    uint32_t frequency;
    uint32_t day_count_convention;
    uint32_t business_day_convention;
    double   coupon_rate;              /* decimal, e.g. 0.0130 */
    double   face_value;               /* per 100 notional */
} ficant_kernel_bond_input_v1;

/* ── Calculate-request input struct ──────────────────────────────── */
typedef struct {
    uint32_t       struct_size;
    uint32_t       abi_version;
    int32_t        settlement_date;        /* epoch days */
    uint32_t       input_mode;
    double         input_value;            /* YIELD_IN: decimal yield;
                                              PRICE_IN: clean price per 100 */
    uint32_t       calendar_requirement;
    int32_t        calendar_coverage_start;  /* epoch days, inclusive */
    int32_t        calendar_coverage_end;    /* epoch days, inclusive */
    const int32_t* non_business_days;        /* sorted ascending, no duplicates */
    uint32_t       non_business_days_count;
    const int32_t* work_weekends;            /* sorted ascending, no duplicates */
    uint32_t       work_weekends_count;
} ficant_kernel_calculate_input_v1;

/* ── Result struct ───────────────────────────────────────────────── */
typedef struct {
    uint32_t struct_size;
    uint32_t abi_version;
    uint32_t cashflow_count;
    uint32_t calendar_resolution;
    uint32_t status_code;
    double   accrued_interest;
    double   clean_price;
    double   dirty_price;
    double   yield_to_maturity;
    double   macaulay_duration;
    double   modified_duration;
    double   convexity;
    double   dv01;
} ficant_kernel_result_v1;

/* ── Cashflow entry struct ───────────────────────────────────────── */
typedef struct {
    uint32_t struct_size;
    uint32_t abi_version;
    uint32_t sequence;          /* 1-based */
    int32_t  nominal_date;      /* epoch days */
    int32_t  payment_date;      /* epoch days, after business-day adjustment */
    double   coupon;
    double   principal;
    double   total;             /* coupon + principal */
} ficant_kernel_cashflow_v1;

/* ── ABI functions ───────────────────────────────────────────────── */

/** Return the ABI version this shared library was compiled with. */
FICANT_KERNEL_API uint32_t ficant_kernel_abi_version(void)
#if defined(__cplusplus) && __cplusplus >= 201103L
    noexcept
#endif
;

/**
 * Calculate bond analytics.
 *
 * Two-call pattern for cashflows:
 *   Call 1 — pass cashflows==NULL, cashflow_capacity==0.
 *            result->cashflow_count is set to the required buffer size.
 *            May return BUFFER_TOO_SMALL (or OK if zero cashflows).
 *   Call 2 — pass a caller-owned buffer of at least cashflow_count entries.
 *            Cashflows are written in payment-date order.
 *
 * All pointers must be non-NULL (except cashflows on the sizing call).
 * struct_size and abi_version in each input struct must match.
 *
 * @return status code (0 = OK).
 */
FICANT_KERNEL_API uint32_t ficant_kernel_calculate_bond_v1(
    const ficant_kernel_bond_input_v1*   bond_input,
    const ficant_kernel_calculate_input_v1* calc_input,
    ficant_kernel_result_v1*             result,
    ficant_kernel_cashflow_v1*           cashflows,
    uint32_t                             cashflow_capacity)
#if defined(__cplusplus) && __cplusplus >= 201103L
    noexcept
#endif
;

#ifdef __cplusplus
}
#endif

#endif /* FICANT_KERNEL_H */
