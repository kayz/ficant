use chrono::{NaiveDate, NaiveTime};
use chrono_tz::Tz;

use crate::market::require_text;
use crate::primitives::{EffectivePeriod, OwnerRef, Ulid, Version};
use crate::{DomainErrorCode, DomainResult, VersionedDefinition};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarSession {
    local_date: NaiveDate,
    open_local_time: Option<NaiveTime>,
    close_local_time: Option<NaiveTime>,
}

impl CalendarSession {
    pub fn open(
        local_date: NaiveDate,
        open_local_time: NaiveTime,
        close_local_time: NaiveTime,
    ) -> DomainResult<Self> {
        if open_local_time >= close_local_time {
            return Err(DomainErrorCode::InvalidEffectiveTime);
        }
        Ok(Self {
            local_date,
            open_local_time: Some(open_local_time),
            close_local_time: Some(close_local_time),
        })
    }

    pub fn closed(local_date: NaiveDate) -> Self {
        Self {
            local_date,
            open_local_time: None,
            close_local_time: None,
        }
    }

    pub fn local_date(&self) -> NaiveDate {
        self.local_date
    }

    pub fn open_local_time(&self) -> Option<NaiveTime> {
        self.open_local_time
    }

    pub fn close_local_time(&self) -> Option<NaiveTime> {
        self.close_local_time
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Calendar {
    calendar_id: Ulid,
    version: Version,
    owner: OwnerRef,
    market: String,
    market_timezone: String,
    effective: EffectivePeriod,
    sessions: Vec<CalendarSession>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarInput {
    pub calendar_id: Ulid,
    pub version: Version,
    pub owner: OwnerRef,
    pub market: String,
    pub market_timezone: String,
    pub effective: EffectivePeriod,
    pub sessions: Vec<CalendarSession>,
}

impl Calendar {
    pub fn new(input: CalendarInput) -> DomainResult<Self> {
        let CalendarInput {
            calendar_id,
            version,
            owner,
            market,
            market_timezone,
            effective,
            sessions,
        } = input;
        require_text(&market)?;
        let timezone = market_timezone
            .parse::<Tz>()
            .map_err(|_| DomainErrorCode::InvalidEffectiveTime)?;
        if effective.from().market_timezone() != timezone.name()
            || effective.to().market_timezone() != timezone.name()
        {
            return Err(DomainErrorCode::InvalidEffectiveTime);
        }
        let from_date = effective.from().local_trading_date();
        let to_date = effective.to().local_trading_date();
        let mut previous_date = None;
        for session in &sessions {
            if session.local_date < from_date || session.local_date > to_date {
                return Err(DomainErrorCode::InvalidEffectiveTime);
            }
            if previous_date.is_some_and(|date| date >= session.local_date) {
                return Err(DomainErrorCode::InvalidEffectiveTime);
            }
            previous_date = Some(session.local_date);
        }
        Ok(Self {
            calendar_id,
            version,
            owner,
            market,
            market_timezone: timezone.name().to_owned(),
            effective,
            sessions,
        })
    }

    pub fn sessions(&self) -> &[CalendarSession] {
        &self.sessions
    }

    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    pub fn market(&self) -> &str {
        &self.market
    }

    pub fn market_timezone(&self) -> &str {
        &self.market_timezone
    }

    pub fn effective(&self) -> &EffectivePeriod {
        &self.effective
    }
}

impl VersionedDefinition for Calendar {
    fn identity(&self) -> &str {
        self.calendar_id.as_str()
    }

    fn version(&self) -> u64 {
        self.version.get()
    }
}
