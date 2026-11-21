use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::sync::Mutex;

use chrono::NaiveDate;
use ficant_application::ports::{ApplicationResult, PositionBaseProviderPort};
use ficant_application::use_cases::blotter::{
    AnalysisUnits, FullOpeningPositionBase, InstrumentCatalogEntry, RateCoverageEligibility,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_domain::analytics::CalendarRequirement;
use ficant_domain::blotter::{OpeningInstrumentBalance, OpeningPositionBase};
use ficant_domain::primitives::{Ulid, UnitRef, Version};

use crate::spi::csv_rfc4180::{
    CsvTable, parse_csv_date, parse_csv_fixed, parse_csv_market_time, parse_csv_non_empty,
    parse_csv_u64,
};

/// In-memory open-source reference adapter for `PositionBaseProviderPort`.
#[derive(Debug, Default)]
pub struct MockPositionBaseProvider {
    bases: Mutex<BTreeMap<(String, NaiveDate), FullOpeningPositionBase>>,
}

impl MockPositionBaseProvider {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, base: FullOpeningPositionBase) {
        let mut guard = self
            .bases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.insert((base.core.account_id.clone(), base.core.trading_date), base);
    }
}

impl PositionBaseProviderPort for MockPositionBaseProvider {
    fn fetch_opening_position_base(
        &self,
        request_id: &str,
        account_id: &str,
        trading_date: NaiveDate,
    ) -> ApplicationResult<FullOpeningPositionBase> {
        parse_csv_non_empty(request_id)?;
        parse_csv_non_empty(account_id)?;
        let guard = self
            .bases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard
            .get(&(account_id.to_owned(), trading_date))
            .cloned()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))
    }
}

/// RFC-4180 CSV open-source reference adapter for `PositionBaseProviderPort`.
///
/// Expected `accounts_csv` headers:
/// `account_id,base_id,trading_date,next_settlement_date,effective_from_rfc3339,watermark_time_rfc3339,market_timezone,watermark_source_seq,source_id,opening_cash_t0,opening_net_cash_delta_t1,source_start_after_feed_seq`
///
/// Expected `balances_csv` headers:
/// `account_id,trading_date,instrument_code,opening_economic_notional,opening_available_notional,opening_net_t0_notional,opening_net_t1_notional,eligibility_class,calendar_requirement`
#[derive(Clone, Debug)]
pub struct CsvPositionBaseProvider {
    bases: BTreeMap<(String, NaiveDate), FullOpeningPositionBase>,
}

type AccountBalanceGroup = (
    Vec<OpeningInstrumentBalance>,
    Vec<InstrumentCatalogEntry>,
    BTreeSet<String>,
);

impl CsvPositionBaseProvider {
    /// Parses account summary CSV and per-instrument balance CSV with strict Fail-Closed validation.
    #[allow(clippy::too_many_lines)]
    pub fn from_csv_str(
        accounts_csv: &str,
        balances_csv: &str,
        units: &AnalysisUnits,
    ) -> ApplicationResult<Self> {
        let accounts_table = CsvTable::parse(accounts_csv)?;
        accounts_table.require_headers(&[
            "account_id",
            "base_id",
            "trading_date",
            "next_settlement_date",
            "effective_from_rfc3339",
            "watermark_time_rfc3339",
            "market_timezone",
            "watermark_source_seq",
            "source_id",
            "opening_cash_t0",
            "opening_net_cash_delta_t1",
            "source_start_after_feed_seq",
        ])?;

        let balances_table = CsvTable::parse(balances_csv)?;
        balances_table.require_headers(&[
            "account_id",
            "trading_date",
            "instrument_code",
            "opening_economic_notional",
            "opening_available_notional",
            "opening_net_t0_notional",
            "opening_net_t1_notional",
        ])?;

        let mut balances_by_key: BTreeMap<(String, NaiveDate), AccountBalanceGroup> =
            BTreeMap::new();

        for row in balances_table.rows() {
            let account_id = parse_csv_non_empty(balances_table.field(row, "account_id")?)?;
            let trading_date = parse_csv_date(balances_table.field(row, "trading_date")?)?;
            let instrument_code =
                parse_csv_non_empty(balances_table.field(row, "instrument_code")?)?;
            let opening_economic_notional =
                parse_csv_fixed(balances_table.field(row, "opening_economic_notional")?)?;
            let opening_available_notional =
                parse_csv_fixed(balances_table.field(row, "opening_available_notional")?)?;
            let opening_net_t0_notional =
                parse_csv_fixed(balances_table.field(row, "opening_net_t0_notional")?)?;
            let opening_net_t1_notional =
                parse_csv_fixed(balances_table.field(row, "opening_net_t1_notional")?)?;

            let eligibility_class = balances_table
                .optional_field(row, "eligibility_class")
                .map(parse_csv_eligibility)
                .transpose()?;
            let calendar_requirement = balances_table
                .optional_field(row, "calendar_requirement")
                .map(parse_csv_calendar_req)
                .transpose()?;

            let entry = balances_by_key
                .entry((account_id, trading_date))
                .or_insert_with(|| (Vec::new(), Vec::new(), BTreeSet::new()));
            if !entry.2.insert(instrument_code.clone()) {
                // Duplicate instrument row for the same account/date -> fail closed.
                return Err(validation_failed());
            }
            entry.0.push(OpeningInstrumentBalance {
                instrument_code: instrument_code.clone(),
                opening_economic_notional,
                opening_available_notional,
                opening_net_t0_notional,
                opening_net_t1_notional,
            });
            entry.1.push(InstrumentCatalogEntry {
                instrument_code,
                bond: None,
                eligibility_class,
                calendar_requirement,
            });
        }

        let mut bases = BTreeMap::new();
        for row in accounts_table.rows() {
            let account_id = parse_csv_non_empty(accounts_table.field(row, "account_id")?)?;
            let base_id = Ulid::new(parse_csv_non_empty(accounts_table.field(row, "base_id")?)?)
                .map_err(|_| validation_failed())?;
            let trading_date = parse_csv_date(accounts_table.field(row, "trading_date")?)?;
            let next_settlement_date =
                parse_csv_date(accounts_table.field(row, "next_settlement_date")?)?;
            if next_settlement_date <= trading_date {
                return Err(validation_failed());
            }
            let market_tz = parse_csv_non_empty(accounts_table.field(row, "market_timezone")?)?;
            let effective_from = parse_csv_market_time(
                accounts_table.field(row, "effective_from_rfc3339")?,
                &market_tz,
            )?;
            let watermark_time = parse_csv_market_time(
                accounts_table.field(row, "watermark_time_rfc3339")?,
                &market_tz,
            )?;
            let watermark_source_seq = accounts_table
                .optional_field(row, "watermark_source_seq")
                .map(parse_csv_u64)
                .transpose()?;
            let source_id = parse_csv_non_empty(accounts_table.field(row, "source_id")?)?;
            let opening_cash_t0 = parse_csv_fixed(accounts_table.field(row, "opening_cash_t0")?)?;
            let opening_net_cash_delta_t1 =
                parse_csv_fixed(accounts_table.field(row, "opening_net_cash_delta_t1")?)?;
            let source_start_after_feed_seq =
                parse_csv_u64(accounts_table.field(row, "source_start_after_feed_seq")?)?;

            let key = (account_id.clone(), trading_date);
            if bases.contains_key(&key) {
                return Err(validation_failed());
            }
            let (balances, instruments, codes) = balances_by_key.remove(&key).unwrap_or_default();

            let full = FullOpeningPositionBase {
                core: OpeningPositionBase {
                    account_id,
                    base_id,
                    trading_date,
                    next_settlement_date,
                    effective_from,
                    watermark_time,
                    watermark_source_seq,
                    source_id,
                    opening_cash_t0,
                    opening_net_cash_delta_t1,
                    balances,
                    instrument_codes: codes.into_iter().collect(),
                    baseline_evidence: Vec::new(),
                    source_start_after_feed_seq,
                },
                instruments,
                calendar: None,
                default_calendar_requirement: None,
                units: units.clone(),
            };
            bases.insert(key, full);
        }

        // Orphan balance rows whose (account_id, trading_date) is missing from accounts CSV -> fail closed.
        if !balances_by_key.is_empty() {
            return Err(validation_failed());
        }

        Ok(Self { bases })
    }

