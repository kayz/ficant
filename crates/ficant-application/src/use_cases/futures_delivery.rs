use ficant_domain::DomainErrorCode;
use ficant_domain::futures_delivery::{
    FuturesDeliverableInput, FuturesDeliveryBasketResult, FuturesDeliveryResult,
};

use crate::map_domain_error;
use crate::ports::{ApplicationResult, FuturesDeliveryEngine};
use crate::use_cases::bond_analytics::map_analytics_error;

pub struct CalculateFuturesDeliveryBasket<'a> {
    engine: &'a dyn FuturesDeliveryEngine,
}

impl<'a> CalculateFuturesDeliveryBasket<'a> {
    #[must_use]
    pub const fn new(engine: &'a dyn FuturesDeliveryEngine) -> Self {
        Self { engine }
    }

    /// Calculates a homogeneous delivery basket and selects CTD by maximum IRR.
    ///
    /// # Errors
    ///
    /// Returns validation failure for an empty, duplicate, or mixed-contract basket and maps
    /// stable engine failures without publishing partial results.
    pub fn execute(
        &self,
        inputs: &[FuturesDeliverableInput],
    ) -> ApplicationResult<FuturesDeliveryBasketResult> {
        let Some(first) = inputs.first() else {
            return Err(map_domain_error(DomainErrorCode::InvalidValue));
        };
        if inputs.iter().skip(1).any(|input| {
            input.owner() != first.owner()
                || input.futures_contract() != first.futures_contract()
                || input.rule_pack() != first.rule_pack()
                || input.snapshot() != first.snapshot()
                || input.valuation_at() != first.valuation_at()
                || input.purchase_date() != first.purchase_date()
                || input.delivery_month_first() != first.delivery_month_first()
                || input.delivery_date() != first.delivery_date()
                || input.product() != first.product()
                || input.futures_clean_price() != first.futures_clean_price()
                || input.financing_rate() != first.financing_rate()
        }) {
            return Err(map_domain_error(DomainErrorCode::BrokenLineage));
        }
        let candidates = inputs
            .iter()
            .map(|input| {
                let result = self.engine.calculate(input).map_err(map_analytics_error)?;
                result.validate_against(input).map_err(map_domain_error)?;
                Ok(result)
            })
            .collect::<ApplicationResult<Vec<_>>>()?;
        let ctd_index = select_ctd(&candidates);
        FuturesDeliveryBasketResult::new(candidates, ctd_index).map_err(map_domain_error)
    }
}

fn select_ctd(candidates: &[FuturesDeliveryResult]) -> usize {
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
    best
}
