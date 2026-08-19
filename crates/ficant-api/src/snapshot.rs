use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use ficant_application::ports::{
    BlobStore, CanonicalImportManifestEvidence, CanonicalImportReplayRequest,
    DATA_SOURCE_IMPORT_SCOPE, DataSourceAuthorizationRepository, DataSourceRepository,
    DefinitionRepository, DefinitionValue, FoundationChangeContext, IdempotencyKey,
    IntegrityEventSink, SNAPSHOT_WRITE_SCOPE, SafeTraceContext, SnapshotRepository,
    SnapshotVerifiedReadMetadataRepository, VerifiedBlobReader,
};
use ficant_application::{
    ApplicationError, ApplicationErrorCategory, DataSnapshotPayloads, GovernedInputUseCase,
    PublishDataSnapshot, PublishUniverseSnapshot, UniverseSnapshotIntent, VerifiedSnapshotRead,
    VerifiedSnapshotReader, map_domain_error, verify_universe_snapshot_manifest,
};
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::research::v1 as pb;
use ficant_contracts::ficant::research::v1::snapshot_service_server::SnapshotService;
use ficant_data::{
    CanonicalSnapshotCodec, DataError, GovernedCanonicalImportRequest,
    GovernedCanonicalQuoteImporter, InstrumentMapping, PointInTimeWindow, QuoteSourceCatalog,
};
use ficant_domain::governance::{
    ChangeJustification, FoundationResourceKind, FoundationResourceRef, PlatformRole,
    deterministic_change_record_id,
};
use ficant_domain::market::{Calendar, ImportInterface, Unit};
use ficant_domain::primitives::{ContentHash, LineageRef, OwnerRef, Ulid, Version, VersionRef};
use ficant_domain::research::{DataSnapshot, UniverseSnapshot};
use ficant_domain::{ContentAddressed, Lineaged, VersionedDefinition};
use prost::Message;
use tonic::{Request, Response, Status};

use ficant_application::ports::DefinitionUseCase;

use crate::core_error::CoreBusinessErrorMapper;
use crate::data_source_registry::parse_mapping;
use crate::grpc_web::request_credential;
use crate::market_definition::{
    hash, market_time, owner, parse_calendar, parse_change, parse_hash, parse_market_time,
    parse_owner, parse_ulid, parse_unit_definition, parse_version_ref, server_market_time, ulid,
    version_ref,
};
use crate::registry::PlatformPort;

const SNAPSHOT_READ_SCOPE: &str = "snapshots:read";

/// Authenticated transport for canonical imports and immutable Snapshot publication/readback.
#[derive(Clone)]
pub struct SnapshotGrpcService {
    identity: Arc<dyn PlatformPort>,
    authorizations: Arc<dyn DataSourceAuthorizationRepository>,
    data_sources: Arc<dyn DataSourceRepository>,
    definitions: Arc<dyn DefinitionRepository>,
    catalog: Arc<QuoteSourceCatalog>,
    snapshots: Arc<dyn SnapshotRepository>,
    blob_store: Arc<dyn BlobStore>,
    verified_snapshots: Arc<dyn SnapshotVerifiedReadMetadataRepository>,
    blob_reader: Arc<dyn VerifiedBlobReader>,
    integrity_events: Arc<dyn IntegrityEventSink>,
    errors: CoreBusinessErrorMapper,
}