    pub fn from_csv_files(
        accounts_path: impl AsRef<Path>,
        balances_path: impl AsRef<Path>,
        units: &AnalysisUnits,
    ) -> ApplicationResult<Self> {
        let accounts_csv = fs::read_to_string(accounts_path).map_err(|_| validation_failed())?;
        let balances_csv = fs::read_to_string(balances_path).map_err(|_| validation_failed())?;
        Self::from_csv_str(&accounts_csv, &balances_csv, units)
    }
}

impl PositionBaseProviderPort for CsvPositionBaseProvider {
    fn fetch_opening_position_base(
        &self,
        request_id: &str,
        account_id: &str,
        trading_date: NaiveDate,
    ) -> ApplicationResult<FullOpeningPositionBase> {
        parse_csv_non_empty(request_id)?;
        parse_csv_non_empty(account_id)?;
        self.bases
            .get(&(account_id.to_owned(), trading_date))
            .cloned()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))
    }
}

/// Builds a generic default `AnalysisUnits` bundle from canonical ULIDs for reference CSV adapters.
pub fn default_reference_analysis_units() -> ApplicationResult<AnalysisUnits> {
    let v1 = Version::new(1).map_err(|_| validation_failed())?;
    Ok(AnalysisUnits {
        currency_amount: UnitRef::new(
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5FA1").map_err(|_| validation_failed())?,
            v1,
        ),
        price_per_100: UnitRef::new(
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5FA2").map_err(|_| validation_failed())?,
            v1,
        ),
        rate: UnitRef::new(
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5FA3").map_err(|_| validation_failed())?,
            v1,
        ),
        dv01: UnitRef::new(
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5FA4").map_err(|_| validation_failed())?,
            v1,
        ),
        dv01_per_100: UnitRef::new(
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5FA5").map_err(|_| validation_failed())?,
            v1,
        ),
        dimensionless: UnitRef::new(
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5FA6").map_err(|_| validation_failed())?,
            v1,
        ),
        contract_count: UnitRef::new(
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5FA7").map_err(|_| validation_failed())?,
            v1,
        ),
    })
}

fn parse_csv_eligibility(text: &str) -> ApplicationResult<RateCoverageEligibility> {
    match text {
        "SOVEREIGN_OR_POLICY_FIXED" => Ok(RateCoverageEligibility::SovereignOrPolicyFixed),
        "CREDIT_OR_SPREAD_PRODUCT" => Ok(RateCoverageEligibility::CreditOrSpreadProduct),
        "FLOATING_OR_INDEXED" => Ok(RateCoverageEligibility::FloatingOrIndexed),
        "EMBEDDED_OPTION_OR_SINKING" => Ok(RateCoverageEligibility::EmbeddedOptionOrSinking),
        _ => Err(validation_failed()),
    }
}

fn parse_csv_calendar_req(text: &str) -> ApplicationResult<CalendarRequirement> {
    match text {
        "REFERENCE_REPLAY" => Ok(CalendarRequirement::ReferenceReplay),
        "EXACT_MARKET" => Ok(CalendarRequirement::ExactMarket),
        _ => Err(validation_failed()),
    }
}

const fn validation_failed() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}
