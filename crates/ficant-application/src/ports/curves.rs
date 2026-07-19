use ficant_domain::analytics::AnalyticsError;
use ficant_domain::curves::{CarryRollInput, CarryRollResult, YieldCurvePoint, YieldCurveQuery};

pub trait YieldCurveEngine: Send + Sync {
    /// Interpolates one validated point without extrapolation.
    ///
    /// # Errors
    ///
    /// Returns a stable analytics error for invalid ABI, input, or numerical failure.
    fn interpolate(&self, query: &YieldCurveQuery) -> Result<YieldCurvePoint, AnalyticsError>;
}

pub trait CarryRollEngine: Send + Sync {
    /// Calculates one unfunded holding-period carry and roll-down decomposition.
    ///
    /// # Errors
    ///
    /// Returns a stable analytics error for invalid input, curve coverage, or numerical failure.
    fn calculate(&self, input: &CarryRollInput) -> Result<CarryRollResult, AnalyticsError>;
}