impl SnapshotGrpcService {
    /// Composes the governed Snapshot boundary. Adapter registrations remain server-owned.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when the trace-key contract is invalid.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        identity: Arc<dyn PlatformPort>,
        authorizations: Arc<dyn DataSourceAuthorizationRepository>,
        data_sources: Arc<dyn DataSourceRepository>,
        definitions: Arc<dyn DefinitionRepository>,
        catalog: Arc<QuoteSourceCatalog>,
        snapshots: Arc<dyn SnapshotRepository>,
        blob_store: Arc<dyn BlobStore>,
        verified_snapshots: Arc<dyn SnapshotVerifiedReadMetadataRepository>,
        blob_reader: Arc<dyn VerifiedBlobReader>,
        integrity_events: Arc<dyn IntegrityEventSink>,
        trace_key: &[u8],
    ) -> Result<Self, &'static str> {
        Ok(Self {
            identity,
            authorizations,
            data_sources,
            definitions,
            catalog,
            snapshots,
            blob_store,
            verified_snapshots,
            blob_reader,
            integrity_events,
            errors: CoreBusinessErrorMapper::new(trace_key)?,
        })
    }

    /// Composes the governed Snapshot boundary with the fixed production adapter catalog.
    ///
    /// # Errors
    ///
    /// Returns a configuration error before serving when any trusted adapter binding is invalid.
    #[allow(clippy::too_many_arguments)]
    pub fn new_production(
        identity: Arc<dyn PlatformPort>,
        authorizations: Arc<dyn DataSourceAuthorizationRepository>,
        data_sources: Arc<dyn DataSourceRepository>,
        definitions: Arc<dyn DefinitionRepository>,
        file_connection_binding: String,
        file_root: PathBuf,
        postgres_connection_binding: String,
        postgres_database_url: &str,
        snapshots: Arc<dyn SnapshotRepository>,
        blob_store: Arc<dyn BlobStore>,
        verified_snapshots: Arc<dyn SnapshotVerifiedReadMetadataRepository>,
        blob_reader: Arc<dyn VerifiedBlobReader>,
        integrity_events: Arc<dyn IntegrityEventSink>,
        trace_key: &[u8],
    ) -> Result<Self, &'static str> {
        let catalog = QuoteSourceCatalog::production(
            file_connection_binding,
            file_root,
            postgres_connection_binding,
            postgres_database_url,
        )
        .map(Arc::new)
        .map_err(|_| "trusted governed input catalog is invalid")?;
        Self::new(
            identity,
            authorizations,
            data_sources,
            definitions,
            catalog,
            snapshots,
            blob_store,
            verified_snapshots,
            blob_reader,
            integrity_events,
            trace_key,
        )
    }

    fn principal(
        &self,
        request: &Request<impl Sized>,
        required_scope: &str,
    ) -> Result<ficant_application::AuthorizedPrincipal, ApplicationError> {
        let credential = request_credential(request.metadata());
        let session = self
            .identity
            .current_session(&credential)
            .map_err(|_| forbidden())?;
        let principal = session.authorized_principal()?;
        principal
            .has_scope(required_scope)
            .then_some(principal)
            .ok_or_else(forbidden)
    }

    fn error(&self, operation: &str, error: &ApplicationError) -> core::ErrorDetail {
        self.errors.map(operation, "snapshot-application", error)
    }
}

#[tonic::async_trait]
impl SnapshotService for SnapshotGrpcService {
    async fn import_canonical_quote_snapshot(
        &self,
        request: Request<pb::ImportCanonicalQuoteSnapshotRequest>,
    ) -> Result<Response<pb::ImportCanonicalQuoteSnapshotResponse>, Status> {
        const OPERATION: &str = "snapshots.import-canonical-quotes";
        let trace = trace_context(request.get_ref());
        let result = match self.principal(&request, DATA_SOURCE_IMPORT_SCOPE) {
            Err(error) => Err(error),
            Ok(principal) => {
                let value = request.get_ref();
                let parsed = (|| {
                    let snapshot_id = parse_ulid(value.target_snapshot_id.as_ref())?;
                    let authorization_ref = parse_version_ref(value.authorization_ref.as_ref())?;
                    let mapping = parse_mapping(value.mapping.as_ref())?;
                    let calendar = parse_calendar(value.calendar.as_ref().ok_or_else(invalid)?)?;
                    let unit = parse_unit_definition(value.unit.as_ref().ok_or_else(invalid)?)?;
                    let as_of = parse_market_time(value.as_of.as_ref())?;
                    let visible_at = parse_market_time(value.visible_at.as_ref())?;
                    let window =
                        PointInTimeWindow::new(as_of, visible_at).map_err(map_data_error)?;
                    let key = IdempotencyKey::new(value.idempotency_key.clone())?;
                    Ok((
                        snapshot_id,
                        authorization_ref,
                        mapping,
                        calendar,
                        unit,
                        window,
                        key,
                        value.import_reason.clone(),
                    ))
                })();
                match parsed {
                    Err(error) => Err(error),
                    Ok((
                        snapshot_id,
                        authorization_ref,
                        mapping,
                        calendar,
                        unit,
                        window,
                        key,
                        import_reason,
                    )) => {
                        self.import_canonical(
                            principal,
                            snapshot_id,
                            authorization_ref,
                            mapping,
                            calendar,
                            unit,
                            window,
                            key,
                            import_reason,
                            trace,
                        )
                        .await
                    }
                }
            }
        };
        Ok(Response::new(pb::ImportCanonicalQuoteSnapshotResponse {
            result: Some(match result {
                Ok((snapshot, authorization, actor)) => {
                    pb::import_canonical_quote_snapshot_response::Result::DataSnapshot(
                        data_snapshot(&snapshot, &authorization, &actor),
                    )
                }
                Err(error) => pb::import_canonical_quote_snapshot_response::Result::Error(
                    self.error(OPERATION, &error),
                ),
            }),
        }))
    }

