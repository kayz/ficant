use ficant_domain::analytics::{AnalyticsError, BondAnalyticsInput, BondAnalyticsResult};

pub trait BondAnalyticsEngine: Send + Sync {
    fn calculate(&self, input: &BondAnalyticsInput) -> Result<BondAnalyticsResult, AnalyticsError>;
}
