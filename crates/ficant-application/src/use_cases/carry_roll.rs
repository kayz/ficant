use ficant_domain::curves::{CarryRollInput, CarryRollResult, YieldCurvePoint, YieldCurveQuery};

use crate::ports::{ApplicationResult, CarryRollEngine, YieldCurveEngine};
use crate::use_cases::bond_analytics::map_analytics_error;

pub struct InterpolateYieldCurve<'a> {
    engine: &'a dyn YieldCurveEngine,
}

impl<'a> InterpolateYieldCurve<'a> {
    #[must_use]
    pub const fn new(engine: &'a dyn YieldCurveEngine) -> Self {
        Self { engine }
    }

    pub fn execute(&self, query: &YieldCurveQuery) -> ApplicationResult<YieldCurvePoint> {
        self.engine.interpolate(query).map_err(map_analytics_error)
    }
}

pub struct CalculateCarryRoll<'a> {
    engine: &'a dyn CarryRollEngine,
}

impl<'a> CalculateCarryRoll<'a> {
    #[must_use]
    pub const fn new(engine: &'a dyn CarryRollEngine) -> Self {
        Self { engine }
    }

    pub fn execute(&self, input: &CarryRollInput) -> ApplicationResult<CarryRollResult> {
        self.engine.calculate(input).map_err(map_analytics_error)
    }
}
