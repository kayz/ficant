use async_trait::async_trait;
use ficant_domain::portfolio::{
    BenchmarkLevelSnapshot, PortfolioPerformanceConvention, PortfolioValuationSnapshot,
};
use ficant_domain::primitives::{ContentHash, LineageRef, MarketTime, OwnerRef, VersionRef};

use super::{AccessScope, ApplicationResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisiblePortfolioPerformanceConvention {
    value: PortfolioPerformanceConvention,
    visible_at: MarketTime,
}

impl VisiblePortfolioPerformanceConvention {
    #[must_use]
    pub const fn new(value: PortfolioPerformanceConvention, visible_at: MarketTime) -> Self {
        Self { value, visible_at }
    }

    #[must_use]
    pub const fn value(&self) -> &PortfolioPerformanceConvention {
        &self.value
    }

    #[must_use]
    pub const fn visible_at(&self) -> &MarketTime {
        &self.visible_at
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioPerformanceReadQuery {
    pub owner: OwnerRef,
    pub subject_ref: VersionRef,
    pub member_portfolios: Vec<LineageRef>,
    pub benchmark: LineageRef,
    pub period_from: MarketTime,
    pub period_to: MarketTime,
    pub knowledge_at: MarketTime,
}

#[async_trait]
pub trait PortfolioPerformanceRepository: Send + Sync {
    async fn read_valuation_snapshots(
        &self,
        scope: &AccessScope,
        query: &PortfolioPerformanceReadQuery,
    ) -> ApplicationResult<Vec<PortfolioValuationSnapshot>>;

    async fn read_benchmark_level_snapshots(
        &self,
        scope: &AccessScope,
        query: &PortfolioPerformanceReadQuery,
    ) -> ApplicationResult<Vec<BenchmarkLevelSnapshot>>;

    async fn read_performance_convention_exact(
        &self,
        scope: &AccessScope,
        owner: &OwnerRef,
        reference: &VersionRef,
        content_hash: &ContentHash,
        knowledge_at: &MarketTime,
    ) -> ApplicationResult<Option<VisiblePortfolioPerformanceConvention>>;
}
