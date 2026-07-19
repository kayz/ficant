use ficant_domain::futures_hedge::{FuturesHedgeInput, FuturesHedgeResult};

use crate::map_domain_error;
use crate::ports::{ApplicationResult, FuturesHedgeEngine};
use crate::use_cases::bond_analytics::map_analytics_error;

pub struct CalculateFuturesHedge<'a> {
    engine: &'a dyn FuturesHedgeEngine,
}

impl<'a> CalculateFuturesHedge<'a> {
    #[must_use]
    pub const fn new(engine: &'a dyn FuturesHedgeEngine) -> Self {
        Self { engine }
    }

    /// Calculates one exact-input-bound CTD DV01 hedge.
    ///
    /// # Errors
    ///
    /// Returns a stable validation or analytics failure without side effects.
    pub fn execute(&self, input: &FuturesHedgeInput) -> ApplicationResult<FuturesHedgeResult> {
        let result = self.engine.calculate(input).map_err(map_analytics_error)?;
        result.validate_against(input).map_err(map_domain_error)?;
        Ok(result)
    }
}
