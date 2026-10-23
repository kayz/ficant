//! Bar snapshots: immutable packaging, verified decoding, and point-in-time reads (AC38 input).

mod bar_support;

use chrono::Duration;
use ficant_data::{
    BAR_SNAPSHOT_MANIFEST_SCHEMA_ID, BarRejectionReason, BarSnapshotCodec,
    CanonicalBarIngestRequest, CanonicalBarIngestor, CanonicalSnapshotPackage, DataError,
    VerifiedBarSnapshot, canonical_bar_schema_hash,
};
use ficant_domain::primitives::Ulid;
use ficant_domain::research::{DataSnapshot, DataSnapshotInput};
use ficant_domain::{ContentAddressed, Lineaged};

use bar_support::*;

const SNAPSHOT_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FS0";

/// Three minute bars (09:30, 09:31, 09:32 local), each visible 2 seconds after it ends, plus one
/// rejected row that must travel with the snapshot.
const ROWS: &str = r#"
{"id":"a-0930","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:30:00","pub":"2026-07-20 09:31:02","o":"100.1200","h":"100.1500","l":"100.1000","c":"100.1400","vol":"30000000","n":3,"ytm":"1.8360"}
{"id":"a-0931","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:31:00","pub":"2026-07-20 09:32:02","o":"100.1400","h":"100.1600","l":"100.1300","c":"100.1500","vol":"20000000","n":2}
{"id":"a-0932","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:32:00","pub":"2026-07-20 09:33:02","vol":"0","n":0}
{"id":"a-bad","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 08:00:00","pub":"2026-07-20 08:01:02","vol":"0"}
"#;

/// The same source one bar later: an extra 09:33 bar, visible at 09:34:02.
const ROWS_WITH_LATER_BAR: &str = r#"
{"id":"a-0930","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:30:00","pub":"2026-07-20 09:31:02","o":"100.1200","h":"100.1500","l":"100.1000","c":"100.1400","vol":"30000000","n":3,"ytm":"1.8360"}
{"id":"a-0931","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:31:00","pub":"2026-07-20 09:32:02","o":"100.1400","h":"100.1600","l":"100.1300","c":"100.1500","vol":"20000000","n":2}
{"id":"a-0932","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:32:00","pub":"2026-07-20 09:33:02","vol":"0","n":0}
{"id":"a-0933","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:33:00","pub":"2026-07-20 09:34:02","o":"100.1500","h":"100.1700","l":"100.1500","c":"100.1700","vol":"10000000","n":1}
{"id":"a-bad","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 08:00:00","pub":"2026-07-20 08:01:02","vol":"0"}
"#;

async fn package(rows: &str) -> (CanonicalBarIngestRequest, CanonicalSnapshotPackage) {
    let source = file_source(vendor_a_mapping(), SOURCE_A, rows);
    let request = request(SOURCE_A, "260011.IB");
    let batch = CanonicalBarIngestor
        .ingest(&source, &request)
        .await
        .unwrap();
    let package = BarSnapshotCodec
        .build(Ulid::new(SNAPSHOT_ID).unwrap(), &request, &batch)
        .unwrap();
    (request, package)
}

fn decode(package: &CanonicalSnapshotPackage) -> VerifiedBarSnapshot {
    BarSnapshotCodec
        .decode_verified(
            package.snapshot().clone(),
            package.parquet(),
            package.manifest(),
        )
        .unwrap()
}

#[tokio::test]
async fn snapshot_round_trips_bars_and_the_rejection_report() {
    let (_, package) = package(ROWS).await;
    let snapshot = package.snapshot();
    assert_eq!(snapshot.schema_hash(), &canonical_bar_schema_hash());
    assert_eq!(snapshot.visible_at().instant(), utc("2026-07-21T09:00:00Z"));

    let verified = decode(&package);
    let ids: Vec<_> = verified
        .bars()
        .iter()
        .map(ficant_domain::market::Bar::source_record_id)
        .collect();
    assert_eq!(ids, ["a-0930", "a-0931", "a-0932"]);
    assert!(verified.bars()[2].prices().is_none());
    assert_eq!(
        verified.bars()[0].close_yield().unwrap().coefficient(),
        "1836"
    );

    let manifest = verified.manifest();
    assert_eq!(manifest.row_count(), 3);
    assert_eq!(manifest.visibility_rule(), "source-field");
    let rejections = manifest.rejections().unwrap();
    assert_eq!(rejections.len(), 1);
    assert_eq!(rejections[0].row_id, "a-bad");
    assert_eq!(
        rejections[0].reason,
        BarRejectionReason::OutsideTradingSession
    );
    let text = std::str::from_utf8(package.manifest()).unwrap();
    assert!(text.contains(BAR_SNAPSHOT_MANIFEST_SCHEMA_ID));
    assert!(text.contains("OUTSIDE_TRADING_SESSION"));

    // Lineage covers source, mapping, calendar, all three units and the instrument.
    assert_eq!(snapshot.lineage().len(), 7);
}

