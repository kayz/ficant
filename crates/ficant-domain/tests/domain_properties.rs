use chrono::{Duration, TimeZone, Utc};
use proptest::prelude::*;

use ficant_domain::market::{Unit, UnitInput};
use ficant_domain::primitives::{
    ContentHash, LineageRef, MarketTime, OwnerRef, Ulid, Version, ensure_next_version,
};
use ficant_domain::research::{DataSnapshot, DataSnapshotInput};
use ficant_domain::{ContentAddressed, DomainErrorCode};

const ULID_PREFIX: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA";

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("{ULID_PREFIX}{suffix}")).unwrap()
}

fn owner() -> OwnerRef {
    OwnerRef::new(id('A'), id('B'))
}

fn market_time(day: u32) -> MarketTime {
    let instant = Utc
        .with_ymd_and_hms(2026, 1, day, 1, 0, 0)
        .single()
        .unwrap();
    let local_date = instant
        .with_timezone(&chrono_tz::Asia::Shanghai)
        .date_naive();
    MarketTime::new(instant, "Asia/Shanghai", local_date).unwrap()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    #[test]
    fn q2_inv_01_random_invalid_unit_codes_are_rejected(
        tail in "[a-z0-9_ -]{0,18}",
        scale in 0u32..20,
        precision in 1u32..20,
    ) {
        let invalid_code = format!("lower{tail}");
        let result = Unit::new(UnitInput {
            unit_id: id('C'),
            version: Version::new(1).unwrap(),
            owner: owner(),
            code: invalid_code,
            dimension: "currency".to_owned(),
            scale: scale.min(precision),
            precision,
        });
        prop_assert_eq!(result.unwrap_err(), DomainErrorCode::InvalidUnit);
    }

    #[test]
    fn q2_inv_02_market_time_rejects_random_local_date_drift(seconds in 0i64..86_400) {
        let instant = Utc
            .with_ymd_and_hms(2026, 1, 2, 1, 0, 0)
            .single()
            .unwrap()
            + Duration::seconds(seconds);
        let actual_local_date = instant
            .with_timezone(&chrono_tz::Asia::Shanghai)
            .date_naive();
        let wrong_local_date = actual_local_date.succ_opt().unwrap();

        let result = MarketTime::new(instant, "Asia/Shanghai", wrong_local_date);
        prop_assert_eq!(result.unwrap_err(), DomainErrorCode::InvalidEffectiveTime);
    }

    #[test]
    fn q2_inv_03_historical_versions_cannot_be_overwritten(
        latest in 1u64..10_000,
        candidate in 1u64..10_000,
    ) {
        prop_assume!(candidate <= latest);
        let identity = id('D');
        let result = ensure_next_version(
            &identity,
            Version::new(latest).unwrap(),
            &identity,
            Version::new(candidate).unwrap(),
        );
        prop_assert_eq!(result.unwrap_err(), DomainErrorCode::VersionConflict);
    }

    #[test]
    fn q2_inv_04_published_snapshot_payload_drift_is_detected(
        payload in prop::collection::vec(any::<u8>(), 1..512),
        mutation in any::<u8>(),
    ) {
        let content_hash = ContentHash::digest(&payload);
        let snapshot = DataSnapshot::new(DataSnapshotInput {
            data_snapshot_id: id('E'),
            owner: owner(),
            visible_at: market_time(2),
            as_of: market_time(1),
            schema_hash: ContentHash::digest(b"schema"),
            manifest_hash: ContentHash::digest(b"manifest"),
            blob_content_hash: content_hash,
            lineage: vec![LineageRef::content_addressed(id('F'), ContentHash::digest(b"source"))],
        })
        .unwrap();
        let mut changed = payload.clone();
        changed.push(mutation);

        prop_assert_eq!(
            snapshot.verify_content(&changed).unwrap_err(),
            DomainErrorCode::ContentHashMismatch
        );
    }
}
