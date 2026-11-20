use ficant_domain::analytics::{
    AnalyticsError, BondAnalyticsInput, BondAnalyticsResult, BondTaxBasis, CouponFrequency,
    DerivedCashflow, FixedDecimal,
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
    tax_basis: BondTaxBasis,
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

    #[must_use]
    pub const fn tax_basis(&self) -> BondTaxBasis {
        self.tax_basis
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
        if input.tax_basis() != BondTaxBasis::PreTax {
            return Err(invalid());
        }
        let result = self.engine.calculate(input).map_err(map_analytics_error)?;
        result.validate_against(input).map_err(map_domain_error)?;
        if result.tax_basis() != BondTaxBasis::PreTax {
            return Err(invalid());
        }
        Ok(result)
    }

    pub fn execute_for_mode(
        &self,
        input: &BondAnalyticsInput,
        mode: BondTaxBasis,
        treatment: Option<&CouponTaxTreatment>,
    ) -> ApplicationResult<(BondAnalyticsResult, Option<TaxAdjustedBondAnalytics>)> {
        match (mode, treatment) {
            (BondTaxBasis::PreTax, None) => Ok((self.execute(input)?, None)),
            (BondTaxBasis::TaxAdjusted, Some(treatment)) => {
                let (pre_tax, after_tax) = self.execute_with_tax(input, treatment)?;
                Ok((pre_tax, Some(after_tax)))
            }
            (BondTaxBasis::PreTax, Some(_)) | (BondTaxBasis::TaxAdjusted, None) => Err(invalid()),
        }
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
        let after_tax = self
            .engine
            .calculate(&after_tax_input)
            .map_err(map_analytics_error)?;
        after_tax
            .validate_against(&after_tax_input)
            .map_err(map_domain_error)?;
        if after_tax.tax_basis() != BondTaxBasis::TaxAdjusted {
            return Err(invalid());
        }

        Ok((
            pre_tax,
            TaxAdjustedBondAnalytics {
                cashflows,
                yield_to_maturity: after_tax.measures().yield_to_maturity(),
                claim_scope: treatment.claim_scope(),
                tax_basis: after_tax.tax_basis(),
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