#[tokio::test]
async fn packaging_is_deterministic() {
    let (_, first) = package(ROWS).await;
    let (_, second) = package(ROWS).await;
    assert_eq!(first.parquet(), second.parquet());
    assert_eq!(first.manifest(), second.manifest());
    assert_eq!(first.snapshot(), second.snapshot());
}

#[tokio::test]
async fn any_drift_between_snapshot_parquet_and_manifest_fails_closed() {
    let (_, package) = package(ROWS).await;

    let mut parquet = package.parquet().to_vec();
    let middle = parquet.len() / 2;
    parquet[middle] ^= 0x01;
    assert_eq!(
        BarSnapshotCodec
            .decode_verified(package.snapshot().clone(), &parquet, package.manifest())
            .unwrap_err(),
        DataError::SnapshotIntegrityFailed
    );

    // A rewritten manifest with a self-consistent snapshot still fails: the rejection report no
    // longer accounts for every source row.
    let text = std::str::from_utf8(package.manifest())
        .unwrap()
        .replace("\"outside_window_rows\":0", "\"outside_window_rows\":5");
    let snapshot = rebind_manifest(package.snapshot(), text.as_bytes());
    assert_eq!(
        BarSnapshotCodec
            .decode_verified(snapshot, package.parquet(), text.as_bytes())
            .unwrap_err(),
        DataError::SnapshotIntegrityFailed
    );

    // Claiming the import used a different visibility rule is caught as well.
    let text = std::str::from_utf8(package.manifest())
        .unwrap()
        .replace("\"source-field\"", "\"guessed\"");
    let snapshot = rebind_manifest(package.snapshot(), text.as_bytes());
    assert!(
        BarSnapshotCodec
            .decode_verified(snapshot, package.parquet(), text.as_bytes())
            .is_err()
    );
}

#[tokio::test]
async fn reads_return_only_bars_visible_at_the_decision_time() {
    let (_, package) = package(ROWS).await;
    let verified = decode(&package);
    let ids_at = |time: &str| -> Vec<String> {
        verified
            .visible_at(utc(time))
            .unwrap()
            .iter()
            .map(|bar| bar.source_record_id().to_owned())
            .collect()
    };
    // 09:31:00 local: the 09:30 bar has ended but is not visible until 09:31:02.
    assert!(ids_at("2026-07-20T01:31:00Z").is_empty());
    assert_eq!(ids_at("2026-07-20T01:31:02Z"), ["a-0930"]);
    assert_eq!(ids_at("2026-07-20T01:33:01Z"), ["a-0930", "a-0931"]);
    assert_eq!(
        ids_at("2026-07-21T09:00:00Z"),
        ["a-0930", "a-0931", "a-0932"]
    );
    for bar in verified.visible_at(utc("2026-07-20T01:33:01Z")).unwrap() {
        assert!(bar.visible_at() <= utc("2026-07-20T01:33:01Z"));
        assert!(bar.observed_at() <= bar.visible_at());
    }
    // The snapshot cannot answer for times after its visibility cutoff.
    assert_eq!(
        verified
            .visible_at(utc("2026-07-21T09:00:00Z") + Duration::microseconds(1))
            .unwrap_err(),
        DataError::PointInTimeViolation
    );
}

#[tokio::test]
async fn a_bar_visible_later_never_changes_what_was_known_earlier() {
    // AC38 property: appending a record whose visible_at is after a decision time leaves every
    // read at or before that time unchanged.
    let (_, base) = package(ROWS).await;
    let (_, extended) = package(ROWS_WITH_LATER_BAR).await;
    let base = decode(&base);
    let extended = decode(&extended);
    assert_eq!(extended.bars().len(), base.bars().len() + 1);

    let later_bar_visible = utc("2026-07-20T01:34:02Z");
    let mut decision = utc("2026-07-20T01:30:00Z");
    while decision < later_bar_visible {
        assert_eq!(
            base.visible_at(decision).unwrap(),
            extended.visible_at(decision).unwrap(),
            "reads at {decision} must not see the later bar"
        );
        decision += Duration::seconds(1);
    }
    assert_ne!(
        base.visible_at(later_bar_visible).unwrap(),
        extended.visible_at(later_bar_visible).unwrap()
    );
}

/// Rebuilds the domain snapshot around a modified manifest so only the content checks can catch
/// the change.
fn rebind_manifest(snapshot: &DataSnapshot, manifest: &[u8]) -> DataSnapshot {
    DataSnapshot::new(DataSnapshotInput {
        data_snapshot_id: snapshot.id().clone(),
        owner: snapshot.owner().clone(),
        visible_at: snapshot.visible_at().clone(),
        as_of: snapshot.as_of().clone(),
        schema_hash: snapshot.schema_hash().clone(),
        manifest_hash: ficant_domain::primitives::ContentHash::digest(manifest),
        blob_content_hash: snapshot.content_hash().clone(),
        lineage: snapshot.lineage().to_vec(),
    })
    .unwrap()
}
