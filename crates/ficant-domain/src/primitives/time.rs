use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;

use crate::{DomainErrorCode, DomainResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarketTime {
    instant: DateTime<Utc>,
    market_timezone: String,
    local_trading_date: NaiveDate,
}

impl MarketTime {
    pub fn new(
        instant: DateTime<Utc>,
        market_timezone: impl Into<String>,
        local_trading_date: NaiveDate,
    ) -> DomainResult<Self> {
        let market_timezone = market_timezone.into();
        let timezone = market_timezone
            .parse::<Tz>()
            .map_err(|_| DomainErrorCode::InvalidEffectiveTime)?;
        if instant.with_timezone(&timezone).date_naive() != local_trading_date {
            return Err(DomainErrorCode::InvalidEffectiveTime);
        }
        Ok(Self {
            instant,
            market_timezone: timezone.name().to_owned(),
            local_trading_date,
        })
    }

    pub fn instant(&self) -> DateTime<Utc> {
        self.instant
    }

    pub fn market_timezone(&self) -> &str {
        &self.market_timezone
    }

    pub fn local_trading_date(&self) -> NaiveDate {
        self.local_trading_date
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectivePeriod {
    from: MarketTime,
    to: MarketTime,
}

impl EffectivePeriod {
    pub fn new(from: MarketTime, to: MarketTime) -> DomainResult<Self> {
        if from.instant >= to.instant {
            return Err(DomainErrorCode::InvalidEffectiveTime);
        }
        Ok(Self { from, to })
    }

    pub fn from(&self) -> &MarketTime {
        &self.from
    }

    pub fn to(&self) -> &MarketTime {
        &self.to
    }
}
