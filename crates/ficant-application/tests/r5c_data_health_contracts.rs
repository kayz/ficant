use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use ficant_application::ports::{
    AccessScope, ApplicationResult, CanonicalQuote, CanonicalSnapshotDecoder, DataSourceRepository,
    DecodedCanonicalQuotes, IntegrityEvent, IntegrityEventSink, PositionSnapshotRepository,
    RegisterDataSource, RequiredVerifiedBlobRead, SnapshotVerifiedReadMetadata,
    SnapshotVerifiedReadMetadataRepository, VerifiedBlobPayload, VerifiedBlobReader,
    VerifiedBlobRole,
};
use ficant_application::{
    ApplicationError, ApplicationErrorDetail, DataHealthQuery, GetDataHealthReport,
    PositionViewsUseCase,
};
use ficant_domain::VersionedDefinition;
use ficant_domain::analytics::FixedDecimal;
use ficant_domain::market::{DataSource, DataSourceInput, DataSourceKind, PriceSourceType};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::research::{
    AccountingBook, AccountingClassification, AccountingClassificationState, DataHealthIssueCode,
    DataHealthThresholdProfile, DataHealthThresholdProfileInput, DataSnapshot, DataSnapshotInput,
    Position, PositionHoldingForm, PositionInput, PositionSnapshot, PositionSnapshotInput,
};

const PREFIX: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA";
const PARQUET: &[u8] = b"r5c-canonical-parquet";
const MANIFEST: &[u8] = b"r5c-canonical-manifest";

#[tokio::test]
async fn reports_the_six_frozen_warning_shapes_from_exact_inputs() {
    let fixture = Fixture::new(None);
    let report = use_case(&fixture)
        .execute(&scope(), query(Some(id('D')), 200))
        .await
        .unwrap();
    let codes = report
        .issues()
        .iter()
        .map(ficant_domain::research::DataHealthIssue::code)
        .collect::<Vec<_>>();
    assert_eq!(
        codes,
        vec![
            DataHealthIssueCode::UnknownAccountingClassification,
            DataHealthIssueCode::StalePositionSnapshot,
            DataHealthIssueCode::UntypedPriceSource,
            DataHealthIssueCode::StaleDataSnapshot,
        ]
    );
    let unknown = &report.issues()[0];
    assert_eq!(unknown.record_count(), 2);
    assert_eq!(unknown.ratio_basis_points(), 5_000);
    assert_eq!(unknown.affected_position_ids(), &[id('A'), id('C')]);
    assert!(report.price_evidence_evaluated());
    assert_eq!(report.coverage().imported_position_count(), 4);
    assert_eq!(fixture.decode_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.source_calls.load(Ordering::SeqCst), 1);

    let model = Fixture::new(Some(PriceSourceType::ModelValuation));
    let report = use_case(&model)
        .execute(&scope(), query(Some(id('D')), 200))
        .await
        .unwrap();
    let model_issue = report
        .issues()
        .iter()
        .find(|issue| issue.code() == DataHealthIssueCode::ModelValuationShare)
        .unwrap();
    assert_eq!(model_issue.record_count(), 4);
    assert_eq!(model_issue.ratio_basis_points(), 10_000);
    assert_eq!(model_issue.data_source_ref(), Some(&source_ref()));
}