    async fn publish_universe_snapshot(
        &self,
        request: Request<pb::PublishUniverseSnapshotRequest>,
    ) -> Result<Response<pb::PublishUniverseSnapshotResponse>, Status> {
        const OPERATION: &str = "snapshots.publish-universe";
        let result = match self.principal(&request, SNAPSHOT_WRITE_SCOPE) {
            Err(error) => Err(error),
            Ok(principal) if principal.require_role(PlatformRole::PlatformAdmin).is_ok() => {
                let value = request.get_ref();
                let parsed = (|| {
                    let snapshot_id = parse_ulid(value.universe_snapshot_id.as_ref())?;
                    let owner = parse_owner(value.owner.as_ref())?;
                    principal.authorize_mutation(
                        PlatformRole::PlatformAdmin,
                        SNAPSHOT_WRITE_SCOPE,
                        &owner,
                    )?;
                    let members = value
                        .instrument_versions
                        .iter()
                        .map(|member| parse_version_ref(Some(member)))
                        .collect::<Result<Vec<_>, _>>()?;
                    let lineage = value
                        .lineage
                        .iter()
                        .map(parse_lineage)
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok((
                        snapshot_id,
                        owner,
                        members,
                        parse_hash(value.filter_digest.as_ref())?,
                        lineage,
                        IdempotencyKey::new(value.idempotency_key.clone())?,
                        parse_change(value.change.as_ref())?,
                    ))
                })();
                match parsed {
                    Err(error) => Err(error),
                    Ok((snapshot_id, owner, members, filter, lineage, key, change)) => {
                        self.publish_universe(
                            principal,
                            snapshot_id,
                            owner,
                            members,
                            filter,
                            lineage,
                            key,
                            change,
                        )
                        .await
                    }
                }
            }
            Ok(_) => Err(forbidden()),
        };
        Ok(Response::new(pb::PublishUniverseSnapshotResponse {
            result: Some(match result {
                Ok((snapshot, actor)) => {
                    pb::publish_universe_snapshot_response::Result::UniverseSnapshot(
                        universe_snapshot(&snapshot, &actor),
                    )
                }
                Err(error) => pb::publish_universe_snapshot_response::Result::Error(
                    self.error(OPERATION, &error),
                ),
            }),
        }))
    }

