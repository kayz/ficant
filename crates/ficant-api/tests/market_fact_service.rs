use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use ficant_api::{
    MarketFactGrpcService, PlatformApplication, PlatformPort, SessionPolicy, SystemClock,
    TrustedIdentity,
};
use ficant_application::ports::{
    AccessScope, AeadCursorCodec, AppendDefinitionVersion, AppendMarketFact, ApplicationResult,
    BeginBlobStage, BlobStore, CorrectMarketFact, CursorKey, CursorPage, CurveSnapshotMetadata,
    CurveSnapshotMetadataRepository, DefinitionIdentity, DefinitionRepository, DefinitionValue,
    GovernedAppendMarketFact, GovernedCorrectMarketFact, GovernedPublishCurveSnapshot,
    IntegrityEvent, IntegrityEventSink, IntegrityFailureReason, MarketFact, MarketFactRepository,
    MarketFactWindow, PublishCurveSnapshot, RequiredVerifiedBlobRead, StagedBlobRef,
    VerifiedBlobPayload, VerifiedBlobReader, VerifiedBlobRef, VerifyBlobStage,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::market::v1 as pb;
use ficant_contracts::ficant::market::v1::market_fact_service_server::MarketFactService;
use ficant_domain::governance::{FoundationChangeOperation, PlatformRole};
use ficant_domain::market::{
    Calendar, CalendarInput, CalendarSession, MarketRulePack, MarketRulePackInput, Unit, UnitInput,
    VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, EffectivePeriod, MarketTime, OwnerRef, Ulid, Version,
};
use prost::Message;
use prost_types::Timestamp;
use tonic::Request;

const KEY: &[u8] = b"0123456789abcdef0123456789abcdef";

struct Definitions {
    values: BTreeMap<(String, u64), DefinitionValue>,
}

impl Definitions {
    fn fixture() -> Self {
        let values = [
            DefinitionValue::Unit(unit('M', "CNY", "currency", 2)),
            DefinitionValue::Unit(unit('P', "PRICE", "price", 4)),
            DefinitionValue::Unit(unit('N', "NOTIONAL", "notional", 2)),
            DefinitionValue::Unit(unit('R', "RATE", "rate", 6)),
            DefinitionValue::Calendar(calendar()),
            DefinitionValue::MarketRulePack(rule_pack()),
        ]
        .into_iter()
        .map(|value| ((value.identity().to_owned(), value.version()), value))
        .collect();
        Self { values }
    }
}

#[async_trait]
impl DefinitionRepository for Definitions {
    async fn create_identity(&self, _: DefinitionIdentity) -> ApplicationResult<()> {
        Err(not_used())
    }

    async fn append_version(
        &self,
        _: AppendDefinitionVersion,
    ) -> ApplicationResult<DefinitionValue> {
        Err(not_used())
    }

    async fn get_version(
        &self,
        scope: &AccessScope,
        definition_id: Ulid,
        version: Version,
    ) -> ApplicationResult<Option<DefinitionValue>> {
        let value = self
            .values
            .get(&(definition_id.as_str().to_owned(), version.get()))
            .cloned();
        if let Some(value) = value.as_ref() {
            scope.authorize(value.owner())?;
        }
        Ok(value)
    }

    async fn resolve_as_of(
        &self,
        _: &AccessScope,
        _: Ulid,
        _: MarketTime,
    ) -> ApplicationResult<Option<DefinitionValue>> {
        Err(not_used())
    }
}

#[derive(Default)]
struct Repository {
    facts: Mutex<Vec<MarketFact>>,
    curve: Mutex<Option<(ficant_domain::market::CurveSnapshot, u64)>>,
    stages: Mutex<BTreeMap<String, (OwnerRef, Vec<u8>)>>,
    promoted: Mutex<Option<Vec<u8>>>,
    governed_appends: AtomicUsize,
    governed_corrections: AtomicUsize,
    governed_curves: AtomicUsize,
    legacy_writes: AtomicUsize,
    stage_begins: AtomicUsize,
    verified_reads: AtomicUsize,
    integrity_events: AtomicUsize,
}

#[async_trait]
impl MarketFactRepository for Repository {
    async fn append_governed_fact(
        &self,
        command: GovernedAppendMarketFact,
    ) -> ApplicationResult<MarketFact> {
        assert_eq!(
            command.change_record()?.operation(),
            FoundationChangeOperation::AppendMarketFact,
        );
        self.governed_appends.fetch_add(1, Ordering::SeqCst);
        let value = command.command().fact().clone();
        self.facts.lock().unwrap().push(value.clone());
        Ok(value)
    }

    async fn append_governed_correction(
        &self,
        command: GovernedCorrectMarketFact,
    ) -> ApplicationResult<MarketFact> {
        assert_eq!(
            command
                .change_record(ContentHash::digest(b"original"))?
                .operation(),
            FoundationChangeOperation::CorrectMarketFact,
        );
        self.governed_corrections.fetch_add(1, Ordering::SeqCst);
        let value = command.command().correction().clone();
        self.facts.lock().unwrap().push(value.clone());
        Ok(value)
    }

    async fn publish_governed_curve_snapshot(
        &self,
        command: GovernedPublishCurveSnapshot,
    ) -> ApplicationResult<ficant_domain::market::CurveSnapshot> {
        assert_eq!(
            command.change_record()?.operation(),
            FoundationChangeOperation::PublishCurveSnapshot,
        );
        self.governed_curves.fetch_add(1, Ordering::SeqCst);
        let value = command.command().curve().clone();
        *self.curve.lock().unwrap() = Some((value.clone(), command.command().declared_blob_size()));
        Ok(value)
    }

    async fn append_fact(&self, _: AppendMarketFact) -> ApplicationResult<MarketFact> {
        self.legacy_writes.fetch_add(1, Ordering::SeqCst);
        Err(not_used())
    }

    async fn append_correction(&self, _: CorrectMarketFact) -> ApplicationResult<MarketFact> {
        self.legacy_writes.fetch_add(1, Ordering::SeqCst);
        Err(not_used())
    }

    async fn query_instrument_window(
        &self,
        scope: &AccessScope,
        query: MarketFactWindow,
    ) -> ApplicationResult<CursorPage<MarketFact>> {
        query.authorize_scope(scope)?;
        Ok(CursorPage::new(self.facts.lock().unwrap().clone(), None))
    }

    async fn publish_curve_snapshot(
        &self,
        _: PublishCurveSnapshot,
    ) -> ApplicationResult<ficant_domain::market::CurveSnapshot> {
        self.legacy_writes.fetch_add(1, Ordering::SeqCst);
        Err(not_used())
    }

    async fn get_curve_snapshot(
        &self,
        scope: &AccessScope,
        curve_snapshot_id: Ulid,
    ) -> ApplicationResult<Option<ficant_domain::market::CurveSnapshot>> {
        let value = self.curve.lock().unwrap().clone().map(|value| value.0);
        if let Some(value) = value.as_ref() {
            scope.authorize(value.owner())?;
            assert_eq!(value.id(), &curve_snapshot_id);
        }
        Ok(value)
    }

    async fn get_curve_snapshot_at(
        &self,
        scope: &AccessScope,
        curve_snapshot_id: Ulid,
        knowledge_at: &MarketTime,
    ) -> ApplicationResult<Option<ficant_domain::market::CurveSnapshot>> {
        let value = self.curve.lock().unwrap().clone().map(|value| value.0);
        let Some(value) = value else {
            return Ok(None);
        };
        scope.authorize(value.owner())?;
        assert_eq!(value.id(), &curve_snapshot_id);
        let is_visible = value
            .visible_at()
            .is_some_and(|visible_at| visible_at.instant() <= knowledge_at.instant());
        Ok(is_visible.then_some(value))
    }
}

#[async_trait]
impl CurveSnapshotMetadataRepository for Repository {
    async fn get_curve_snapshot_metadata(
        &self,
        scope: &AccessScope,
        curve_snapshot_id: Ulid,
    ) -> ApplicationResult<Option<CurveSnapshotMetadata>> {
        let value = self.curve.lock().unwrap().clone();
        match value {
            None => Ok(None),
            Some((snapshot, size)) => {
                scope.authorize(snapshot.owner())?;
                if snapshot.id() != &curve_snapshot_id {
                    return Ok(None);
                }
                CurveSnapshotMetadata::new(snapshot, size).map(Some)
            }
        }
    }
}

#[async_trait]
impl BlobStore for Repository {
    async fn begin_stage(&self, command: BeginBlobStage) -> ApplicationResult<StagedBlobRef> {
        self.stage_begins.fetch_add(1, Ordering::SeqCst);
        let staged = StagedBlobRef::new(id('S'), command.owner().clone());
        self.stages.lock().unwrap().insert(
            staged.id().as_str().to_owned(),
            (command.owner().clone(), Vec::new()),
        );
        Ok(staged)
    }

    async fn append_chunk(
        &self,
        scope: &AccessScope,
        staged: &StagedBlobRef,
        chunk: Vec<u8>,
    ) -> ApplicationResult<()> {
        staged.authorize(scope)?;
        self.stages
            .lock()
            .unwrap()
            .get_mut(staged.id().as_str())
            .ok_or_else(not_used)?
            .1
            .extend(chunk);
        Ok(())
    }

    async fn verify_and_promote(
        &self,
        command: VerifyBlobStage,
    ) -> ApplicationResult<VerifiedBlobRef> {
        let bytes = self
            .stages
            .lock()
            .unwrap()
            .remove(command.staged().id().as_str())
            .ok_or_else(not_used)?
            .1;
        let size = u64::try_from(bytes.len()).unwrap();
        let actual_hash = ContentHash::digest(&bytes);
        if size != command.expected_size() || &actual_hash != command.expected_hash() {
            return Err(ApplicationError::new(
                ApplicationErrorCategory::HashMismatch,
                false,
            ));
        }
        *self.promoted.lock().unwrap() = Some(bytes);
        VerifiedBlobRef::new(actual_hash, size)
    }

    async fn discard_stage(
        &self,
        _: &AccessScope,
        staged: &StagedBlobRef,
    ) -> ApplicationResult<()> {
        self.stages.lock().unwrap().remove(staged.id().as_str());
        Ok(())
    }
}

#[async_trait]
impl VerifiedBlobReader for Repository {
    async fn read_required(
        &self,
        request: &RequiredVerifiedBlobRead,
        sink: &dyn IntegrityEventSink,
    ) -> ApplicationResult<VerifiedBlobPayload> {
        self.verified_reads.fetch_add(1, Ordering::SeqCst);
        let promoted = self.promoted.lock().unwrap().clone();
        match promoted {
            Some(bytes) => request.verify_bytes(sink, bytes).await,
            None => Err(request
                .fail_integrity(sink, IntegrityFailureReason::Missing)
                .await),
        }
    }
}

#[async_trait]
impl IntegrityEventSink for Repository {
    async fn emit(&self, _: IntegrityEvent) -> ApplicationResult<()> {
        self.integrity_events.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn admin_round_trips_all_fact_variants_and_both_roles_query_them() {
    fn assert_service<T: MarketFactService>() {}
    assert_service::<MarketFactGrpcService>();

    let repository = Arc::new(Repository::default());
    let definitions = Arc::new(Definitions::fixture());
    let admin = service(
        repository.clone(),
        definitions.clone(),
        PlatformRole::PlatformAdmin,
    );
    let researcher = service(repository.clone(), definitions, PlatformRole::Researcher);
    let fixtures = [
        cashflow(None),
        quote('Q', None),
        trade(None),
        valuation(None),
    ];
    for (index, fixture) in fixtures.iter().enumerate() {
        assert_eq!(
            append(&admin, fixture.clone(), &format!("append-{index}"))
                .await
                .unwrap(),
            *fixture,
        );
    }

    let correction = quote('E', Some('Q'));
    let response = admin
        .correct_market_fact(Request::new(pb::CorrectMarketFactRequest {
            idempotency_key: "correct-quote".to_owned(),
            original_fact_id: Some(proto_id('Q')),
            fact: Some(correction.clone()),
            change: Some(change()),
        }))
        .await
        .unwrap()
        .into_inner();
    let Some(pb::correct_market_fact_response::Result::Fact(corrected)) = response.result else {
        panic!("Platform Admin must append an explicit immutable correction");
    };
    assert_eq!(corrected, correction);

    for reader in [&admin, &researcher] {
        let response = reader
            .query_instrument_facts(Request::new(pb::QueryInstrumentFactsRequest {
                instrument: Some(version_ref('B')),
                from: Some(market_time(0)),
                to: Some(market_time(8)),
                knowledge_at: Some(market_time(8)),
                page: Some(core::PageRequest {
                    page_size: 20,
                    cursor: String::new(),
                }),
            }))
            .await
            .unwrap()
            .into_inner();
        let Some(pb::query_instrument_facts_response::Result::InstrumentFacts(page)) =
            response.result
        else {
            panic!("both active roles with read scope must query facts");
        };
        assert_eq!(page.facts.len(), 5);
        assert!(matches!(
            page.facts[0].fact,
            Some(pb::market_fact::Fact::Cashflow(_))
        ));
        assert!(matches!(
            page.facts[1].fact,
            Some(pb::market_fact::Fact::Quote(_))
        ));
        assert!(matches!(
            page.facts[2].fact,
            Some(pb::market_fact::Fact::Trade(_))
        ));
        assert!(matches!(
            page.facts[3].fact,
            Some(pb::market_fact::Fact::Valuation(_))
        ));
    }

    assert_missing_fact_knowledge_fails(&researcher).await;

    assert_eq!(repository.governed_appends.load(Ordering::SeqCst), 4);
    assert_eq!(repository.governed_corrections.load(Ordering::SeqCst), 1);
    assert_eq!(repository.legacy_writes.load(Ordering::SeqCst), 0);
}

async fn assert_missing_fact_knowledge_fails(service: &MarketFactGrpcService) {
    let response = service
        .query_instrument_facts(Request::new(pb::QueryInstrumentFactsRequest {
            instrument: Some(version_ref('B')),
            from: Some(market_time(0)),
            to: Some(market_time(8)),
            page: Some(core::PageRequest {
                page_size: 20,
                cursor: String::new(),
            }),
            knowledge_at: None,
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        response.result,
        Some(pb::query_instrument_facts_response::Result::Error(core::ErrorDetail {
            code,
            ..
        })) if code == core::ErrorCode::ValidationFailed as i32
    ));
}

#[tokio::test]
async fn curve_publish_hashes_canonical_points_and_get_requires_verified_bytes() {
    let repository = Arc::new(Repository::default());
    let definitions = Arc::new(Definitions::fixture());
    let admin = service(
        repository.clone(),
        definitions.clone(),
        PlatformRole::PlatformAdmin,
    );
    let researcher = service(repository.clone(), definitions, PlatformRole::Researcher);
    let points = curve_points();
    let response = admin
        .publish_curve_snapshot(Request::new(pb::PublishCurveSnapshotRequest {
            idempotency_key: "publish-curve".to_owned(),
            points: Some(points.clone()),
            change: Some(change()),
            curve: Some(curve_input()),
        }))
        .await
        .unwrap()
        .into_inner();
    let Some(pb::publish_curve_snapshot_response::Result::CurveSnapshot(curve)) = response.result
    else {
        panic!("Platform Admin must publish a verified curve fixture");
    };
    assert_eq!(
        curve.content_hash,
        Some(hash(&points.encode_to_vec())),
        "the server, not the caller, owns the canonical point hash",
    );

    for knowledge_at in [None, Some(market_time(5))] {
        let response = researcher
            .get_curve_snapshot(Request::new(pb::GetCurveSnapshotRequest {
                curve_snapshot_id: Some(proto_id('X')),
                knowledge_at,
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(matches!(
            response.result,
            Some(pb::get_curve_snapshot_response::Result::Error(_))
        ));
        assert_eq!(
            repository.verified_reads.load(Ordering::SeqCst),
            0,
            "missing or future knowledge must fail before immutable blob reads"
        );
    }

    for reader in [&admin, &researcher] {
        let response = reader
            .get_curve_snapshot(Request::new(pb::GetCurveSnapshotRequest {
                curve_snapshot_id: Some(proto_id('X')),
                knowledge_at: Some(market_time(6)),
            }))
            .await
            .unwrap()
            .into_inner();
        let Some(pb::get_curve_snapshot_response::Result::Curve(payload)) = response.result else {
            panic!("get must return metadata only together with verified decoded points");
        };
        assert_eq!(payload.curve_snapshot.as_ref(), Some(&curve));
        assert_eq!(payload.points, Some(points.clone()));
    }
    *repository.promoted.lock().unwrap() = Some(b"tampered-curve-points".to_vec());
    let tampered = researcher
        .get_curve_snapshot(Request::new(pb::GetCurveSnapshotRequest {
            curve_snapshot_id: Some(proto_id('X')),
            knowledge_at: Some(market_time(6)),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        tampered.result,
        Some(pb::get_curve_snapshot_response::Result::Error(core::ErrorDetail {
            code,
            ..
        })) if code == core::ErrorCode::HashMismatch as i32
    ));
    assert_eq!(repository.stage_begins.load(Ordering::SeqCst), 1);
    assert_eq!(repository.governed_curves.load(Ordering::SeqCst), 1);
    assert_eq!(repository.verified_reads.load(Ordering::SeqCst), 3);
    assert_eq!(repository.integrity_events.load(Ordering::SeqCst), 1);
    assert_eq!(repository.legacy_writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn role_unknown_cashflow_and_unresolved_units_fail_before_writes_or_blob_staging() {
    let repository = Arc::new(Repository::default());
    let definitions = Arc::new(Definitions::fixture());
    let researcher = service(
        repository.clone(),
        definitions.clone(),
        PlatformRole::Researcher,
    );
    let denied = researcher
        .append_market_fact(Request::new(pb::AppendMarketFactRequest {
            idempotency_key: "researcher-direct".to_owned(),
            fact: Some(quote('Q', None)),
            change: Some(change()),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_append_error(denied, core::ErrorCode::Forbidden);
    let denied_curve = researcher
        .publish_curve_snapshot(Request::new(pb::PublishCurveSnapshotRequest {
            idempotency_key: "researcher-curve".to_owned(),
            points: Some(curve_points()),
            change: Some(change()),
            curve: Some(curve_input()),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        denied_curve.result,
        Some(pb::publish_curve_snapshot_response::Result::Error(core::ErrorDetail {
            code,
            ..
        })) if code == core::ErrorCode::Forbidden as i32
    ));

    let admin = service(repository.clone(), definitions, PlatformRole::PlatformAdmin);
    assert_append_error(
        admin
            .append_market_fact(Request::new(pb::AppendMarketFactRequest {
                idempotency_key: "missing-change-source".to_owned(),
                fact: Some(quote('G', None)),
                change: Some(core::ChangeJustification {
                    reason: "reason without evidence".to_owned(),
                    sources: vec![],
                }),
            }))
            .await
            .unwrap()
            .into_inner(),
        core::ErrorCode::ValidationFailed,
    );
    let mut unspecified = cashflow(None);
    let Some(pb::market_fact::Fact::Cashflow(value)) = unspecified.fact.as_mut() else {
        unreachable!();
    };
    value.cashflow_type = pb::CashflowType::Unspecified as i32;
    assert_append_error(
        admin
            .append_market_fact(Request::new(pb::AppendMarketFactRequest {
                idempotency_key: "unspecified-cashflow".to_owned(),
                fact: Some(unspecified),
                change: Some(change()),
            }))
            .await
            .unwrap()
            .into_inner(),
        core::ErrorCode::ValidationFailed,
    );
    let unresolved = pb::MarketFact {
        fact: Some(pb::market_fact::Fact::Quote(pb::Quote {
            bid: Some(decimal_with_unit("100", 2, 'Y')),
            ..match quote('Y', None).fact.unwrap() {
                pb::market_fact::Fact::Quote(value) => value,
                _ => unreachable!(),
            }
        })),
    };
    assert_append_error(
        admin
            .append_market_fact(Request::new(pb::AppendMarketFactRequest {
                idempotency_key: "unresolved-unit".to_owned(),
                fact: Some(unresolved),
                change: Some(change()),
            }))
            .await
            .unwrap()
            .into_inner(),
        core::ErrorCode::ValidationFailed,
    );
    assert_eq!(repository.governed_appends.load(Ordering::SeqCst), 0);
    assert_eq!(repository.governed_curves.load(Ordering::SeqCst), 0);
    assert_eq!(repository.stage_begins.load(Ordering::SeqCst), 0);
    assert_eq!(repository.legacy_writes.load(Ordering::SeqCst), 0);
}

async fn append(
    service: &MarketFactGrpcService,
    fact: pb::MarketFact,
    key: &str,
) -> Option<pb::MarketFact> {
    let response = service
        .append_market_fact(Request::new(pb::AppendMarketFactRequest {
            idempotency_key: key.to_owned(),
            fact: Some(fact),
            change: Some(change()),
        }))
        .await
        .unwrap()
        .into_inner();
    match response.result {
        Some(pb::append_market_fact_response::Result::Fact(value)) => Some(value),
        Some(pb::append_market_fact_response::Result::Error(error)) => {
            panic!("unexpected append error for {key}: {}", error.code)
        }
        None => None,
    }
}

fn service(
    repository: Arc<Repository>,
    definitions: Arc<Definitions>,
    role: PlatformRole,
) -> MarketFactGrpcService {
    let identity = TrustedIdentity::implicit(
        "market-fact-test",
        id('W'),
        id('T'),
        vec![id('A')],
        role,
        ["facts:read", "facts:write"],
    )
    .unwrap();
    let application: Arc<dyn PlatformPort> = Arc::new(
        PlatformApplication::try_new(
            Arc::new(SystemClock),
            SessionPolicy::new(900, 60).unwrap(),
            KEY,
            vec![],
            Some(identity),
            vec![],
        )
        .unwrap(),
    );
    let cursor = Arc::new(
        AeadCursorCodec::new(CursorKey::new("market-fact", [9_u8; 32]).unwrap(), vec![]).unwrap(),
    );
    MarketFactGrpcService::new(
        application,
        repository.clone(),
        definitions,
        repository.clone(),
        repository.clone(),
        repository.clone(),
        repository,
        cursor,
        KEY,
    )
    .unwrap()
}

fn cashflow(supersedes: Option<char>) -> pb::MarketFact {
    pb::MarketFact {
        fact: Some(pb::market_fact::Fact::Cashflow(pb::Cashflow {
            cashflow_id: Some(proto_id('F')),
            bond: Some(version_ref('B')),
            payment_time: Some(market_time(2)),
            amount: Some(decimal_with_unit("225", 2, 'M')),
            owner: Some(owner()),
            source: Some(source("cashflow")),
            supersedes_id: supersedes.map(proto_id),
            schedule_id: "bond-cashflows".to_owned(),
            sequence: 1,
            cashflow_type: pb::CashflowType::Coupon as i32,
        })),
    }
}

fn quote(id_suffix: char, supersedes: Option<char>) -> pb::MarketFact {
    pb::MarketFact {
        fact: Some(pb::market_fact::Fact::Quote(pb::Quote {
            quote_id: Some(proto_id(id_suffix)),
            instrument: Some(version_ref('B')),
            owner: Some(owner()),
            source: Some(source("quote")),
            observed_at: Some(market_time(2)),
            received_at: Some(market_time(3)),
            bid: Some(decimal_with_unit("9999", 2, 'P')),
            ask: Some(decimal_with_unit("10001", 2, 'P')),
            supersedes_id: supersedes.map(proto_id),
        })),
    }
}

fn trade(supersedes: Option<char>) -> pb::MarketFact {
    pb::MarketFact {
        fact: Some(pb::market_fact::Fact::Trade(pb::Trade {
            trade_id: Some(proto_id('J')),
            instrument: Some(version_ref('B')),
            owner: Some(owner()),
            source: Some(source("trade")),
            executed_at: Some(market_time(4)),
            price: Some(decimal_with_unit("100", 0, 'P')),
            quantity: Some(decimal_with_unit("10000", 0, 'N')),
            supersedes_id: supersedes.map(proto_id),
        })),
    }
}

fn valuation(supersedes: Option<char>) -> pb::MarketFact {
    pb::MarketFact {
        fact: Some(pb::market_fact::Fact::Valuation(pb::Valuation {
            valuation_id: Some(proto_id('V')),
            instrument: Some(version_ref('B')),
            owner: Some(owner()),
            source: Some(source("valuation")),
            valuation_at: Some(market_time(5)),
            method: "external-close".to_owned(),
            rule_pack: Some(version_ref('K')),
            values: vec![decimal_with_unit("10002", 2, 'P')],
            supersedes_id: supersedes.map(proto_id),
        })),
    }
}

fn curve_input() -> pb::CurveSnapshotInput {
    pb::CurveSnapshotInput {
        curve_snapshot_id: Some(proto_id('X')),
        owner: Some(owner()),
        as_of: Some(market_time(5)),
        currency: Some(unit_ref('M')),
        curve_kind: "government-zero".to_owned(),
        calendar: Some(version_ref('C')),
        rule_pack: Some(version_ref('K')),
        point_schema: "ficant.yield-curve-points.protobuf.v1".to_owned(),
        lineage: vec![core::LineageRef {
            object_id: Some(proto_id('D')),
            version: 1,
            content_hash: None,
        }],
        visible_at: Some(market_time(6)),
        curve_family_id: "cn.gov.zero".to_owned(),
    }
}

fn curve_points() -> pb::CurvePointSet {
    pb::CurvePointSet {
        curve_family_id: "cn.gov.zero".to_owned(),
        points: vec![
            pb::CurvePoint {
                curve_node_id: "02Y".to_owned(),
                curve_node_content_hash: Some(hash(b"node-02y")),
                yield_to_maturity: Some(decimal_with_unit("1825", 6, 'R')),
            },
            pb::CurvePoint {
                curve_node_id: "10Y".to_owned(),
                curve_node_content_hash: Some(hash(b"node-10y")),
                yield_to_maturity: Some(decimal_with_unit("215", 5, 'R')),
            },
        ],
    }
}

fn source(kind: &str) -> pb::FactSource {
    pb::FactSource {
        source_id: "fixture-source".to_owned(),
        external_id: format!("{kind}-external-id"),
        source_revision: 1,
        data_source: Some(version_ref('D')),
    }
}

fn change() -> core::ChangeJustification {
    core::ChangeJustification {
        reason: "publish governed immutable market input".to_owned(),
        sources: vec![core::SourceDocumentRef {
            uri: "urn:test:market-fact-source".to_owned(),
            sha256: Some(hash(b"market-fact-source")),
        }],
    }
}

fn unit(suffix: char, code: &str, dimension: &str, scale: u32) -> Unit {
    Unit::new(UnitInput {
        unit_id: id(suffix),
        version: version(),
        owner: domain_owner(),
        code: code.to_owned(),
        dimension: dimension.to_owned(),
        scale,
        precision: 18,
    })
    .unwrap()
}

fn calendar() -> Calendar {
    Calendar::new(CalendarInput {
        calendar_id: id('C'),
        version: version(),
        owner: domain_owner(),
        market: "CN".to_owned(),
        market_timezone: "Asia/Shanghai".to_owned(),
        effective: EffectivePeriod::new(domain_time(-1), domain_time(24)).unwrap(),
        sessions: vec![
            CalendarSession::open(
                NaiveDate::from_ymd_opt(2026, 8, 13).unwrap(),
                NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
                NaiveTime::from_hms_opt(17, 0, 0).unwrap(),
            )
            .unwrap(),
        ],
    })
    .unwrap()
}

fn rule_pack() -> MarketRulePack {
    MarketRulePack::new(MarketRulePackInput {
        rule_pack_id: id('K'),
        version: version(),
        owner: domain_owner(),
        market: "CN".to_owned(),
        rule_type: "valuation-and-curve".to_owned(),
        source: "fixture".to_owned(),
        effective: EffectivePeriod::new(domain_time(-1), domain_time(24)).unwrap(),
        verification_status: VerificationStatus::Verified,
        content_hash: ContentHash::digest(b"rule-pack"),
    })
    .unwrap()
}

fn assert_append_error(response: pb::AppendMarketFactResponse, expected: core::ErrorCode) {
    let Some(pb::append_market_fact_response::Result::Error(error)) = response.result else {
        panic!("append must fail with a business error");
    };
    assert_eq!(error.code, expected as i32);
}

fn decimal_with_unit(coefficient: &str, scale: u32, suffix: char) -> core::DecimalValue {
    core::DecimalValue {
        coefficient: coefficient.to_owned(),
        scale,
        unit: Some(unit_ref(suffix)),
    }
}

fn market_time(hour: i64) -> core::MarketTime {
    core::MarketTime {
        instant: Some(timestamp(hour)),
        market_timezone: "Asia/Shanghai".to_owned(),
        local_trading_date: "2026-08-13".to_owned(),
    }
}

fn domain_time(hour: i64) -> MarketTime {
    let timestamp = timestamp(hour);
    let instant = DateTime::<Utc>::from_timestamp(timestamp.seconds, 0).unwrap();
    let local_date = if hour < 0 {
        NaiveDate::from_ymd_opt(2026, 8, 12).unwrap()
    } else if hour >= 24 {
        NaiveDate::from_ymd_opt(2026, 8, 14).unwrap()
    } else {
        NaiveDate::from_ymd_opt(2026, 8, 13).unwrap()
    };
    MarketTime::new(instant, "Asia/Shanghai", local_date).unwrap()
}

fn timestamp(hour: i64) -> Timestamp {
    Timestamp {
        seconds: 1_786_550_400 + hour * 3_600,
        nanos: 0,
    }
}

fn hash(value: &[u8]) -> core::Sha256 {
    core::Sha256 {
        value: ContentHash::digest(value).as_bytes().to_vec(),
    }
}

fn owner() -> core::OwnerRef {
    core::OwnerRef {
        tenant_id: Some(proto_id('T')),
        owner_id: Some(proto_id('A')),
    }
}

fn domain_owner() -> OwnerRef {
    OwnerRef::new(id('T'), id('A'))
}

fn version_ref(suffix: char) -> core::VersionRef {
    core::VersionRef {
        id: Some(proto_id(suffix)),
        version: 1,
    }
}

fn unit_ref(suffix: char) -> core::UnitRef {
    core::UnitRef {
        unit_id: Some(proto_id(suffix)),
        version: 1,
    }
}

fn proto_id(suffix: char) -> core::Ulid {
    core::Ulid {
        value: id(suffix).as_str().to_owned(),
    }
}

fn version() -> Version {
    Version::new(1).unwrap()
}

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5F0{suffix}")).unwrap()
}

fn not_used() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::StateConflict, false)
}
