use ficant_domain::analytics::AnalyticsError;
use ficant_domain::futures_delivery::{FuturesDeliverableInput, FuturesDeliveryResult};

pub trait FuturesDeliveryEngine: Send + Sync {
    fn calculate(
        &self,
        input: &FuturesDeliverableInput,
    ) -> Result<FuturesDeliveryResult, AnalyticsError>;
}
