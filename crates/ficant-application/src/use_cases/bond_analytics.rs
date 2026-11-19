use ficant_domain::analytics::{
    AnalyticsError, BondAnalyticsInput, BondAnalyticsResult, CouponFrequency, DerivedCashflow,
    FixedDecimal,
};

use crate::ports::{
    ApplicationResult, BondAnalyticsEngine, CouponTaxClaimScope, CouponTaxTreatment,
};
use crate::{ApplicationError, ApplicationErrorCategory, map_domain_error};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaxAdjustedBondAnalytics {
    cashflows: Vec<DerivedCashflow>,
    yield_to_maturity: FixedDecimal,
    claim_scope: CouponTaxClaimScope,
}

impl TaxAdjustedBondAnalytics {
    #[must_use]
    pub fn cashflows(&self) -> &[DerivedCashflow] {
        &self.cashflows
    }

    #[must_use]
    pub const fn yield_to_maturity(&self) -> FixedDecimal {
        self.yield_to_maturity
    }

    #[must_use]
    pub const fn claim_scope(&self) -> CouponTaxClaimScope {
        self.claim_scope
    }
}

pub struct CalculateBondAnalytics<'a> {
    engine: &'a dyn BondAnalyticsEngine,
}

impl<'a> CalculateBondAnalytics<'a> {
    #[must_use]
    pub const fn new(engine: &'a dyn BondAnalyticsEngine) -> Self {
        Self { engine }
    }

    pub fn execute(&self, input: &BondAnalyticsInput) -> ApplicationResult<BondAnalyticsResult> {
        self.engine.calculate(input).map_err(map_analytics_error)
    }

    pub fn execute_with_tax(
        &self,
        input: &BondAnalyticsInput,
        treatment: &CouponTaxTreatment,
    ) -> ApplicationResult<(BondAnalyticsResult, TaxAdjustedBondAnalytics)> {
        let pre_tax = self.execute(input)?;
        let cashflows = pre_tax
            .cashflows()
            .iter()
            .map(|cf| {
                let coupon = treatment.adjust_coupon(cf.coupon())?;
                let total = coupon
                    .checked_add(cf.principal())
                    .map_err(map_domain_error)?;
                DerivedCashflow::new(
                    cf.sequence(),
                    cf.nominal_date(),
                    cf.payment_date(),
                    coupon,
                    cf.principal(),
                    total,
                )
                .map_err(|_| invalid())
            })
            .collect::<ApplicationResult<Vec<_>>>()?;

        let gross_coupon = pre_tax
            .cashflows()
            .first()
            .map(DerivedCashflow::coupon)
            .ok_or_else(invalid)?;
        if pre_tax
            .cashflows()
            .iter()
            .any(|cf| cf.coupon() != gross_coupon)
        {
            return Err(invalid());
        }
        let periods_per_year = match input.terms().frequency() {
            CouponFrequency::Annual => 1,
            CouponFrequency::Semiannual => 2,
        };
        let after_tax_rate = treatment
            .adjust_coupon(gross_coupon)?
            .checked_mul_integer(periods_per_year)
            .and_then(|value| value.checked_div_round_ties_even(input.terms().face_amount()))
            .map_err(map_domain_error)?;

        let terms = input
            .terms()
            .with_coupon_rate(after_tax_rate)
            .map_err(map_domain_error)?;
        let after_tax_input = input
            .with_terms_and_price_in(terms, pre_tax.measures().clean_price())
            .map_err(map_domain_error)?;
        let after_tax = self.execute(&after_tax_input)?;

        Ok((
            pre_tax,
            TaxAdjustedBondAnalytics {
                cashflows,
                yield_to_maturity: after_tax.measures().yield_to_maturity(),
                claim_scope: treatment.claim_scope(),
            },
        ))
    }
}

#[must_use]
pub const fn map_analytics_error(error: AnalyticsError) -> ApplicationError {
    match error {
        AnalyticsError::InvalidInput
        | AnalyticsError::NoBracket
        | AnalyticsError::NotConverged
        | AnalyticsError::NonFinite
        | AnalyticsError::CalendarCoverageMissing => {
            ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
        }
        AnalyticsError::AbiMismatch | AnalyticsError::BufferTooSmall | AnalyticsError::Internal => {
            ApplicationError::new(ApplicationErrorCategory::StateConflict, false)
        }
    }
}

const fn invalid() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}