    async fn get_snapshot(
        &self,
        request: Request<pb::GetSnapshotRequest>,
    ) -> Result<Response<pb::GetSnapshotResponse>, Status> {
        const OPERATION: &str = "snapshots.get";
        let trace = trace_context(request.get_ref());
        let result = match self.principal(&request, SNAPSHOT_READ_SCOPE) {
            Err(error) => Err(error),
            Ok(principal) => match parse_ulid(request.get_ref().snapshot_id.as_ref()) {
                Err(error) => Err(error),
                Ok(snapshot_id) => VerifiedSnapshotReader::new(
                    self.verified_snapshots.as_ref(),
                    self.blob_reader.as_ref(),
                    self.integrity_events.as_ref(),
                )
                .read(principal.access_scope(), snapshot_id, trace)
                .await
                .and_then(verified_snapshot),
            },
        };
        Ok(Response::new(pb::GetSnapshotResponse {
            result: Some(match result {
                Ok(VerifiedSnapshotResponse::Data {
                    snapshot,
                    authorization,
                    authorization_hash: _,
                    actor,
                }) => pb::get_snapshot_response::Result::DataSnapshot(data_snapshot(
                    &snapshot,
                    &authorization,
                    &actor,
                )),
                Ok(VerifiedSnapshotResponse::Universe { snapshot, actor }) => {
                    pb::get_snapshot_response::Result::UniverseSnapshot(universe_snapshot(
                        &snapshot, &actor,
                    ))
                }
                Err(error) => {
                    pb::get_snapshot_response::Result::Error(self.error(OPERATION, &error))
                }
            }),
        }))
    }
}

impl SnapshotGrpcService {
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn import_canonical(
        &self,
        principal: ficant_application::AuthorizedPrincipal,
        snapshot_id: Ulid,
        authorization_ref: VersionRef,
        mapping: InstrumentMapping,
        calendar: Calendar,
        unit: Unit,
        window: PointInTimeWindow,
        key: IdempotencyKey,
        import_reason: String,
        trace: SafeTraceContext,
    ) -> Result<(DataSnapshot, VersionRef, Ulid), ApplicationError> {
        let authorized_at = server_market_time();
        // This authorization/read gate deliberately precedes Definition reads, adapter lookup,
        // blob staging, and every mutation.
        let authorized =
            GovernedInputUseCase::new(self.authorizations.as_ref(), self.data_sources.as_ref())
                .resolve_authorized_data_source(
                    &principal,
                    &authorization_ref,
                    mapping.id(),
                    mapping.content_hash(),
                    ImportInterface::CanonicalQuoteSnapshot,
                    &authorized_at,
                )
                .await?;
        validate_import_definitions(
            self.definitions.as_ref(),
            principal.access_scope(),
            authorized.authorization().owner(),
            &mapping,
            &calendar,
            &unit,
        )
        .await?;
        let actor = principal.actor_id().clone();
        let scope = principal.access_scope().clone();
        let owner = authorized.authorization().owner().clone();
        let calendar_ref = VersionRef::new(
            Ulid::new(calendar.identity().to_owned()).map_err(map_domain_error)?,
            Version::new(calendar.version()).map_err(map_domain_error)?,
        );
        let unit_ref = VersionRef::new(
            Ulid::new(unit.identity().to_owned()).map_err(map_domain_error)?,
            Version::new(unit.version()).map_err(map_domain_error)?,
        );
        let resource = FoundationResourceRef::unversioned(
            FoundationResourceKind::DataSnapshot,
            snapshot_id.clone(),
        );
        let record_id =
            deterministic_change_record_id(&authorized_at, &actor, &resource, key.as_str())
                .map_err(map_domain_error)?;
        let context = FoundationChangeContext::authorized_import(
            principal,
            ChangeJustification::for_authorized_import(import_reason.clone())
                .map_err(map_domain_error)?,
            record_id,
            authorized_at.clone(),
        )?;
        let replay_request = CanonicalImportReplayRequest::new(
            context,
            owner,
            snapshot_id.clone(),
            authorization_ref.clone(),
            authorized.authorization_hash().clone(),
            mapping.id().clone(),
            mapping.content_hash().clone(),
            calendar_ref,
            unit_ref,
            window.as_of().clone(),
            window.visible_at_cutoff().clone(),
            key.clone(),
        )?;
        let publisher = PublishDataSnapshot::new(self.blob_store.as_ref(), self.snapshots.as_ref());
        if let Some(replay) = publisher.probe_replay(&replay_request).await? {
            let read = VerifiedSnapshotReader::new(
                self.verified_snapshots.as_ref(),
                self.blob_reader.as_ref(),
                self.integrity_events.as_ref(),
            )
            .read(&scope, replay.snapshot().id().clone(), trace)
            .await?;
            let VerifiedSnapshotResponse::Data {
                snapshot,
                authorization,
                authorization_hash,
                actor,
            } = verified_snapshot(read)?
            else {
                return Err(lineage_error());
            };
            if &snapshot != replay.snapshot()
                || &authorization != replay.authorization()
                || &authorization_hash != replay.authorization_hash()
                || &actor != replay.actor_id()
            {
                return Err(lineage_error());
            }
            return Ok((snapshot, authorization, actor));
        }
        let request = GovernedCanonicalImportRequest::new(
            snapshot_id.clone(),
            actor.clone(),
            authorized_at.clone(),
            authorized.authorization().clone(),
            authorized.data_source().clone(),
            mapping,
            calendar,
            unit,
            window,
            import_reason.clone(),
        )
        .map_err(map_data_error)?;
        let prepared = GovernedCanonicalQuoteImporter::new(self.catalog.as_ref())
            .prepare(request)
            .await
            .map_err(map_data_error)?;
        if prepared.authorization() != &authorization_ref
            || prepared.actor_id() != &actor
            || prepared.authorized_at() != &authorized_at
            || prepared.import_reason() != import_reason
        {
            return Err(lineage_error());
        }
        let (snapshot, parquet, manifest) = prepared.into_package().into_parts();
        let verified = CanonicalSnapshotCodec
            .decode_verified(snapshot.clone(), &parquet, &manifest)
            .map_err(map_data_error)?;
        let manifest_evidence = verified.manifest();
        let manifest_authorization = VersionRef::new(
            Ulid::new(
                manifest_evidence
                    .authorization_id()
                    .ok_or_else(lineage_error)?
                    .to_owned(),
            )
            .map_err(map_domain_error)?,
            Version::new(
                manifest_evidence
                    .authorization_version()
                    .ok_or_else(lineage_error)?,
            )
            .map_err(map_domain_error)?,
        );
        let manifest_actor = Ulid::new(
            manifest_evidence
                .actor_id()
                .ok_or_else(lineage_error)?
                .to_owned(),
        )
        .map_err(map_domain_error)?;
        let manifest_authorization_hash = parse_manifest_hash(
            manifest_evidence
                .authorization_hash()
                .ok_or_else(lineage_error)?,
        )?;
        let payloads = DataSnapshotPayloads::new_authorized(
            snapshot,
            parquet,
            manifest,
            key.clone(),
            CanonicalImportManifestEvidence::new(
                manifest_actor,
                manifest_authorization,
                manifest_authorization_hash,
            ),
        )?;
        let snapshot = publisher
            .execute_governed_import(replay_request, payloads)
            .await?;
        Ok((snapshot, authorization_ref, actor))
    }

