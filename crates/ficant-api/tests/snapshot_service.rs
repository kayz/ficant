use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use ficant_api::{
    PlatformApplication, PlatformPort, SessionPolicy, SnapshotGrpcService, SystemClock,
    TrustedIdentity,
};
use ficant_application::ports::{
    AccessScope, AppendDefinitionVersion, ApplicationResult, BeginBlobStage, BlobStore,
    CanonicalImportReplay, CanonicalImportReplayRequest, DataSourceAuthorizationRepository,
    DataSourceRepository, DefinitionIdentity, DefinitionRepository, DefinitionValue,
    GovernedAppendDefinitionVersion, GovernedPublishSnapshot, InstrumentDefinition, IntegrityEvent,
    IntegrityEventSink, IntegrityFailureReason, PageRequest, PublishDataSourceAuthorization,
    PublishSnapshot, RegisterDataSource, RequiredVerifiedBlobRead, SnapshotBlobRole,
    SnapshotRepository, SnapshotValue, SnapshotVerifiedReadMetadata,
    SnapshotVerifiedReadMetadataRepository, StagedBlobRef, VerifiedBlobReader, VerifiedBlobRef,
    VerifyBlobStage,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory, CursorPage};
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::research::v1 as pb;
use ficant_contracts::ficant::research::v1::snapshot_service_server::SnapshotService;
use ficant_data::{
    CANONICAL_QUOTE_SCHEMA_ID, DataResult, InstrumentMapping, InstrumentMappingEntry,
    PointInTimeWindow, QuoteSourceCatalog, RawDecimal, RawQuoteRow, RawQuoteSource,
    RegisteredQuoteSource, canonical_data_source_content_hash, canonical_quote_schema_hash,
};
use ficant_domain::VersionedDefinition;
use ficant_domain::governance::{FoundationChangeOperation, PlatformRole};
use ficant_domain::market::{
    Calendar, CalendarInput, CalendarSession, DataSource, DataSourceAuthorization,
    DataSourceAuthorizationInput, DataSourceAuthorizationState, DataSourceInput, DataSourceKind,
    ImportInterface, Instrument, InstrumentInput, InstrumentKind, PriceSourceType, Unit, UnitInput,
};
use ficant_domain::primitives::{
    ContentHash, EffectivePeriod, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use tonic::Request;

const KEY: &[u8] = b"0123456789abcdef0123456789abcdef";

#[tokio::test]
async fn researcher_imports_exact_authorized_source_and_both_roles_read_verified_payloads() {
    fn assert_service<T: SnapshotService>() {}
    assert_service::<SnapshotGrpcService>();

    let fixture = Fixture::new(DataSourceAuthorizationState::Active, 1);
    let service = fixture.service(
        PlatformRole::Researcher,
        ["data-sources:import", "snapshots:read"],
    );
    let response = service
        .import_canonical_quote_snapshot(Request::new(fixture.import_request()))
        .await
        .unwrap()
        .into_inner();
    let Some(pb::import_canonical_quote_snapshot_response::Result::DataSnapshot(snapshot)) =
        response.result.clone()
    else {
        panic!(
            "exact authorized import must return a DataSnapshot: {:?}",
            response.result
        );
    };
    assert_eq!(snapshot.actor_id, Some(proto_id('A')));
    assert_eq!(snapshot.authorization_ref, Some(version_ref('V', 1)));
    assert_eq!(fixture.raw_reads.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.ports.blob_stages.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.ports.governed_writes.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.ports.legacy_writes.load(Ordering::SeqCst), 0);

    let read = service
        .get_snapshot(Request::new(pb::GetSnapshotRequest {
            snapshot_id: Some(proto_id('S')),
        }))
        .await
        .unwrap()
        .into_inner();
    let Some(pb::get_snapshot_response::Result::DataSnapshot(read)) = read.result else {
        panic!("GetSnapshot must verify and return both durable payloads");
    };
    assert_eq!(read, snapshot);
    assert_eq!(fixture.ports.verified_reads.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.ports.integrity_events.load(Ordering::SeqCst), 0);

    let replay = service
        .import_canonical_quote_snapshot(Request::new(fixture.import_request()))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        replay.result,
        Some(pb::import_canonical_quote_snapshot_response::Result::DataSnapshot(_))
    ));
    assert_eq!(
        fixture.raw_reads.load(Ordering::SeqCst),
        1,
        "a verified idempotent replay must not reopen the external adapter",
    );
    assert_eq!(fixture.ports.blob_stages.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.ports.governed_writes.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.ports.verified_reads.load(Ordering::SeqCst), 4);

    let admin_reader = fixture.service(PlatformRole::PlatformAdmin, ["snapshots:read"]);
    let read = admin_reader
        .get_snapshot(Request::new(pb::GetSnapshotRequest {
            snapshot_id: Some(proto_id('S')),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        read.result,
        Some(pb::get_snapshot_response::Result::DataSnapshot(_))
    ));
}

