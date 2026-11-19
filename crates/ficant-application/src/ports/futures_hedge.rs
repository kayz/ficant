use ficant_domain::analytics::AnalyticsError;
use ficant_domain::futures_hedge::{FuturesHedgeInput, FuturesHedgeResult};

pub trait FuturesHedgeEngine: Send + Sync {
    fn calculate(&self, input: &FuturesHedgeInput) -> Result<FuturesHedgeResult, AnalyticsError>;
}