    #[allow(clippy::too_many_arguments)]
    async fn publish_universe(
        &self,
        principal: ficant_application::AuthorizedPrincipal,
        snapshot_id: Ulid,
        owner: OwnerRef,
        members: Vec<VersionRef>,
        filter: ContentHash,
        lineage: Vec<LineageRef>,
        key: IdempotencyKey,
        change: ChangeJustification,
    ) -> Result<(UniverseSnapshot, Ulid), ApplicationError> {
        validate_universe_members(
            self.definitions.as_ref(),
            principal.access_scope(),
            &owner,
            &members,
        )
        .await?;
        let actor = principal.actor_id().clone();
        let occurred_at = server_market_time();
        let resource = FoundationResourceRef::unversioned(
            FoundationResourceKind::UniverseSnapshot,
            snapshot_id.clone(),
        );
        let record_id =
            deterministic_change_record_id(&occurred_at, &actor, &resource, key.as_str())
                .map_err(map_domain_error)?;
        let intent = UniverseSnapshotIntent::new(
            snapshot_id,
            owner,
            members,
            filter,
            lineage,
            actor.clone(),
            key,
        )?;
        let context =
            FoundationChangeContext::administrator(principal, change, record_id, occurred_at)?;
        let snapshot =
            PublishUniverseSnapshot::new(self.blob_store.as_ref(), self.snapshots.as_ref())
                .execute_governed_admin(context, intent)
                .await?;
        Ok((snapshot, actor))
    }
}

