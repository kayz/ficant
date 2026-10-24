//! Shared fixtures for the replay tests (synthetic data only).
#![allow(dead_code)]

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, Utc};
use ficant_application::{ApplicationError, ApplicationErrorCategory, PointInTimeBars};
use ficant_data::{BarSnapshotCodec, CanonicalBarIngestor, DataError, VerifiedBarSnapshot};
use ficant_domain::ContentAddressed;
use ficant_domain::market::{Bar, BarSettlement, PriceSourceType};
use ficant_domain::primitives::{ContentHash, DecimalValue, Ulid, UnitRef, Version, VersionRef};
use ficant_domain::replay::{BondCarry, ReplayAmount, ReplayFunding};
use ficant_domain::subject::{
    AccessSet, ConstraintSetRef, FundingTier, LimitCeiling, SubjectStateSnapshot, SubjectVersion,
    TaxTreatment,
};

use ficant_data::PointInTimeWindow;

use crate::bar_support::{
    INSTRUMENT, SOURCE_A, build_request_in_window, data_source, file_source, market_time, request,
    vendor_a_mapping,
};

/// The ADR-0024 single-day path (same rows as `bar_replay`): traded face per bar is 20m, 20m,
/// 30m, 0, 10m, 10m, 0, 10m, 10m.
pub const ONE_DAY: &str = r#"
{"id":"b0930","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:30:00","pub":"2026-07-20 09:31:02","o":"100.10","h":"100.10","l":"100.10","c":"100.10","vw":"100.10","vol":"20000000","ytm":"1.845"}
{"id":"b0931","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:31:00","pub":"2026-07-20 09:32:02","o":"100.12","h":"100.12","l":"100.12","c":"100.12","vw":"100.12","vol":"20000000","ytm":"1.838"}
{"id":"b0932","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:32:00","pub":"2026-07-20 09:33:02","o":"100.12","h":"100.14","l":"100.12","c":"100.14","vw":"100.13","vol":"30000000","ytm":"1.836"}
{"id":"b0933","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:33:00","pub":"2026-07-20 09:34:02","vol":"0"}
{"id":"b0934","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:34:00","pub":"2026-07-20 09:35:02","o":"100.30","h":"100.30","l":"100.30","c":"100.30","vol":"10000000","ytm":"1.815"}
{"id":"b0935","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:35:00","pub":"2026-07-20 09:36:02","o":"100.29","h":"100.29","l":"100.29","c":"100.29","vol":"10000000","ytm":"1.816"}
{"id":"b0936","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:36:00","pub":"2026-07-20 09:37:02","vol":"0"}
{"id":"b0937","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:37:00","pub":"2026-07-20 09:38:02","o":"100.31","h":"100.31","l":"100.31","c":"100.31","vw":"100.31","vol":"10000000","ytm":"1.814"}
{"id":"b0938","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:38:00","pub":"2026-07-20 09:39:02","o":"100.32","h":"100.32","l":"100.32","c":"100.32","vol":"10000000","ytm":"1.813"}
"#;

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

/// A state snapshot of `subject` with the given limit ceilings, visible from 2026-07-01.
pub fn state_with(
    subject: &SubjectVersion,
    snapshot_id: &str,
    ceilings: &[(&str, &str)],
) -> SubjectStateSnapshot {
    let unit = UnitRef::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F41").unwrap(),
        Version::new(1).unwrap(),
    );
    let ceilings = ceilings
        .iter()
        .map(|(code, value)| {
            LimitCeiling::new(*code, DecimalValue::new(*value, 0, unit.clone()).unwrap()).unwrap()
        })
        .collect();
    SubjectStateSnapshot::new(
        Ulid::new(snapshot_id).unwrap(),
        subject.reference().clone(),
        DecimalValue::new("1000000000", 0, unit).unwrap(),
        ceilings,
        "2026-07-01T00:00:00Z".parse().unwrap(),
        "2026-07-01T00:00:00Z".parse().unwrap(),
        "Asia/Shanghai",
    )
    .unwrap()
}

/// A state snapshot with no ceilings, for Subjects that bind no constraint set.
pub fn state(subject: &SubjectVersion, snapshot_id: &str) -> SubjectStateSnapshot {
    state_with(subject, snapshot_id, &[])
}

/// Like [`subject`], binding the constraint set of regulatory `RulePack` `rule_pack`.
pub fn subject_bound(version: u64, rule_pack: &VersionRef) -> SubjectVersion {
    SubjectVersion::new(
        VersionRef::new(
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5FV0").unwrap(),
            Version::new(version).unwrap(),
        ),
        AccessSet::new(["CGB"], ["BOND"]).unwrap(),
        FundingTier::DrAvailable,
        TaxTreatment::new("vat-profile", "income-tax-profile").unwrap(),
        "mark-to-market",
        "none",
        Some(ConstraintSetRef::new(rule_pack.clone())),
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