#[tokio::test]
async fn absent_price_snapshot_is_explicitly_unchecked_and_performs_no_price_reads() {
    let fixture = Fixture::new(Some(PriceSourceType::ActiveQuote));
    let report = use_case(&fixture)
        .execute(&scope(), query(None, 200))
        .await
        .unwrap();

    assert!(!report.price_evidence_evaluated());
    assert!(report.data_snapshot_id().is_none());
    assert!(report.data_source_ref().is_none());
    assert_eq!(fixture.metadata_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.decode_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.source_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn warning_query_does_not_override_capital_use_unknown_failure() {
    let fixture = Fixture::new(Some(PriceSourceType::ActiveQuote));
    let report = use_case(&fixture)
        .execute(&scope(), query(None, 200))
        .await
        .unwrap();
    assert!(!report.issues().is_empty());

    let error = PositionViewsUseCase::new(&fixture)
        .capital_use(&scope(), id('S'), time(200))
        .await
        .unwrap_err();
    assert!(matches!(
        error.detail(),
        Some(ApplicationErrorDetail::UnknownAccountingPositions { position_ids })
            if position_ids == &vec![id('A').to_string(), id('C').to_string()]
    ));
}

#[tokio::test]
async fn exact_subject_owner_and_visible_time_drift_fail_closed() {
    let fixture = Fixture::new(Some(PriceSourceType::ActiveQuote));
    let wrong_subject = DataHealthQuery::new(
        VersionRef::new(id('Q'), version()),
        id('S'),
        None,
        time(200),
        profile(),
    );
    assert!(
        use_case(&fixture)
            .execute(&scope(), wrong_subject)
            .await
            .is_err()
    );
    assert!(
        use_case(&fixture)
            .execute(&scope(), query(None, 59))
            .await
            .is_err()
    );
}

fn use_case(fixture: &Fixture) -> GetDataHealthReport<'_> {
    GetDataHealthReport::new(fixture, fixture, fixture, fixture, fixture, fixture)
}

struct Fixture {
    position: PositionSnapshot,
    data: DataSnapshot,
    source: DataSource,
    metadata_calls: AtomicUsize,
    decode_calls: AtomicUsize,
    source_calls: AtomicUsize,
}

impl Fixture {
    fn new(source_type: Option<PriceSourceType>) -> Self {
        let mut source = DataSource::new(DataSourceInput {
            data_source_id: source_ref().id().clone(),
            version: source_ref().version(),
            owner: owner(),
            kind: DataSourceKind::FileNdjson,
            name: "r5c exact source".to_owned(),
            connection_binding: "r5c-source".to_owned(),
            dataset: "quotes".to_owned(),
            canonical_schema_id: "ficant.market.quote.v1".to_owned(),
            canonical_schema_hash: ContentHash::digest(b"schema"),
        })
        .unwrap();
        if let Some(source_type) = source_type {
            source = source.with_price_source_type(source_type).unwrap();
        }
        Self {
            position: position_snapshot(),
            data: data_snapshot(),
            source,
            metadata_calls: AtomicUsize::new(0),
            decode_calls: AtomicUsize::new(0),
            source_calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl PositionSnapshotRepository for Fixture {
    async fn get_position_snapshot(
        &self,
        _: &AccessScope,
        snapshot_id: Ulid,
        _: MarketTime,
    ) -> ApplicationResult<Option<PositionSnapshot>> {
        Ok((snapshot_id == *self.position.id()).then(|| self.position.clone()))
    }

    async fn resolve_position_snapshot(
        &self,
        _: &AccessScope,
        _: VersionRef,
        _: MarketTime,
        _: MarketTime,
    ) -> ApplicationResult<Option<PositionSnapshot>> {
        Ok(None)
    }
}

#[async_trait]
impl SnapshotVerifiedReadMetadataRepository for Fixture {
    async fn get_verified_read_metadata(
        &self,
        _: &AccessScope,
        snapshot_id: Ulid,
    ) -> ApplicationResult<Option<SnapshotVerifiedReadMetadata>> {
        self.metadata_calls.fetch_add(1, Ordering::SeqCst);
        if snapshot_id != *self.data.id() {
            return Ok(None);
        }
        SnapshotVerifiedReadMetadata::data(
            self.data.clone(),
            PARQUET.len() as u64,
            MANIFEST.len() as u64,
        )
        .map(Some)
    }
}

#[async_trait]
impl VerifiedBlobReader for Fixture {
    async fn read_required(
        &self,
        request: &RequiredVerifiedBlobRead,
        sink: &dyn IntegrityEventSink,
    ) -> ApplicationResult<VerifiedBlobPayload> {
        let bytes = match request.blob_role() {
            VerifiedBlobRole::DataParquet => PARQUET,
            VerifiedBlobRole::DataManifest => MANIFEST,
            _ => unreachable!("R5c reads only the two DataSnapshot roles"),
        };
        request.verify_bytes(sink, bytes.to_vec()).await
    }
}

#[async_trait]
impl IntegrityEventSink for Fixture {
    async fn emit(&self, _: IntegrityEvent) -> ApplicationResult<()> {
        unreachable!("fixture hashes and sizes are exact")
    }
}

#[async_trait]
impl CanonicalSnapshotDecoder for Fixture {
    async fn decode_quotes(
        &self,
        snapshot: &DataSnapshot,
        parquet: &[u8],
        manifest: &[u8],
    ) -> ApplicationResult<DecodedCanonicalQuotes> {
        self.decode_calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(snapshot, &self.data);
        assert_eq!(parquet, PARQUET);
        assert_eq!(manifest, MANIFEST);
        DecodedCanonicalQuotes::new(
            source_ref(),
            ['A', 'B', 'C', 'D']
                .into_iter()
                .map(|suffix| {
                    CanonicalQuote::new(
                        VersionRef::new(id(suffix), version()),
                        time(10),
                        time(20),
                        time(10).local_trading_date(),
                        Some(FixedDecimal::from_scaled(100)),
                        Some(FixedDecimal::from_scaled(102)),
                        unit('C'),
                    )
                })
                .collect(),
        )
    }
}

#[async_trait]
impl DataSourceRepository for Fixture {
    async fn register(&self, _: RegisterDataSource) -> Result<DataSource, ApplicationError> {
        unreachable!("R5c is read-only")
    }

    async fn get_exact(
        &self,
        _: &AccessScope,
        reference: VersionRef,
    ) -> Result<Option<DataSource>, ApplicationError> {
        self.source_calls.fetch_add(1, Ordering::SeqCst);
        Ok((self.source.identity() == reference.id().as_str()
            && self.source.version() == reference.version().get())
        .then(|| self.source.clone()))
    }
}

fn position_snapshot() -> PositionSnapshot {
    let mut input = PositionSnapshotInput {
        snapshot_id: id('S'),
        owner: owner(),
        subject_ref: subject_ref(),
        observed_at: time(0),
        visible_at: time(60),
        content_hash: ContentHash::digest(b"pending"),
        lineage: vec![LineageRef::content_addressed(
            id('L'),
            ContentHash::digest(b"positions"),
        )],
        positions: [
            ('A', AccountingClassificationState::Unknown),
            ('B', AccountingClassificationState::Classified),
            ('C', AccountingClassificationState::Unknown),
            ('D', AccountingClassificationState::Classified),
        ]
        .into_iter()
        .map(|(suffix, state)| position(suffix, state))
        .collect(),
    };
    input.content_hash = PositionSnapshot::content_hash_for(&input);
    PositionSnapshot::new(input).unwrap()
}

fn data_snapshot() -> DataSnapshot {
    DataSnapshot::new(DataSnapshotInput {
        data_snapshot_id: id('D'),
        owner: owner(),
        visible_at: time(20),
        as_of: time(10),
        schema_hash: ContentHash::digest(b"schema"),
        manifest_hash: ContentHash::digest(MANIFEST),
        blob_content_hash: ContentHash::digest(PARQUET),
        lineage: vec![LineageRef::content_addressed(
            id('E'),
            ContentHash::digest(b"data-lineage"),
        )],
    })
    .unwrap()
}

fn position(suffix: char, state: AccountingClassificationState) -> Position {
    Position::new(PositionInput {
        position_id: id(suffix),
        instrument_ref: VersionRef::new(id('I'), version()),
        quantity: decimal("1", unit('N')),
        economic_value: decimal("100", unit('C')),
        economic_pnl: decimal("0", unit('C')),
        accounting_pnl: decimal("0", unit('C')),
        capital_requirement: decimal("10", unit('C')),
        accounting_classification: AccountingClassification::new(
            state,
            (state == AccountingClassificationState::Classified).then_some(AccountingBook::Ac),
        )
        .unwrap(),
        holding_form: PositionHoldingForm::Owned,
    })
    .unwrap()
}

fn query(data_snapshot_id: Option<Ulid>, evaluated_seconds: i64) -> DataHealthQuery {
    DataHealthQuery::new(
        subject_ref(),
        id('S'),
        data_snapshot_id,
        time(evaluated_seconds),
        profile(),
    )
}

fn profile() -> DataHealthThresholdProfile {
    let mut input = DataHealthThresholdProfileInput {
        profile_ref: VersionRef::new(id('P'), version()),
        max_position_snapshot_age_seconds: 100,
        unknown_accounting_warning_basis_points: 5_000,
        max_data_snapshot_age_seconds: 100,
        model_valuation_warning_basis_points: 5_000,
        content_hash: ContentHash::digest(b"pending"),
    };
    input.content_hash = DataHealthThresholdProfile::content_hash_for(&input);
    DataHealthThresholdProfile::new(input).unwrap()
}

fn scope() -> AccessScope {
    AccessScope::new(id('T'), id('X'), vec![id('O')]).unwrap()
}

fn owner() -> OwnerRef {
    OwnerRef::new(id('T'), id('O'))
}

fn subject_ref() -> VersionRef {
    VersionRef::new(id('U'), version())
}

fn source_ref() -> VersionRef {
    VersionRef::new(id('R'), version())
}

fn decimal(value: &str, unit: UnitRef) -> DecimalValue {
    DecimalValue::new(value, 0, unit).unwrap()
}

fn unit(suffix: char) -> UnitRef {
    UnitRef::new(id(suffix), version())
}

fn time(seconds: i64) -> MarketTime {
    let instant = Utc.timestamp_opt(1_767_225_600 + seconds, 0).unwrap();
    MarketTime::new(instant, "UTC", instant.date_naive()).unwrap()
}

fn version() -> Version {
    Version::new(1).unwrap()
}

fn id(suffix: char) -> Ulid {
    let suffix = match suffix {
        'I' => '1',
        'L' => '2',
        'O' => '3',
        'U' => '4',
        value => value,
    };
    Ulid::new(format!("{PREFIX}{suffix}")).unwrap()
}

// Reuse the frozen R4d-b economic fixture without editing its protected test file. Keeping the
// fixture inside a module also proves that the existing KRD contract remains intact while R5c
// varies only accounting-classification and freshness facts on the immutable PositionSnapshot.
mod krd_health_regression {
    include!("r4d_b_futures_krd_contracts.rs");

    #[tokio::test]
    async fn worst_health_is_non_blocking_side_effect_free_and_never_degrades_krd() {
        let healthy_fixture = Fixture::new(true, false, Calls::default());
        let healthy_result = healthy_fixture.execute(true).await.unwrap();
        let healthy_report = health_report(&healthy_fixture).await;
        assert!(healthy_report.issues().is_empty());

        let mut control_fixture = Fixture::new(true, false, Calls::default());
        control_fixture.snapshot = worst_health_snapshot(&control_fixture.snapshot);
        let without_health_query = control_fixture.execute(true).await.unwrap();
        assert_eq!(without_health_query.positions().len(), 2);
        assert_eq!(without_health_query.totals().len(), 3);
        assert_eq!(
            without_health_query
                .coverage()
                .participating_position_count(),
            2
        );

        let mut queried_fixture = Fixture::new(true, false, Calls::default());
        queried_fixture.snapshot = worst_health_snapshot(&queried_fixture.snapshot);
        let worst_report = health_report(&queried_fixture).await;
        assert_eq!(
            worst_report
                .issues()
                .iter()
                .map(ficant_domain::research::DataHealthIssue::code)
                .collect::<Vec<_>>(),
            vec![
                ficant_domain::research::DataHealthIssueCode::UnknownAccountingClassification,
                ficant_domain::research::DataHealthIssueCode::StalePositionSnapshot,
            ]
        );
        assert_eq!(worst_report.issues()[0].ratio_basis_points(), 10_000);

        let after_health_query = queried_fixture.execute(true).await.unwrap();
        assert_eq!(
            after_health_query, without_health_query,
            "the health query cannot mutate state, caches, or subsequent KRD bytes"
        );

        assert_eq!(
            without_health_query
                .positions()
                .iter()
                .map(|position| (
                    position.position_id(),
                    position.instrument(),
                    position.exposures(),
                ))
                .collect::<Vec<_>>(),
            healthy_result
                .positions()
                .iter()
                .map(|position| (
                    position.position_id(),
                    position.instrument(),
                    position.exposures(),
                ))
                .collect::<Vec<_>>()
        );
        assert_eq!(without_health_query.totals(), healthy_result.totals());
        assert_eq!(without_health_query.algorithm(), healthy_result.algorithm());
        assert_eq!(
            without_health_query.algorithm().algorithm_id(),
            "ficant.fixed-income.portfolio-key-rate-yield"
        );
        assert_eq!(without_health_query.algorithm().algorithm_version(), 1);
        assert_eq!(
            without_health_query.algorithm().convention_profile(),
            "linear-ytm-fixed-base-ctd-v1"
        );

        let capital_error = ficant_application::PositionViewsUseCase::new(&queried_fixture)
            .capital_use(
                &queried_fixture.scope,
                queried_fixture.snapshot.id().clone(),
                time(2),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            capital_error.detail(),
            Some(ficant_application::ApplicationErrorDetail::UnknownAccountingPositions {
                position_ids
            }) if position_ids == &vec![id('Q').to_string(), id('Z').to_string()]
        ));
    }

    async fn health_report(fixture: &Fixture) -> ficant_domain::research::DataHealthReport {
        ficant_application::GetDataHealthReport::new(
            fixture, fixture, fixture, fixture, fixture, fixture,
        )
        .execute(
            &fixture.scope,
            ficant_application::DataHealthQuery::new(
                fixture.snapshot.subject_ref().clone(),
                fixture.snapshot.id().clone(),
                None,
                time(2),
                health_profile(),
            ),
        )
        .await
        .unwrap()
    }

    fn health_profile() -> ficant_domain::research::DataHealthThresholdProfile {
        let mut input = ficant_domain::research::DataHealthThresholdProfileInput {
            profile_ref: VersionRef::new(id('P'), version()),
            max_position_snapshot_age_seconds: 5_400,
            unknown_accounting_warning_basis_points: 1,
            max_data_snapshot_age_seconds: 5_400,
            model_valuation_warning_basis_points: 1,
            content_hash: ContentHash::digest(b"pending"),
        };
        input.content_hash =
            ficant_domain::research::DataHealthThresholdProfile::content_hash_for(&input);
        ficant_domain::research::DataHealthThresholdProfile::new(input).unwrap()
    }

    fn worst_health_snapshot(snapshot: &PositionSnapshot) -> PositionSnapshot {
        let positions = snapshot
            .positions()
            .iter()
            .map(|position| {
                Position::new(PositionInput {
                    position_id: position.id().clone(),
                    instrument_ref: position.instrument_ref().clone(),
                    quantity: position.quantity().clone(),
                    economic_value: position.economic_value().clone(),
                    economic_pnl: position.economic_pnl().clone(),
                    accounting_pnl: position.accounting_pnl().clone(),
                    capital_requirement: position.capital_requirement().clone(),
                    accounting_classification: AccountingClassification::new(
                        AccountingClassificationState::Unknown,
                        None,
                    )
                    .unwrap(),
                    holding_form: position.holding_form(),
                })
                .unwrap()
            })
            .collect();
        let mut input = PositionSnapshotInput {
            snapshot_id: snapshot.id().clone(),
            owner: snapshot.owner().clone(),
            subject_ref: snapshot.subject_ref().clone(),
            observed_at: time_for(2026, 8, 2, 0),
            visible_at: snapshot.visible_at().clone(),
            content_hash: ContentHash::digest(b"pending"),
            lineage: snapshot.lineage().to_vec(),
            positions,
        };
        input.content_hash = PositionSnapshot::content_hash_for(&input);
        PositionSnapshot::new(input).unwrap()
    }
}