async fn validate_import_definitions(
    definitions: &dyn DefinitionRepository,
    scope: &ficant_application::AccessScope,
    expected_owner: &OwnerRef,
    mapping: &InstrumentMapping,
    calendar: &Calendar,
    unit: &Unit,
) -> Result<(), ApplicationError> {
    let use_case = DefinitionUseCase::new(definitions);
    let calendar_id = Ulid::new(calendar.identity().to_owned()).map_err(map_domain_error)?;
    let stored_calendar = use_case
        .get_exact(
            scope,
            calendar_id,
            Version::new(calendar.version()).map_err(map_domain_error)?,
        )
        .await?
        .ok_or_else(not_found)?;
    if stored_calendar != DefinitionValue::Calendar(calendar.clone()) {
        return Err(lineage_error());
    }
    let unit_id = Ulid::new(unit.identity().to_owned()).map_err(map_domain_error)?;
    let stored_unit = use_case
        .get_exact(
            scope,
            unit_id,
            Version::new(unit.version()).map_err(map_domain_error)?,
        )
        .await?
        .ok_or_else(not_found)?;
    if stored_unit != DefinitionValue::Unit(unit.clone()) {
        return Err(lineage_error());
    }
    let instruments = mapping
        .entries()
        .iter()
        .map(|entry| entry.instrument().clone())
        .collect::<BTreeSet<_>>();
    validate_universe_members(
        definitions,
        scope,
        expected_owner,
        &instruments.into_iter().collect::<Vec<_>>(),
    )
    .await
}

async fn validate_universe_members(
    definitions: &dyn DefinitionRepository,
    scope: &ficant_application::AccessScope,
    expected_owner: &OwnerRef,
    members: &[VersionRef],
) -> Result<(), ApplicationError> {
    if members.is_empty() {
        return Err(invalid());
    }
    let use_case = DefinitionUseCase::new(definitions);
    for member in members.iter().cloned().collect::<BTreeSet<_>>() {
        let value = use_case
            .get_exact(scope, member.id().clone(), member.version())
            .await?
            .ok_or_else(not_found)?;
        match value {
            DefinitionValue::Instrument(value)
                if value.owner() == expected_owner
                    && value.identity() == member.id().as_str()
                    && value.version() == member.version().get() => {}
            _ => return Err(lineage_error()),
        }
    }
    Ok(())
}

enum VerifiedSnapshotResponse {
    Data {
        snapshot: DataSnapshot,
        authorization: VersionRef,
        authorization_hash: ContentHash,
        actor: Ulid,
    },
    Universe {
        snapshot: UniverseSnapshot,
        actor: Ulid,
    },
}

fn verified_snapshot(
    value: VerifiedSnapshotRead,
) -> Result<VerifiedSnapshotResponse, ApplicationError> {
    match value {
        VerifiedSnapshotRead::Data {
            snapshot,
            parquet,
            manifest,
        } => {
            let verified = CanonicalSnapshotCodec
                .decode_verified(snapshot, parquet.bytes(), manifest.bytes())
                .map_err(map_data_error)?;
            let manifest = verified.manifest();
            let authorization = VersionRef::new(
                Ulid::new(
                    manifest
                        .authorization_id()
                        .ok_or_else(lineage_error)?
                        .to_owned(),
                )
                .map_err(map_domain_error)?,
                Version::new(manifest.authorization_version().ok_or_else(lineage_error)?)
                    .map_err(map_domain_error)?,
            );
            let actor = Ulid::new(manifest.actor_id().ok_or_else(lineage_error)?.to_owned())
                .map_err(map_domain_error)?;
            let authorization_hash =
                parse_manifest_hash(manifest.authorization_hash().ok_or_else(lineage_error)?)?;
            Ok(VerifiedSnapshotResponse::Data {
                snapshot: verified.snapshot().clone(),
                authorization,
                authorization_hash,
                actor,
            })
        }
        VerifiedSnapshotRead::Universe {
            snapshot,
            members_manifest,
        } => {
            let actor = verify_universe_snapshot_manifest(&snapshot, members_manifest.bytes())?;
            Ok(VerifiedSnapshotResponse::Universe { snapshot, actor })
        }
    }
}