#[tokio::test]
async fn role_and_authorization_drift_fail_before_adapter_blob_or_repository() {
    for (role, state, version) in [
        (
            PlatformRole::PlatformAdmin,
            DataSourceAuthorizationState::Active,
            1,
        ),
        (
            PlatformRole::Researcher,
            DataSourceAuthorizationState::Revoked,
            2,
        ),
    ] {
        let fixture = Fixture::new(state, version);
        let service = fixture.service(
            role,
            ["data-sources:import", "snapshots:read", "snapshots:write"],
        );
        let response = service
            .import_canonical_quote_snapshot(Request::new(fixture.import_request()))
            .await
            .unwrap()
            .into_inner();
        let Some(pb::import_canonical_quote_snapshot_response::Result::Error(error)) =
            response.result
        else {
            panic!("role/revocation drift must fail closed");
        };
        assert_eq!(error.code, core::ErrorCode::Forbidden as i32);
        assert_eq!(fixture.raw_reads.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.ports.blob_stages.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.ports.governed_writes.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.ports.legacy_writes.load(Ordering::SeqCst), 0);
    }

    let fixture = Fixture::new(DataSourceAuthorizationState::Active, 1);
    let service = fixture.service(PlatformRole::Researcher, ["data-sources:import"]);
    let mut request = fixture.import_request();
    request.mapping.as_mut().unwrap().content_hash = Some(hash(b"caller mapping drift"));
    let response = service
        .import_canonical_quote_snapshot(Request::new(request))
        .await
        .unwrap()
        .into_inner();
    assert_error(response.result, core::ErrorCode::ValidationFailed);
    assert_eq!(fixture.ports.authorization_reads.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.raw_reads.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.ports.blob_stages.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.ports.governed_writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn admin_publishes_server_hashed_universe_while_researcher_direct_write_is_closed() {
    let fixture = Fixture::new(DataSourceAuthorizationState::Active, 1);
    let researcher = fixture.service(
        PlatformRole::Researcher,
        ["snapshots:write", "snapshots:read"],
    );
    let rejected = researcher
        .publish_universe_snapshot(Request::new(Fixture::universe_request('R')))
        .await
        .unwrap()
        .into_inner();
    assert_error(rejected.result, core::ErrorCode::Forbidden);
    assert_eq!(fixture.ports.blob_stages.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.ports.governed_writes.load(Ordering::SeqCst), 0);

    let admin = fixture.service(
        PlatformRole::PlatformAdmin,
        ["snapshots:write", "snapshots:read"],
    );
    let response = admin
        .publish_universe_snapshot(Request::new(Fixture::universe_request('U')))
        .await
        .unwrap()
        .into_inner();
    let Some(pb::publish_universe_snapshot_response::Result::UniverseSnapshot(snapshot)) =
        response.result
    else {
        panic!("Platform Admin must publish one governed UniverseSnapshot");
    };
    assert_eq!(snapshot.actor_id, Some(proto_id('A')));
    assert_eq!(snapshot.instrument_versions, vec![version_ref('I', 1)]);
    assert_eq!(snapshot.content_hash.as_ref().unwrap().value.len(), 32);
    assert_eq!(fixture.ports.blob_stages.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.ports.governed_writes.load(Ordering::SeqCst), 1);

    let read = admin
        .get_snapshot(Request::new(pb::GetSnapshotRequest {
            snapshot_id: Some(proto_id('U')),
        }))
        .await
        .unwrap()
        .into_inner();
    let Some(pb::get_snapshot_response::Result::UniverseSnapshot(read)) = read.result else {
        panic!("GetSnapshot must validate the Universe members manifest");
    };
    assert_eq!(read, snapshot);
}

struct Fixture {
    ports: Arc<Ports>,
    catalog: Arc<QuoteSourceCatalog>,
    raw_reads: Arc<AtomicUsize>,
    mapping: InstrumentMapping,
    calendar: Calendar,
    unit: Unit,
    authorization_version: u64,
}

impl Fixture {
    fn new(state: DataSourceAuthorizationState, authorization_version: u64) -> Self {
        let owner = owner();
        let calendar = calendar();
        let unit = unit();
        let instrument = Instrument::new(InstrumentInput {
            instrument_id: id('I'),
            version: Version::new(1).unwrap(),
            owner: owner.clone(),
            kind: InstrumentKind::Other,
            market: "CGB".to_owned(),
            symbol: "260011.IB".to_owned(),
            currency: UnitRef::new(id('N'), Version::new(1).unwrap()),
            calendar: VersionRef::new(id('C'), Version::new(1).unwrap()),
        })
        .unwrap();
        let source = source();
        let mapping = InstrumentMapping::new(
            id('M'),
            owner.clone(),
            VersionRef::new(id('D'), Version::new(1).unwrap()),
            vec![
                InstrumentMappingEntry::new(
                    "260011.IB",
                    effective_period(),
                    VersionRef::new(id('I'), Version::new(1).unwrap()),
                )
                .unwrap(),
            ],
        )
        .unwrap();
        let authorization = authorization(
            &source,
            &mapping,
            state,
            Version::new(authorization_version).unwrap(),
        );
        let ports = Arc::new(Ports::new(
            source,
            authorization,
            vec![
                DefinitionValue::Calendar(calendar.clone()),
                DefinitionValue::Unit(unit.clone()),
                DefinitionValue::Instrument(InstrumentDefinition::new(instrument, None).unwrap()),
            ],
        ));
        let raw_reads = Arc::new(AtomicUsize::new(0));
        let catalog = Arc::new(
            QuoteSourceCatalog::new(vec![
                RegisteredQuoteSource::new(
                    DataSourceKind::FileNdjson,
                    "admin-file-binding",
                    Arc::new(RawSource {
                        reads: Arc::clone(&raw_reads),
                    }),
                )
                .unwrap(),
            ])
            .unwrap(),
        );
        Self {
            ports,
            catalog,
            raw_reads,
            mapping,
            calendar,
            unit,
            authorization_version,
        }
    }

    fn service<const N: usize>(
        &self,
        role: PlatformRole,
        scopes: [&str; N],
    ) -> SnapshotGrpcService {
        let identity = TrustedIdentity::implicit(
            "snapshot-test",
            id('A'),
            id('T'),
            vec![id('P')],
            role,
            scopes,
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
        SnapshotGrpcService::new(
            application,
            self.ports.clone(),
            self.ports.clone(),
            self.ports.clone(),
            self.catalog.clone(),
            self.ports.clone(),
            self.ports.clone(),
            self.ports.clone(),
            self.ports.clone(),
            self.ports.clone(),
            KEY,
        )
        .unwrap()
    }

    fn import_request(&self) -> pb::ImportCanonicalQuoteSnapshotRequest {
        pb::ImportCanonicalQuoteSnapshotRequest {
            idempotency_key: "snapshot/import-1".to_owned(),
            target_snapshot_id: Some(proto_id('S')),
            authorization_ref: Some(version_ref('V', self.authorization_version)),
            mapping: Some(mapping(&self.mapping)),
            calendar: Some(calendar_proto(&self.calendar)),
            unit: Some(unit_proto(&self.unit)),
            as_of: Some(market_time_proto("2026-08-13T02:00:00Z")),
            visible_at: Some(market_time_proto("2026-08-13T02:05:00Z")),
            import_reason: "approved daily CGB import".to_owned(),
        }
    }

    fn universe_request(snapshot_suffix: char) -> pb::PublishUniverseSnapshotRequest {
        pb::PublishUniverseSnapshotRequest {
            idempotency_key: format!("snapshot/universe-{snapshot_suffix}"),
            universe_snapshot_id: Some(proto_id(snapshot_suffix)),
            owner: Some(owner_proto()),
            instrument_versions: vec![version_ref('I', 1)],
            filter_digest: Some(hash(b"all governed instruments")),
            lineage: vec![core::LineageRef {
                object_id: Some(proto_id('I')),
                version: 1,
                content_hash: None,
            }],
            change: Some(change()),
        }
    }
}

struct Ports {
    source: DataSource,
    authorization: DataSourceAuthorization,
    definitions: Vec<DefinitionValue>,
    authorization_reads: AtomicUsize,
    blob_stages: AtomicUsize,
    governed_writes: AtomicUsize,
    legacy_writes: AtomicUsize,
    verified_reads: AtomicUsize,
    integrity_events: AtomicUsize,
    staged: Mutex<BTreeMap<String, (OwnerRef, Vec<u8>)>>,
    blobs: Mutex<BTreeMap<Vec<u8>, Vec<u8>>>,
    metadata: Mutex<BTreeMap<String, SnapshotVerifiedReadMetadata>>,
    snapshots: Mutex<BTreeMap<String, SnapshotValue>>,
    replay: Mutex<Option<CanonicalImportReplay>>,
}

impl Ports {
    fn new(
        source: DataSource,
        authorization: DataSourceAuthorization,
        definitions: Vec<DefinitionValue>,
    ) -> Self {
        Self {
            source,
            authorization,
            definitions,
            authorization_reads: AtomicUsize::new(0),
            blob_stages: AtomicUsize::new(0),
            governed_writes: AtomicUsize::new(0),
            legacy_writes: AtomicUsize::new(0),
            verified_reads: AtomicUsize::new(0),
            integrity_events: AtomicUsize::new(0),
            staged: Mutex::new(BTreeMap::new()),
            blobs: Mutex::new(BTreeMap::new()),
            metadata: Mutex::new(BTreeMap::new()),
            snapshots: Mutex::new(BTreeMap::new()),
            replay: Mutex::new(None),
        }
    }
}

#[async_trait]
impl DataSourceRepository for Ports {
    async fn register(&self, _: RegisterDataSource) -> ApplicationResult<DataSource> {
        Err(not_used())
    }

    async fn get_exact(
        &self,
        scope: &AccessScope,
        reference: VersionRef,
    ) -> ApplicationResult<Option<DataSource>> {
        scope.authorize(self.source.owner())?;
        Ok((reference.id() == self.source.id()
            && reference.version().get() == self.source.version())
        .then(|| self.source.clone()))
    }
}

#[async_trait]
impl DataSourceAuthorizationRepository for Ports {
    async fn publish_authorization(
        &self,
        _: PublishDataSourceAuthorization,
    ) -> ApplicationResult<DataSourceAuthorization> {
        Err(not_used())
    }

    async fn get_authorization_exact(
        &self,
        _: &AccessScope,
        reference: VersionRef,
    ) -> ApplicationResult<Option<DataSourceAuthorization>> {
        self.authorization_reads.fetch_add(1, Ordering::SeqCst);
        Ok((reference == self.authorization.version_ref()).then(|| self.authorization.clone()))
    }

    async fn list_authorizations_for_source(
        &self,
        _: &AccessScope,
        _: &OwnerRef,
        _: &VersionRef,
        _: Option<ImportInterface>,
        _: PageRequest,
    ) -> ApplicationResult<CursorPage<DataSourceAuthorization>> {
        Ok(CursorPage::new(vec![self.authorization.clone()], None))
    }
}

#[async_trait]
impl DefinitionRepository for Ports {
    async fn append_complete(
        &self,
        _: GovernedAppendDefinitionVersion,
    ) -> ApplicationResult<DefinitionValue> {
        Err(not_used())
    }

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
            .definitions
            .iter()
            .find(|value| {
                value.identity() == definition_id.as_str() && value.version() == version.get()
            })
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

    async fn list_versions(
        &self,
        _: &AccessScope,
        _: Ulid,
        _: PageRequest,
    ) -> ApplicationResult<CursorPage<DefinitionValue>> {
        Err(not_used())
    }
}

#[async_trait]
impl BlobStore for Ports {
    async fn begin_stage(&self, command: BeginBlobStage) -> ApplicationResult<StagedBlobRef> {
        let sequence = self.blob_stages.fetch_add(1, Ordering::SeqCst);
        let suffix =
            char::from(b'0' + u8::try_from(sequence).expect("fixture stage count is small"));
        let staged = StagedBlobRef::new(id(suffix), command.owner().clone());
        self.staged.lock().unwrap().insert(
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
        self.staged
            .lock()
            .unwrap()
            .get_mut(staged.id().as_str())
            .ok_or_else(not_found)?
            .1
            .extend(chunk);
        Ok(())
    }

    async fn verify_and_promote(
        &self,
        command: VerifyBlobStage,
    ) -> ApplicationResult<VerifiedBlobRef> {
        let bytes = self
            .staged
            .lock()
            .unwrap()
            .remove(command.staged().id().as_str())
            .ok_or_else(not_found)?
            .1;
        if ContentHash::digest(&bytes) != *command.expected_hash()
            || u64::try_from(bytes.len()).unwrap() != command.expected_size()
        {
            return Err(ApplicationError::new(
                ApplicationErrorCategory::HashMismatch,
                false,
            ));
        }
        self.blobs
            .lock()
            .unwrap()
            .insert(command.expected_hash().as_bytes().to_vec(), bytes);
        VerifiedBlobRef::new(command.expected_hash().clone(), command.expected_size())
    }

    async fn discard_stage(
        &self,
        _: &AccessScope,
        staged: &StagedBlobRef,
    ) -> ApplicationResult<()> {
        self.staged.lock().unwrap().remove(staged.id().as_str());
        Ok(())
    }
}

#[async_trait]
impl SnapshotRepository for Ports {
    async fn probe_canonical_import_replay(
        &self,
        request: &CanonicalImportReplayRequest,
    ) -> ApplicationResult<Option<CanonicalImportReplay>> {
        let replay = self.replay.lock().unwrap().clone();
        match replay {
            None => Ok(None),
            Some(value) => CanonicalImportReplay::verified(
                request,
                value.snapshot().clone(),
                value.actor_id().clone(),
                value.authorization().clone(),
                value.authorization_hash().clone(),
            )
            .map(Some),
        }
    }

    async fn publish_governed(
        &self,
        command: GovernedPublishSnapshot,
    ) -> ApplicationResult<SnapshotValue> {
        let change = command.change_record()?;
        assert!(matches!(
            change.operation(),
            FoundationChangeOperation::ImportCanonicalQuoteSnapshot
                | FoundationChangeOperation::PublishUniverseSnapshot
        ));
        self.governed_writes.fetch_add(1, Ordering::SeqCst);
        let value = command.command().snapshot().clone();
        let proof = command.command().proof();
        let metadata = match &value {
            SnapshotValue::Data(snapshot) => SnapshotVerifiedReadMetadata::data(
                snapshot.clone(),
                proof
                    .get(SnapshotBlobRole::DataParquet)
                    .unwrap()
                    .verified_blob()
                    .size(),
                proof
                    .get(SnapshotBlobRole::DataManifest)
                    .unwrap()
                    .verified_blob()
                    .size(),
            )?,
            SnapshotValue::Universe(snapshot) => SnapshotVerifiedReadMetadata::universe(
                snapshot.clone(),
                proof
                    .get(SnapshotBlobRole::UniverseMembersManifest)
                    .unwrap()
                    .verified_blob()
                    .size(),
            )?,
            SnapshotValue::DataHealthThresholdProfile(_) | SnapshotValue::Position(_) => {
                return Err(not_used());
            }
        };
        self.metadata
            .lock()
            .unwrap()
            .insert(value.id().as_str().to_owned(), metadata);
        self.snapshots
            .lock()
            .unwrap()
            .insert(value.id().as_str().to_owned(), value.clone());
        if let SnapshotValue::Data(snapshot) = &value {
            let request = command
                .replay_request()
                .expect("governed DataSnapshot carries exact replay evidence");
            *self.replay.lock().unwrap() = Some(CanonicalImportReplay::verified(
                request,
                snapshot.clone(),
                change.actor_id().clone(),
                change
                    .authorization_ref()
                    .expect("import change carries authorization")
                    .clone(),
                request.authorization_hash().clone(),
            )?);
        }
        Ok(value)
    }

    async fn publish_verified_manifest(
        &self,
        _: PublishSnapshot,
    ) -> ApplicationResult<SnapshotValue> {
        self.legacy_writes.fetch_add(1, Ordering::SeqCst);
        Err(not_used())
    }

    async fn get_by_id(
        &self,
        _: &AccessScope,
        snapshot_id: Ulid,
    ) -> ApplicationResult<Option<SnapshotValue>> {
        Ok(self
            .snapshots
            .lock()
            .unwrap()
            .get(snapshot_id.as_str())
            .cloned())
    }
}

#[async_trait]
impl SnapshotVerifiedReadMetadataRepository for Ports {
    async fn get_verified_read_metadata(
        &self,
        _: &AccessScope,
        snapshot_id: Ulid,
    ) -> ApplicationResult<Option<SnapshotVerifiedReadMetadata>> {
        Ok(self
            .metadata
            .lock()
            .unwrap()
            .get(snapshot_id.as_str())
            .cloned())
    }
}

#[async_trait]
impl VerifiedBlobReader for Ports {
    async fn read_required(
        &self,
        request: &RequiredVerifiedBlobRead,
        sink: &dyn IntegrityEventSink,
    ) -> ApplicationResult<ficant_application::ports::VerifiedBlobPayload> {
        self.verified_reads.fetch_add(1, Ordering::SeqCst);
        let bytes = self
            .blobs
            .lock()
            .unwrap()
            .get(request.expected_hash().as_bytes().as_slice())
            .cloned();
        match bytes {
            Some(bytes) => request.verify_bytes(sink, bytes).await,
            None => Err(request
                .fail_integrity(sink, IntegrityFailureReason::Missing)
                .await),
        }
    }
}

#[async_trait]
impl IntegrityEventSink for Ports {
    async fn emit(&self, _: IntegrityEvent) -> ApplicationResult<()> {
        self.integrity_events.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct RawSource {
    reads: Arc<AtomicUsize>,
}

#[async_trait]
impl RawQuoteSource for RawSource {
    async fn read(&self, _: &DataSource, _: &PointInTimeWindow) -> DataResult<Vec<RawQuoteRow>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(vec![RawQuoteRow::new(
            "record-1",
            "260011.IB",
            "2026-08-13T02:00:00Z",
            "2026-08-13T02:00:01Z",
            Some(RawDecimal::new("1010000", 4)),
            Some(RawDecimal::new("1010100", 4)),
        )])
    }
}

fn source() -> DataSource {
    DataSource::new(DataSourceInput {
        data_source_id: id('D'),
        version: Version::new(1).unwrap(),
        owner: owner(),
        kind: DataSourceKind::FileNdjson,
        name: "CGB quotes".to_owned(),
        connection_binding: "admin-file-binding".to_owned(),
        dataset: "quotes".to_owned(),
        canonical_schema_id: CANONICAL_QUOTE_SCHEMA_ID.to_owned(),
        canonical_schema_hash: canonical_quote_schema_hash(),
    })
    .unwrap()
    .with_price_source_type(PriceSourceType::ActiveQuote)
    .unwrap()
}

fn authorization(
    source: &DataSource,
    mapping: &InstrumentMapping,
    state: DataSourceAuthorizationState,
    version: Version,
) -> DataSourceAuthorization {
    DataSourceAuthorization::new(DataSourceAuthorizationInput {
        authorization_id: id('V'),
        version,
        owner: owner(),
        data_source: VersionRef::new(id('D'), Version::new(1).unwrap()),
        data_source_hash: canonical_data_source_content_hash(source),
        import_interface: ImportInterface::CanonicalQuoteSnapshot,
        canonical_schema_id: CANONICAL_QUOTE_SCHEMA_ID.to_owned(),
        canonical_schema_hash: canonical_quote_schema_hash(),
        effective: effective_period(),
        state,
        supersedes: (version.get() > 1)
            .then(|| VersionRef::new(id('V'), Version::new(version.get() - 1).unwrap())),
        mapping_id: mapping.id().clone(),
        mapping_hash: mapping.content_hash().clone(),
    })
    .unwrap()
}

fn calendar() -> Calendar {
    Calendar::new(CalendarInput {
        calendar_id: id('C'),
        version: Version::new(1).unwrap(),
        owner: owner(),
        market: "CGB".to_owned(),
        market_timezone: "Asia/Shanghai".to_owned(),
        effective: effective_period(),
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

fn unit() -> Unit {
    Unit::new(UnitInput {
        unit_id: id('N'),
        version: Version::new(1).unwrap(),
        owner: owner(),
        code: "CNY_CLEAN_PRICE".to_owned(),
        dimension: "price".to_owned(),
        scale: 4,
        precision: 12,
    })
    .unwrap()
}

fn mapping(value: &InstrumentMapping) -> ficant_contracts::ficant::market::v1::InstrumentMapping {
    ficant_contracts::ficant::market::v1::InstrumentMapping {
        mapping_id: Some(proto_id('M')),
        owner: Some(owner_proto()),
        source: Some(version_ref('D', 1)),
        entries: value
            .entries()
            .iter()
            .map(
                |entry| ficant_contracts::ficant::market::v1::InstrumentMappingEntry {
                    source_instrument_key: entry.source_instrument_key().to_owned(),
                    effective_from: Some(market_time(entry.effective().from())),
                    effective_to: Some(market_time(entry.effective().to())),
                    instrument: Some(version_ref('I', 1)),
                },
            )
            .collect(),
        content_hash: Some(core::Sha256 {
            value: value.content_hash().as_bytes().to_vec(),
        }),
    }
}

fn calendar_proto(value: &Calendar) -> ficant_contracts::ficant::market::v1::Calendar {
    ficant_contracts::ficant::market::v1::Calendar {
        calendar_id: Some(proto_id('C')),
        version: 1,
        owner: Some(owner_proto()),
        market: "CGB".to_owned(),
        market_timezone: "Asia/Shanghai".to_owned(),
        effective_from: Some(market_time(value.effective().from())),
        effective_to: Some(market_time(value.effective().to())),
        sessions: vec![ficant_contracts::ficant::market::v1::CalendarSession {
            local_date: "2026-08-13".to_owned(),
            open_local_time: "09:00:00".to_owned(),
            close_local_time: "17:00:00".to_owned(),
            closed: false,
        }],
    }
}

fn unit_proto(_: &Unit) -> ficant_contracts::ficant::market::v1::Unit {
    ficant_contracts::ficant::market::v1::Unit {
        unit_id: Some(proto_id('N')),
        version: 1,
        owner: Some(owner_proto()),
        code: "CNY_CLEAN_PRICE".to_owned(),
        dimension: "price".to_owned(),
        scale: 4,
        precision: 12,
    }
}

fn change() -> core::ChangeJustification {
    core::ChangeJustification {
        reason: "approved universe publication".to_owned(),
        sources: vec![core::SourceDocumentRef {
            uri: "fixture://snapshot/universe".to_owned(),
            sha256: Some(hash(b"universe fixture")),
        }],
    }
}

fn effective_period() -> EffectivePeriod {
    EffectivePeriod::new(time("2026-01-01T00:00:00Z"), time("2027-01-01T00:00:00Z")).unwrap()
}

fn time(value: &str) -> MarketTime {
    let instant = value.parse::<DateTime<Utc>>().unwrap();
    let timezone = "Asia/Shanghai".parse::<chrono_tz::Tz>().unwrap();
    MarketTime::new(
        instant,
        "Asia/Shanghai",
        instant.with_timezone(&timezone).date_naive(),
    )
    .unwrap()
}

fn market_time(value: &MarketTime) -> core::MarketTime {
    core::MarketTime {
        instant: Some(prost_types::Timestamp {
            seconds: value.instant().timestamp(),
            nanos: value.instant().timestamp_subsec_nanos().cast_signed(),
        }),
        market_timezone: value.market_timezone().to_owned(),
        local_trading_date: value.local_trading_date().to_string(),
    }
}

fn market_time_proto(value: &str) -> core::MarketTime {
    market_time(&time(value))
}

fn owner() -> OwnerRef {
    OwnerRef::new(id('T'), id('P'))
}

fn owner_proto() -> core::OwnerRef {
    core::OwnerRef {
        tenant_id: Some(proto_id('T')),
        owner_id: Some(proto_id('P')),
    }
}

fn version_ref(suffix: char, version: u64) -> core::VersionRef {
    core::VersionRef {
        id: Some(proto_id(suffix)),
        version,
    }
}

fn proto_id(suffix: char) -> core::Ulid {
    core::Ulid {
        value: id(suffix).as_str().to_owned(),
    }
}

fn id(suffix: char) -> Ulid {
    let suffix = match suffix {
        'I' => 'J',
        'U' => 'W',
        value => value,
    };
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}

fn hash(value: &[u8]) -> core::Sha256 {
    core::Sha256 {
        value: ContentHash::digest(value).as_bytes().to_vec(),
    }
}

fn assert_error<T>(result: Option<T>, expected: core::ErrorCode)
where
    T: IntoSnapshotError,
{
    let error = result.and_then(IntoSnapshotError::into_error).unwrap();
    assert_eq!(error.code, expected as i32);
}

trait IntoSnapshotError {
    fn into_error(self) -> Option<core::ErrorDetail>;
}

impl IntoSnapshotError for pb::import_canonical_quote_snapshot_response::Result {
    fn into_error(self) -> Option<core::ErrorDetail> {
        match self {
            Self::Error(error) => Some(error),
            Self::DataSnapshot(_) => None,
        }
    }
}

impl IntoSnapshotError for pb::publish_universe_snapshot_response::Result {
    fn into_error(self) -> Option<core::ErrorDetail> {
        match self {
            Self::Error(error) => Some(error),
            Self::UniverseSnapshot(_) => None,
        }
    }
}

fn not_found() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::NotFound, false)
}

fn not_used() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::StateConflict, false)
}
