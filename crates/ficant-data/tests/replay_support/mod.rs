//! Shared fixtures for the replay tests (synthetic data only).
#![allow(dead_code)]

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, Utc};
use ficant_application::{ApplicationError, ApplicationErrorCategory, PointInTimeBars};
use ficant_data::{BarSnapshotCodec, CanonicalBarIngestor, DataError, VerifiedBarSnapshot};
use ficant_domain::ContentAddressed;
use ficant_domain::market::{Bar, BarSettlement, PriceSourceType};
use ficant_domain::primitives::{ContentHash, Ulid, Version, VersionRef};
use ficant_domain::replay::{BondCarry, ReplayAmount, ReplayFunding};
use ficant_domain::subject::{AccessSet, FundingTier, SubjectVersion, TaxTreatment};

use ficant_data::PointInTimeWindow;

use crate::bar_support::{
    INSTRUMENT, SOURCE_A, build_request_in_window, data_source, file_source, market_time, request,
    vendor_a_mapping,
};

pub struct SnapshotBars(pub VerifiedBarSnapshot);

impl PointInTimeBars for SnapshotBars {
    fn snapshot(&self) -> (Ulid, ContentHash) {
        (
            self.0.snapshot().id().clone(),
            self.0.snapshot().content_hash().clone(),
        )
    }

    fn price_source_type(&self) -> PriceSourceType {
        self.0.price_source_type()
    }

    fn all_bars(&self) -> &[Bar] {
        self.0.bars()
    }

    fn visible_at(&self, decision_time: DateTime<Utc>) -> Result<Vec<&Bar>, ApplicationError> {
        self.0.visible_at(decision_time).map_err(|_: DataError| {
            ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
        })
    }
}

/// Imports vendor-A rows (with a VWAP field) and returns the verified snapshot.
pub async fn snapshot(rows: &str, snapshot_id: &str) -> VerifiedBarSnapshot {
    import(rows, snapshot_id, Ok(request(SOURCE_A, "260011.IB"))).await
}

/// Like [`snapshot`], known as of the end of 2026-07-21 (Shanghai), for two-day replays.
pub async fn two_day_snapshot(rows: &str, snapshot_id: &str) -> VerifiedBarSnapshot {
    let window = PointInTimeWindow::new(
        market_time("2026-07-21T09:00:00Z"),
        market_time("2026-07-21T09:00:00Z"),
    )
    .unwrap();
    let request = build_request_in_window(
        data_source(SOURCE_A, Some(PriceSourceType::RealTrade)),
        "260011.IB",
        window,
    );
    import(rows, snapshot_id, request).await
}

async fn import(
    rows: &str,
    snapshot_id: &str,
    request: ficant_data::DataResult<ficant_data::CanonicalBarIngestRequest>,
) -> VerifiedBarSnapshot {
    let request = request.unwrap();
    let mut mapping = vendor_a_mapping();
    mapping.vwap = Some("vw".to_owned());
    let source = file_source(mapping, SOURCE_A, rows);
    let batch = CanonicalBarIngestor
        .ingest(&source, &request)
        .await
        .unwrap();
    assert!(
        batch.report().rejections.is_empty(),
        "{:?}",
        batch.report().rejections
    );
    let package = BarSnapshotCodec
        .build(Ulid::new(snapshot_id).unwrap(), &request, &batch)
        .unwrap();
    BarSnapshotCodec
        .decode_verified(
            package.snapshot().clone(),
            package.parquet(),
            package.manifest(),
        )
        .unwrap()
}

pub fn amount(text: &str) -> ReplayAmount {
    ReplayAmount::parse(text).unwrap()
}

pub fn instrument() -> VersionRef {
    VersionRef::new(Ulid::new(INSTRUMENT).unwrap(), Version::new(7).unwrap())
}

pub fn date(text: &str) -> NaiveDate {
    NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap()
}

pub fn subject(version: u64, tier: FundingTier) -> SubjectVersion {
    SubjectVersion::new(
        VersionRef::new(
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5FV0").unwrap(),
            Version::new(version).unwrap(),
        ),
        AccessSet::new(["CGB"], ["BOND"]).unwrap(),
        tier,
        TaxTreatment::new("vat-profile", "income-tax-profile").unwrap(),
        "mark-to-market",
        "none",
        None,
    )
    .unwrap()
}

/// Funding for the `DR_AVAILABLE` tier: 3.65% a year on a 365-day basis, i.e. exactly 0.01% of
/// the borrowed amount per calendar day.
pub fn funding(annual_rate: &str, day_basis: u32) -> ReplayFunding {
    ReplayFunding::new(
        VersionRef::new(
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5FR0").unwrap(),
            Version::new(1).unwrap(),
        ),
        FundingTier::DrAvailable,
        amount(annual_rate),
        day_basis,
    )
    .unwrap()
}

/// Coupon data for the one test bond: accrued interest per trading date and coupon payments by
/// payment date, all per 100 face.
#[derive(Clone)]
pub struct TableCarry {
    pub accrued: BTreeMap<NaiveDate, ReplayAmount>,
    pub payments: BTreeMap<NaiveDate, ReplayAmount>,
}

impl TableCarry {
    pub fn new(accrued: &[(&str, &str)], payments: &[(&str, &str)]) -> Self {
        let table = |rows: &[(&str, &str)]| {
            rows.iter()
                .map(|(day, value)| (date(day), amount(value)))
                .collect()
        };
        Self {
            accrued: table(accrued),
            payments: table(payments),
        }
    }
}

impl BondCarry for TableCarry {
    fn identity(&self) -> ContentHash {
        ContentHash::digest(format!("{:?}|{:?}", self.accrued, self.payments).as_bytes())
    }

    fn accrued_interest(
        &self,
        bond: &VersionRef,
        _: BarSettlement,
        trading_date: NaiveDate,
    ) -> Option<ReplayAmount> {
        (bond == &instrument())
            .then(|| self.accrued.get(&trading_date).copied())
            .flatten()
    }

    fn coupon_paid(
        &self,
        bond: &VersionRef,
        _: BarSettlement,
        after: NaiveDate,
        through: NaiveDate,
    ) -> Option<ReplayAmount> {
        (bond == &instrument()).then(|| {
            self.payments
                .range(after.succ_opt().unwrap()..=through)
                .fold(ReplayAmount::ZERO, |total, (_, value)| total.plus(*value))
        })
    }
}