fn parse_manifest_hash(value: &str) -> Result<ContentHash, ApplicationError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(lineage_error());
    }
    let mut bytes = [0_u8; 32];
    for (index, target) in bytes.iter_mut().enumerate() {
        let offset = index * 2;
        *target =
            u8::from_str_radix(&value[offset..offset + 2], 16).map_err(|_| lineage_error())?;
    }
    ContentHash::from_bytes(&bytes).map_err(map_domain_error)
}

fn data_snapshot(
    value: &DataSnapshot,
    authorization: &VersionRef,
    actor: &Ulid,
) -> pb::DataSnapshot {
    pb::DataSnapshot {
        data_snapshot_id: Some(ulid(value.id())),
        owner: Some(owner(value.owner())),
        visible_at: Some(market_time(value.visible_at())),
        as_of: Some(market_time(value.as_of())),
        schema_hash: Some(hash(value.schema_hash())),
        manifest_hash: Some(hash(value.manifest_hash())),
        blob_content_hash: Some(hash(value.content_hash())),
        lineage: value.lineage().iter().map(lineage).collect(),
        authorization_ref: Some(version_ref(authorization)),
        actor_id: Some(ulid(actor)),
    }
}

fn universe_snapshot(value: &UniverseSnapshot, actor: &Ulid) -> pb::UniverseSnapshot {
    pb::UniverseSnapshot {
        universe_snapshot_id: Some(ulid(value.id())),
        owner: Some(owner(value.owner())),
        instrument_versions: value
            .instrument_versions()
            .iter()
            .map(version_ref)
            .collect(),
        filter_digest: Some(hash(value.filter_digest())),
        content_hash: Some(hash(value.content_hash())),
        lineage: value.lineage().iter().map(lineage).collect(),
        actor_id: Some(ulid(actor)),
    }
}

fn parse_lineage(value: &core::LineageRef) -> Result<LineageRef, ApplicationError> {
    LineageRef::new(
        parse_ulid(value.object_id.as_ref())?,
        (value.version != 0)
            .then(|| Version::new(value.version).map_err(map_domain_error))
            .transpose()?,
        value
            .content_hash
            .as_ref()
            .map(|hash| parse_hash(Some(hash)))
            .transpose()?,
    )
    .map_err(map_domain_error)
}

fn lineage(value: &LineageRef) -> core::LineageRef {
    core::LineageRef {
        object_id: Some(ulid(value.object_id())),
        version: value.version().map_or(0, Version::get),
        content_hash: value.content_hash().map(hash),
    }
}

fn trace_context(message: &impl Message) -> SafeTraceContext {
    let digest = ContentHash::digest(&message.encode_to_vec());
    let token =
        digest.as_bytes()[..16]
            .iter()
            .fold(String::with_capacity(32), |mut output, byte| {
                use std::fmt::Write as _;
                write!(output, "{byte:02x}").expect("writing to String cannot fail");
                output
            });
    SafeTraceContext::new(token).expect("derived trace token is canonical")
}

fn map_data_error(error: DataError) -> ApplicationError {
    match error {
        DataError::InvalidConfiguration
        | DataError::InvalidSourceData
        | DataError::PointInTimeViolation
        | DataError::QualityRuleFailed
        | DataError::SchemaMismatch => {
            ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
        }
        DataError::SourceRowViolation {
            source_record_id, ..
        } => ApplicationError::observed_after_visible_source_row(source_record_id),
        DataError::SourceUnavailable => {
            ApplicationError::new(ApplicationErrorCategory::StorageUnavailable, true)
        }
        DataError::SnapshotIntegrityFailed => {
            ApplicationError::new(ApplicationErrorCategory::HashMismatch, false)
        }
    }
}

fn invalid() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}

fn forbidden() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::Forbidden, false)
}

fn not_found() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::NotFound, false)
}

fn lineage_error() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::LineageIncomplete, false)
}
