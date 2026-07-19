use chrono::NaiveDate;

use crate::analytics::{AnalyticsObjectRef, FixedDecimal};
use crate::{DomainErrorCode, DomainResult};

pub const CURVE_RESULT_SCHEMA_ID: &str = "ficant.yield-curve-point.result.v1";
pub const CURVE_ALGORITHM_ID: &str = "ficant.cgb.ytm-curve.linear";
pub const CURVE_ALGORITHM_VERSION: u32 = 1;
pub const CURVE_CONVENTION_PROFILE: &str = "cfets-ytm-linear-v1";

const NEGATIVE_ONE_SCALED: i128 = -1_000_000_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum YieldCurveInterpolation {
    LinearYield = 1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct YieldCurveNode {
    maturity_date: NaiveDate,
    yield_to_maturity: FixedDecimal,
}

impl YieldCurveNode {
    pub fn new(maturity_date: NaiveDate, yield_to_maturity: FixedDecimal) -> DomainResult<Self> {
        if yield_to_maturity.scaled() <= NEGATIVE_ONE_SCALED {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            maturity_date,
            yield_to_maturity,
        })
    }

    #[must_use]
    pub const fn maturity_date(self) -> NaiveDate {
        self.maturity_date
    }

    #[must_use]
    pub const fn yield_to_maturity(self) -> FixedDecimal {
        self.yield_to_maturity
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct YieldCurveBinding {
    curve_snapshot: AnalyticsObjectRef,
    valuation_date: NaiveDate,
    interpolation: YieldCurveInterpolation,
    nodes: Vec<YieldCurveNode>,
}

impl YieldCurveBinding {
    pub fn new(
        curve_snapshot: AnalyticsObjectRef,
        valuation_date: NaiveDate,
        interpolation: YieldCurveInterpolation,
        nodes: Vec<YieldCurveNode>,
    ) -> DomainResult<Self> {
        if nodes.len() < 2
            || nodes
                .iter()
                .any(|node| node.maturity_date() <= valuation_date)
            || nodes
                .windows(2)
                .any(|pair| pair[0].maturity_date() >= pair[1].maturity_date())
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            curve_snapshot,
            valuation_date,
            interpolation,
            nodes,
        })
    }

    #[must_use]
    pub fn curve_snapshot(&self) -> &AnalyticsObjectRef {
        &self.curve_snapshot
    }

    #[must_use]
    pub const fn valuation_date(&self) -> NaiveDate {
        self.valuation_date
    }

    #[must_use]
    pub const fn interpolation(&self) -> YieldCurveInterpolation {
        self.interpolation
    }

    #[must_use]
    pub fn nodes(&self) -> &[YieldCurveNode] {
        &self.nodes
    }

    #[must_use]
    pub fn covers(&self, query_date: NaiveDate) -> bool {
        self.nodes.first().is_some_and(|first| {
            self.nodes.last().is_some_and(|last| {
                query_date >= first.maturity_date() && query_date <= last.maturity_date()
            })
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct YieldCurveQuery {
    curve: YieldCurveBinding,
    query_date: NaiveDate,
}

impl YieldCurveQuery {
    pub fn new(curve: YieldCurveBinding, query_date: NaiveDate) -> DomainResult<Self> {
        if query_date <= curve.valuation_date() || !curve.covers(query_date) {
            return Err(DomainErrorCode::InvalidEffectiveTime);
        }
        Ok(Self { curve, query_date })
    }

    #[must_use]
    pub fn curve(&self) -> &YieldCurveBinding {
        &self.curve
    }

    #[must_use]
    pub const fn query_date(&self) -> NaiveDate {
        self.query_date
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct YieldCurvePoint {
    query: YieldCurveQuery,
    yield_to_maturity: FixedDecimal,
}

impl YieldCurvePoint {
    pub fn new(query: YieldCurveQuery, yield_to_maturity: FixedDecimal) -> DomainResult<Self> {
        if yield_to_maturity.scaled() <= NEGATIVE_ONE_SCALED {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            query,
            yield_to_maturity,
        })
    }

    pub fn validate_against(&self, query: &YieldCurveQuery) -> DomainResult<()> {
        if &self.query != query {
            return Err(DomainErrorCode::BrokenLineage);
        }
        Ok(())
    }

    #[must_use]
    pub fn query(&self) -> &YieldCurveQuery {
        &self.query
    }

    #[must_use]
    pub const fn yield_to_maturity(&self) -> FixedDecimal {
        self.yield_to_maturity
    }

    #[must_use]
    pub fn schema_id(&self) -> &'static str {
        CURVE_RESULT_SCHEMA_ID
    }

    #[must_use]
    pub fn algorithm_id(&self) -> &'static str {
        CURVE_ALGORITHM_ID
    }

    #[must_use]
    pub const fn algorithm_version(&self) -> u32 {
        CURVE_ALGORITHM_VERSION
    }

    #[must_use]
    pub fn convention_profile(&self) -> &'static str {
        CURVE_CONVENTION_PROFILE
    }
}
