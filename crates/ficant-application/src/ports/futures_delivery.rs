use ficant_domain::analytics::AnalyticsError;
use ficant_domain::futures_delivery::{FuturesDeliverableInput, FuturesDeliveryResult};

pub trait FuturesDeliveryEngine: Send + Sync {
    /// Calculates one validated CFFEX deliverable-candidate result.
    ///
    /// # Errors
    ///
    /// Returns a stable analytics error for invalid ABI, input, eligibility, or numerical failure.
    fn calculate(
        &self,
        input: &FuturesDeliverableInput,
    ) -> Result<FuturesDeliveryResult, AnalyticsError>;
}
