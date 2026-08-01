mod support;

use async_trait::async_trait;
use chrono::{NaiveDate, TimeZone, Utc};
use ficant_application::ports::{
    AppendDefinitionVersion, ApplicationResult, BeginBlobStage, BlobStore,
    CurveSnapshotMetadataRepository, DefinitionIdentity, DefinitionRepository, DefinitionValue,
    IdempotencyKey, InstrumentDefinition, InstrumentSubtype, IntegrityEvent, IntegrityEventSink,
    MarketFactRepository, PublishCurveSnapshot, RequiredVerifiedBlobRead, SafeTraceContext,
    VerifiedBlobReader, VerifiedBlobRole, VerifiedReadResourceKind, VerifyBlobStage,
};
use ficant_domain::ContentAddressed;
use ficant_domain::market::{
    ArtifactInputKind, Bond, BondBusinessDayConvention, BondCouponFrequency,
    BondDayCountConvention, BondPricingTerms, BondTaxAttributes, Calendar, CalendarInput,
    CurveSnapshot, CurveSnapshotInput, IncomeTaxStatus, Instrument, InstrumentInput,
    InstrumentKind, MarketRulePack, MarketRulePackInput, Unit, UnitInput, ValueAddedTaxStatus,
    VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, EffectivePeriod, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef,
    Version, VersionRef,
};
use ficant_storage::s3::S3BlobStore;

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn priced_bond_and_complete_curve_snapshot_round_trip_without_legacy_defaults() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let repository = support::repository(pool.clone());
    let owner = owner();
    let scope = support::access_scope(&owner);

    let values = definitions();
    for (index, value) in values.iter().cloned().enumerate() {
        repository
            .create_identity(DefinitionIdentity::new(
                Ulid::new(value.identity()).unwrap(),
                owner.clone(),
                value.kind(),
                IdempotencyKey::new(format!("r4d-a:definition:{index}:identity")).unwrap(),
            ))
            .await
            .unwrap();
        repository
            .append_version(
                AppendDefinitionVersion::new(
                    None,
                    value.clone(),
                    IdempotencyKey::new(format!("r4d-a:definition:{index}:v1")).unwrap(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            repository
                .get_version(
                    &scope,
                    Ulid::new(value.identity()).unwrap(),
                    Version::new(value.version()).unwrap(),
                )
                .await
                .unwrap(),
            Some(value)
        );
    }

    let bytes = b"canonical-r4d-a-curve-points";
    let curve = CurveSnapshot::new(CurveSnapshotInput {
        curve_snapshot_id: id('S'),
        owner: owner.clone(),
        as_of: time(1),
        currency: unit_ref('C'),
        curve_kind: "YTM".to_owned(),
        calendar: VersionRef::new(id('K'), version()),
        rule_pack: VersionRef::new(id('R'), version()),
        point_schema: ficant_application::ports::CURVE_POINT_SCHEMA.to_owned(),
        content_hash: ContentHash::digest(bytes),
        lineage: vec![LineageRef::versioned(id('B'), version())],
        input_kind: ArtifactInputKind::ExternalFixture,
    })
    .unwrap()
    .with_knowledge_time(time(2), "cn.gov.yield-curve")
    .unwrap();
    let verified = stage_verified_blob(&pool, owner.clone(), bytes).await;
    repository
        .publish_curve_snapshot(
            PublishCurveSnapshot::new(
                scope.clone(),
                curve.clone(),
                bytes.len() as u64,
                verified,
                IdempotencyKey::new("r4d-a:curve:publish:v1").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let metadata = repository
        .get_curve_snapshot_metadata(&scope, curve.id().clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(metadata.snapshot(), &curve);
    assert_eq!(metadata.blob_size(), bytes.len() as u64);
    assert_eq!(metadata.snapshot().visible_at(), Some(&time(2)));
    assert_eq!(
        metadata.snapshot().curve_family_id(),
        Some("cn.gov.yield-curve")
    );
    let (endpoint, bucket, access_key, secret_key) = support::s3_environment();
    let store =
        S3BlobStore::new(&endpoint, bucket, &access_key, &secret_key, pool.clone()).unwrap();
    let verified = store
        .read_required(
            &RequiredVerifiedBlobRead::new(
                scope,
                owner.clone(),
                VerifiedReadResourceKind::CurveSnapshot,
                curve.id().clone(),
                VerifiedBlobRole::CurvePoints,
                curve.content_hash().clone(),
                bytes.len() as u64,
                SafeTraceContext::new("0123456789abcdef0123456789abcdef").unwrap(),
            )
            .unwrap(),
            &Sink,
        )
        .await
        .unwrap();
    assert_eq!(verified.bytes(), bytes);

    let normalized: (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = sqlx::query_as(
        "SELECT coupon_frequency, day_count_convention, business_day_convention,
                    curve_family_id
             FROM market.bonds b
             CROSS JOIN market.curve_snapshots c
             WHERE b.instrument_id = $1 AND c.curve_snapshot_id = $2",
    )
    .bind(id('B').as_str())
    .bind(id('S').as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        normalized,
        (
            Some("semiannual".to_owned()),
            Some("act_act_bond_isma".to_owned()),
            Some("following".to_owned()),
            Some("cn.gov.yield-curve".to_owned()),
        )
    );
}

struct Sink;

#[async_trait]
impl IntegrityEventSink for Sink {
    async fn emit(&self, _: IntegrityEvent) -> ApplicationResult<()> {
        Ok(())
    }
}

fn definitions() -> Vec<DefinitionValue> {
    let owner = owner();
    let calendar = VersionRef::new(id('K'), version());
    let instrument = Instrument::new(InstrumentInput {
        instrument_id: id('B'),
        version: version(),
        owner: owner.clone(),
        kind: InstrumentKind::Bond,
        market: "CN".to_owned(),
        symbol: "BOND-R4D-A".to_owned(),
        currency: unit_ref('C'),
        calendar: calendar.clone(),
    })
    .unwrap();
    let bond = Bond::with_issuance(
        &instrument,
        date(2024, 1, 15),
        date(2024, 1, 15),
        date(2036, 8, 3),
        decimal("100000000", 0, unit_ref('N')),
        BondTaxAttributes::new(ValueAddedTaxStatus::Exempt, IncomeTaxStatus::Exempt),
        decimal("100", 0, unit_ref('N')),
    )
    .unwrap()
    .with_pricing_terms(
        BondPricingTerms::new(
            decimal("25", 3, unit_ref('V')),
            BondCouponFrequency::Semiannual,
            BondDayCountConvention::ActActBondIsma,
            BondBusinessDayConvention::Following,
        )
        .unwrap(),
    )
    .unwrap();
    vec![
        DefinitionValue::Unit(unit('C', "CNY", "currency")),
        DefinitionValue::Unit(unit('N', "FACE_CNY", "notional")),
        DefinitionValue::Unit(unit('V', "RATE", "rate")),
        DefinitionValue::Calendar(
            Calendar::new(CalendarInput {
                calendar_id: calendar.id().clone(),
                version: calendar.version(),
                owner: owner.clone(),
                market: "CN".to_owned(),
                market_timezone: "Asia/Shanghai".to_owned(),
                effective: EffectivePeriod::new(year_time(2020), year_time(2040)).unwrap(),
                sessions: Vec::new(),
            })
            .unwrap(),
        ),
        DefinitionValue::MarketRulePack(
            MarketRulePack::new(MarketRulePackInput {
                rule_pack_id: id('R'),
                version: version(),
                owner: owner.clone(),
                market: "CN".to_owned(),
                rule_type: "bond-pricing".to_owned(),
                source: "fixture".to_owned(),
                effective: EffectivePeriod::new(year_time(2020), year_time(2040)).unwrap(),
                verification_status: VerificationStatus::Verified,
                content_hash: ContentHash::digest(b"r4d-a-rule-pack"),
            })
            .unwrap(),
        ),
        DefinitionValue::Instrument(
            InstrumentDefinition::new(instrument, Some(InstrumentSubtype::Bond(bond))).unwrap(),
        ),
    ]
}

async fn stage_verified_blob(
    pool: &sqlx::PgPool,
    owner: OwnerRef,
    bytes: &[u8],
) -> ficant_application::ports::VerifiedBlobRef {
    let scope = support::access_scope(&owner);
    let (endpoint, bucket, access_key, secret_key) = support::s3_environment();
    let store =
        S3BlobStore::new(&endpoint, bucket, &access_key, &secret_key, pool.clone()).unwrap();
    let staged = store
        .begin_stage(
            BeginBlobStage::new(
                scope.clone(),
                owner,
                bytes.len() as u64,
                IdempotencyKey::new("r4d-a:curve:blob:v1").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    store
        .append_chunk(&scope, &staged, bytes.to_vec())
        .await
        .unwrap();
    store
        .verify_and_promote(
            VerifyBlobStage::new(
                scope,
                staged,
                ContentHash::digest(bytes),
                bytes.len() as u64,
            )
            .unwrap(),
        )
        .await
        .unwrap()
}

fn unit(suffix: char, code: &str, dimension: &str) -> Unit {
    Unit::new(UnitInput {
        unit_id: id(suffix),
        version: version(),
        owner: owner(),
        code: code.to_owned(),
        dimension: dimension.to_owned(),
        scale: 12,
        precision: 28,
    })
    .unwrap()
}

fn time(hour: u32) -> MarketTime {
    MarketTime::new(
        Utc.with_ymd_and_hms(2026, 8, 3, hour, 0, 0).unwrap(),
        "Asia/Shanghai",
        date(2026, 8, 3),
    )
    .unwrap()
}

fn year_time(year: i32) -> MarketTime {
    MarketTime::new(
        Utc.with_ymd_and_hms(year, 1, 1, 1, 0, 0).unwrap(),
        "Asia/Shanghai",
        date(year, 1, 1),
    )
    .unwrap()
}

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn decimal(value: &str, scale: u32, unit: UnitRef) -> DecimalValue {
    DecimalValue::new(value, scale, unit).unwrap()
}

fn owner() -> OwnerRef {
    OwnerRef::new(id('T'), id('O'))
}

fn unit_ref(suffix: char) -> UnitRef {
    UnitRef::new(id(suffix), version())
}

fn version() -> Version {
    Version::new(1).unwrap()
}

fn id(suffix: char) -> Ulid {
    let suffix = match suffix {
        'I' => '1',
        'L' => '2',
        'O' => '0',
        'U' => '3',
        value => value,
    };
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}
