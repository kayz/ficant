//! Production market wall clock implementing [`BlotterClock`].

use std::time::SystemTime;

use chrono::{DateTime, Utc};
use chrono_tz::Asia::Shanghai;
use ficant_application::ports::BlotterClock;
use ficant_domain::analytics::MARKET_TIMEZONE;
use ficant_domain::primitives::MarketTime;

/// Production wall clock anchored to `Asia/Shanghai`.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemBlotterClock;

impl BlotterClock for SystemBlotterClock {
    fn now(&self) -> MarketTime {
        let utc_now: DateTime<Utc> = SystemTime::now().into();
        let local_date = utc_now.with_timezone(&Shanghai).date_naive();
        MarketTime::new(utc_now, MARKET_TIMEZONE, local_date)
            .expect("SystemBlotterClock produces valid MarketTime")
    }
}
