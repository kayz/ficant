use chrono::NaiveDate;
use ficant_domain::analytics::FixedDecimal;
use ficant_domain::futures_delivery::{FuturesDeliverableInput, FuturesDeliveryBasketResult};

use crate::ports::{
    ApplicationResult, CouponTaxClaimScope, CouponTaxTreatment, FuturesDeliveryEngine,
};
use crate::use_cases::bond_analytics::map_analytics_error;
use crate::{ApplicationError, ApplicationErrorCategory, map_domain_error};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubjectCandidateMeasures {
    pub funding_adjusted_irr: FixedDecimal,
    pub tax_adjusted_interim_coupons: FixedDecimal,
    pub subject_tax_adjusted_irr: FixedDecimal,
    pub claim_scope: CouponTaxClaimScope,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubjectDeliveryBasketAnalysis {
    pub basket: FuturesDeliveryBasketResult,
    pub candidate_adjustments: Vec<SubjectCandidateMeasures>,
    pub subject_ctd_index: usize,
}

pub struct CalculateFuturesDeliveryBasket<'a> {
    engine: &'a dyn FuturesDeliveryEngine,
}

impl<'a> CalculateFuturesDeliveryBasket<'a> {
    #[must_use]
    pub const fn new(engine: &'a dyn FuturesDeliveryEngine) -> Self {
        Self { engine }
    }

    pub fn execute(
        &self,
        inputs: &[FuturesDeliverableInput],
    ) -> ApplicationResult<FuturesDeliveryBasketResult> {
        if inputs.is_empty() {
            return Err(invalid());
        }
        let mut results = Vec::with_capacity(inputs.len());
        for input in inputs {
            results.push(self.engine.calculate(input).map_err(map_analytics_error)?);
        }
        let ctd_index = select_market_ctd(&results)?;
        FuturesDeliveryBasketResult::new(results, ctd_index).map_err(|_| invalid())
    }

    pub fn execute_for_subject(
        &self,
        inputs: &[FuturesDeliverableInput],
        annual_financing_rate: FixedDecimal,
        treatments: &[CouponTaxTreatment],
    ) -> ApplicationResult<(
        FuturesDeliveryBasketResult,
        Vec<SubjectCandidateMeasures>,
        usize,
    )> {
        if inputs.is_empty() || inputs.len() != treatments.len() {
            return Err(invalid());
        }
        let basket = self.execute(inputs)?;
        let mut adjustments = Vec::with_capacity(treatments.len());
        let mut subject_rates = Vec::with_capacity(treatments.len());

        for (candidate, treatment) in basket.candidates().iter().zip(treatments) {
            let value = candidate.measures();
            let funding_adjusted_irr = value
                .implied_repo_rate()
                .checked_sub(annual_financing_rate)
                .map_err(map_domain_error)?;
            let tax_adjusted_interim_coupons = treatment.adjust_coupon(value.interim_coupons())?;
            let subject_tax_adjusted_irr = subject_delivery_irr(
                value.invoice_price(),
                tax_adjusted_interim_coupons,
                value.purchase_dirty_price(),
                candidate.input().purchase_date(),
                candidate.input().delivery_date(),
            )?;
            subject_rates.push(subject_tax_adjusted_irr);
            adjustments.push(SubjectCandidateMeasures {
                funding_adjusted_irr,
                tax_adjusted_interim_coupons,
                subject_tax_adjusted_irr,
                claim_scope: treatment.claim_scope(),
            });
        }
        let subject_ctd_index = select_subject_ctd(&basket, &subject_rates)?;
        Ok((basket, adjustments, subject_ctd_index))
    }
}

pub fn subject_delivery_irr(
    invoice_price: FixedDecimal,
    tax_adjusted_interim_coupons: FixedDecimal,
    purchase_dirty_price: FixedDecimal,
    purchase_date: NaiveDate,
    delivery_date: NaiveDate,
) -> ApplicationResult<FixedDecimal> {
    let days = (delivery_date - purchase_date).num_days();
    if days <= 0 {
        return Err(invalid());
    }
    invoice_price
        .checked_add(tax_adjusted_interim_coupons)
        .and_then(|value| value.checked_div_round_ties_even(purchase_dirty_price))
        .and_then(|value| value.checked_sub(FixedDecimal::ONE))
        .and_then(|value| value.checked_mul_integer(365))
        .and_then(|value| {
            value.checked_div_round_ties_even(FixedDecimal::from_scaled(
                i128::from(days) * 1_000_000_000_000,
            ))
        })
        .map_err(map_domain_error)
}

fn select_market_ctd(
    candidates: &[ficant_domain::futures_delivery::FuturesDeliveryResult],
) -> ApplicationResult<usize> {
    if candidates.is_empty() {
        return Err(invalid());
    }
    let mut best = 0;
    for index in 1..candidates.len() {
        let candidate = candidates[index].measures();
        let incumbent = candidates[best].measures();
        let candidate_id = candidates[index].input().bond().version_ref().id();
        let incumbent_id = candidates[best].input().bond().version_ref().id();
        if candidate.implied_repo_rate() > incumbent.implied_repo_rate()
            || (candidate.implied_repo_rate() == incumbent.implied_repo_rate()
                && (candidate.net_basis() < incumbent.net_basis()
                    || (candidate.net_basis() == incumbent.net_basis()
                        && candidate_id < incumbent_id)))
        {
            best = index;
        }
    }
    Ok(best)
}

fn select_subject_ctd(
    result: &FuturesDeliveryBasketResult,
    subject_rates: &[FixedDecimal],
) -> ApplicationResult<usize> {
    if result.candidates().len() != subject_rates.len() || subject_rates.is_empty() {
        return Err(invalid());
    }
    let mut best = 0;
    for index in 1..subject_rates.len() {
        let candidate = result.candidates()[index].measures();
        let incumbent = result.candidates()[best].measures();
        let candidate_id = result.candidates()[index].input().bond().version_ref().id();
        let incumbent_id = result.candidates()[best].input().bond().version_ref().id();
        if subject_rates[index] > subject_rates[best]
            || (subject_rates[index] == subject_rates[best]
                && (candidate.net_basis() < incumbent.net_basis()
                    || (candidate.net_basis() == incumbent.net_basis()
                        && candidate_id < incumbent_id)))
        {
            best = index;
        }
    }
    Ok(best)
}

const fn invalid() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}
