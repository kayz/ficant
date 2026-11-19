use ficant_domain::futures_hedge::{FuturesHedgeInput, FuturesHedgeResult};

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

    pub fn execute(&self, input: &FuturesHedgeInput) -> ApplicationResult<FuturesHedgeResult> {
        self.engine.calculate(input).map_err(map_analytics_error)
    }
}
