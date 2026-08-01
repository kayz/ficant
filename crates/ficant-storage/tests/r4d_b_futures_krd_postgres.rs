mod support;

use ficant_application::IdempotencyKey;
use ficant_application::ports::{
    AppendDefinitionVersion, DefinitionIdentity, DefinitionKind, DefinitionRepository,
    DefinitionValue, InstrumentDefinition, InstrumentSubtype,
};
use ficant_domain::market::{FuturesContract, Instrument, InstrumentInput, InstrumentKind};
use ficant_domain::primitives::{
    DecimalValue, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};

#[tokio::test]
async fn risk_ready_and_legacy_futures_round_trip_without_reinterpreting_legacy_rows() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    seed_dependencies(&pool).await;
    let repository = support::repository(pool.clone());
    let scope = support::access_scope(&owner());

    let risk_ready = definition('F', true);
    persist(&repository, risk_ready.clone(), "risk-ready").await;
    assert_eq!(
        repository
            .get_version(&scope, id('F'), version(1))
            .await
            .unwrap(),
        Some(DefinitionValue::Instrument(risk_ready))
    );
    let risk_columns: (Option<String>, Option<String>, Option<i64>) = sqlx::query_as(
        "SELECT product_code, price_unit_id::text, price_unit_version
         FROM market.futures_contracts
         WHERE tenant_id = $1 AND instrument_id = $2 AND version = 1",
    )
    .bind(id('T').as_str())
    .bind(id('F').as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        risk_columns,
        (Some("T".to_owned()), Some(id('Q').to_string()), Some(1))
    );

    let legacy = definition('G', false);
    persist(&repository, legacy.clone(), "legacy").await;
    assert_eq!(
        repository
            .get_version(&scope, id('G'), version(1))
            .await
            .unwrap(),
        Some(DefinitionValue::Instrument(legacy))
    );
    let legacy_columns: (Option<String>, Option<String>, Option<i64>) = sqlx::query_as(
        "SELECT product_code, price_unit_id::text, price_unit_version
         FROM market.futures_contracts
         WHERE tenant_id = $1 AND instrument_id = $2 AND version = 1",
    )
    .bind(id('T').as_str())
    .bind(id('G').as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(legacy_columns, (None, None, None));
}

async fn persist(
    repository: &impl DefinitionRepository,
    definition: InstrumentDefinition,
    key_suffix: &str,
) {
    repository
        .create_identity(DefinitionIdentity::new(
            definition.instrument().id().clone(),
            owner(),
            DefinitionKind::Instrument,
            key(&format!("create-{key_suffix}")),
        ))
        .await
        .unwrap();
    repository
        .append_version(
            AppendDefinitionVersion::new(
                None,
                DefinitionValue::Instrument(definition),
                key(&format!("append-{key_suffix}")),
            )
            .unwrap(),
        )
        .await
        .unwrap();
}

fn definition(suffix: char, risk_ready: bool) -> InstrumentDefinition {
    let instrument = Instrument::new(InstrumentInput {
        instrument_id: id(suffix),
        version: version(1),
        owner: owner(),
        kind: InstrumentKind::Futures,
        market: "CFFEX".to_owned(),
        symbol: format!("T26{suffix}"),
        currency: UnitRef::new(id('C'), version(1)),
        calendar: VersionRef::new(id('A'), version(1)),
    })
    .unwrap();
    let future = FuturesContract::new(
        &instrument,
        time(10),
        time(11),
        time(12),
        DecimalValue::new("10000", 0, UnitRef::new(id('C'), version(1))).unwrap(),
        VersionRef::new(id('R'), version(2)),
    )
    .unwrap();
    let future = if risk_ready {
        future
            .with_risk_terms("T", UnitRef::new(id('Q'), version(1)))
            .unwrap()
    } else {
        future
    };
    InstrumentDefinition::new(instrument, Some(InstrumentSubtype::FuturesContract(future))).unwrap()
}

async fn seed_dependencies(pool: &sqlx::PgPool) {
    sqlx::raw_sql(
        "INSERT INTO market.units
         (tenant_id, unit_id, version, owner_id, code, dimension, scale, precision, payload)
         VALUES
         ('01ARZ3NDEKTSV4RRFFQ69G5F0T', '01ARZ3NDEKTSV4RRFFQ69G5F0C', 1,
          '01ARZ3NDEKTSV4RRFFQ69G5F0P', 'CNY', 'currency', 2, 18, '\\x01'),
         ('01ARZ3NDEKTSV4RRFFQ69G5F0T', '01ARZ3NDEKTSV4RRFFQ69G5F0Q', 1,
          '01ARZ3NDEKTSV4RRFFQ69G5F0P', 'CNY100', 'price', 12, 28, '\\x01');
         INSERT INTO market.calendars
         (tenant_id, calendar_id, version, owner_id, market, market_timezone,
          effective_from, effective_to, payload)
         VALUES ('01ARZ3NDEKTSV4RRFFQ69G5F0T', '01ARZ3NDEKTSV4RRFFQ69G5F0A', 1,
                 '01ARZ3NDEKTSV4RRFFQ69G5F0P', 'CFFEX', 'Asia/Shanghai',
                 '2020-01-01T00:00:00Z', '2030-01-01T00:00:00Z', '\\x01');
         INSERT INTO market.market_rule_packs
         (tenant_id, rule_pack_id, version, owner_id, market, rule_type, source,
          effective_from, effective_to, verification_status, content_hash, payload)
         VALUES ('01ARZ3NDEKTSV4RRFFQ69G5F0T', '01ARZ3NDEKTSV4RRFFQ69G5F0R', 2,
                 '01ARZ3NDEKTSV4RRFFQ69G5F0P', 'CFFEX', 'cgb-futures', 'fixture',
                 '2020-01-01T00:00:00Z', '2030-01-01T00:00:00Z', 'VERIFIED',
                 repeat('a', 64), '\\x01');",
    )
    .execute(pool)
    .await
    .unwrap();
}

fn time(hour: u32) -> MarketTime {
    MarketTime::new(
        format!("2026-03-04T{hour:02}:00:00Z").parse().unwrap(),
        "Asia/Shanghai",
        "2026-03-04".parse().unwrap(),
    )
    .unwrap()
}

fn owner() -> OwnerRef {
    OwnerRef::new(id('T'), id('P'))
}

fn key(value: &str) -> IdempotencyKey {
    IdempotencyKey::new(value).unwrap()
}

fn version(value: u64) -> Version {
    Version::new(value).unwrap()
}

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5F0{suffix}")).unwrap()
}
