use ficant_domain::analytics::AnalyticsError;
use ficant_domain::curves::{YieldCurvePoint, YieldCurveQuery};

pub trait YieldCurveEngine: Send + Sync {
    /// Interpolates one validated point without extrapolation.
    ///
    /// # Errors
    ///
    /// Returns a stable analytics error for invalid ABI, input, or numerical failure.
    fn interpolate(&self, query: &YieldCurveQuery) -> Result<YieldCurvePoint, AnalyticsError>;
}
