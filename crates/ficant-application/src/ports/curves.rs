use ficant_domain::analytics::AnalyticsError;
use ficant_domain::curves::{CarryRollInput, CarryRollResult, YieldCurvePoint, YieldCurveQuery};

pub trait YieldCurveEngine: Send + Sync {
    fn interpolate(&self, query: &YieldCurveQuery) -> Result<YieldCurvePoint, AnalyticsError>;
}

pub trait CarryRollEngine: Send + Sync {
    fn calculate(&self, input: &CarryRollInput) -> Result<CarryRollResult, AnalyticsError>;
}
