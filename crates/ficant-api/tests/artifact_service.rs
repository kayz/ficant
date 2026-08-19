use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use chrono::{NaiveDate, TimeZone, Utc};
use ficant_api::{
    ArtifactGrpcService, PlatformApplication, PlatformPort, SessionPolicy, SystemClock,
    TrustedIdentity,
};
use ficant_application::AccessScope;
use ficant_application::ports::ApplicationResult;
use ficant_application::ports::{
    AeadCursorCodec, ArtifactRepository, CursorKey, IntegrityEvent, IntegrityEventSink,
    PublishArtifact, PublishSignalSet, RequiredVerifiedBlobRead, SignalRepository,
    SnapshotVerifiedReadMetadata, SnapshotVerifiedReadMetadataRepository, VerifiedBlobPayload,
    VerifiedBlobReader,
};
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::research::v1 as pb;
use ficant_contracts::ficant::research::v1::artifact_service_server::ArtifactService;
use ficant_domain::governance::PlatformRole;
use ficant_domain::primitives::{
    ContentHash, EffectivePeriod, LineageRef, MarketTime, OwnerRef, Ulid, Version, VersionRef,
};
use ficant_domain::research::{Artifact, ArtifactKind, SignalSet, SignalSetInput};
use tonic::Request;

const KEY: &[u8] = b"0123456789abcdef0123456789abcdef";
const PAYLOAD: &[u8] = b"r6b-artifact-payload";

#[tokio::test]
async fn researcher_reads_verified_metadata_and_canonical_lineage_pages() {
    fn assert_service<T: ArtifactService>() {}
    assert_service::<ArtifactGrpcService>();

    let ports = Arc::new(Ports::new(false));
    let service = service(
        Arc::clone(&ports),
        PlatformRole::Researcher,
        vec!["artifacts:read"],
        actor(),
        vec![owner().owner_id().clone()],
    );
    let artifact = service
        .get_artifact(Request::new(pb::GetArtifactRequest {
            artifact_id: Some(proto_id(&artifact_id())),
        }))
        .await
        .unwrap()
        .into_inner();
    let Some(pb::get_artifact_response::Result::Artifact(artifact)) = artifact.result else {
        panic!("verified Artifact read must succeed")
    };
    assert_eq!(artifact.kind, pb::ArtifactKind::SignalSet as i32);
    assert_eq!(artifact.lineage.len(), 4);

    let signal = service
        .get_signal_set(Request::new(pb::GetSignalSetRequest {
            signal_set_id: Some(proto_id(&signal_id())),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        signal.result,
        Some(pb::get_signal_set_response::Result::SignalSet(_))
    ));

    let first = service
        .read_artifact_lineage(Request::new(pb::ReadArtifactLineageRequest {
            artifact_id: Some(proto_id(&artifact_id())),
            page: Some(core::PageRequest {
                page_size: 2,
                cursor: String::new(),
            }),
        }))
        .await
        .unwrap()
        .into_inner();
    let Some(pb::read_artifact_lineage_response::Result::LineagePage(first)) = first.result else {
        panic!("first Artifact lineage page must succeed")
    };
    assert_eq!(first.lineage.len(), 2);
    let cursor = first.page.unwrap().next_cursor;
    assert!(!cursor.is_empty());

    let second = service
        .read_artifact_lineage(Request::new(pb::ReadArtifactLineageRequest {
            artifact_id: Some(proto_id(&artifact_id())),
            page: Some(core::PageRequest {
                page_size: 2,
                cursor,
            }),
        }))
        .await
        .unwrap()
        .into_inner();
    let Some(pb::read_artifact_lineage_response::Result::LineagePage(second)) = second.result
    else {
        panic!("second Artifact lineage page must succeed")
    };
    assert_eq!(second.lineage.len(), 2);
    assert!(second.page.unwrap().next_cursor.is_empty());
    assert_eq!(ports.required_reads.load(Ordering::SeqCst), 4);
    assert_eq!(ports.integrity_events.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn role_scope_owner_and_cursor_drift_fail_closed() {
    let ports = Arc::new(Ports::new(false));
    for (role, scopes) in [
        (PlatformRole::PlatformAdmin, vec!["artifacts:read"]),
        (PlatformRole::Researcher, Vec::new()),
    ] {
        let service = service(
            Arc::clone(&ports),
            role,
            scopes,
            actor(),
            vec![owner().owner_id().clone()],
        );
        let response = service
            .get_artifact(Request::new(pb::GetArtifactRequest {
                artifact_id: Some(proto_id(&artifact_id())),
            }))
            .await
            .unwrap()
            .into_inner();
        let Some(pb::get_artifact_response::Result::Error(error)) = response.result else {
            panic!("role/scope drift must fail")
        };
        assert_eq!(error.code, core::ErrorCode::Forbidden as i32);
    }
    assert_eq!(ports.artifact_reads.load(Ordering::SeqCst), 0);
    assert_eq!(ports.required_reads.load(Ordering::SeqCst), 0);

    let wrong_owner = service(
        Arc::clone(&ports),
        PlatformRole::Researcher,
        vec!["artifacts:read"],
        actor(),
        vec![id("01ARZ3NDEKTSV4RRFFQ69G5F03")],
    );
    let response = wrong_owner
        .get_artifact(Request::new(pb::GetArtifactRequest {
            artifact_id: Some(proto_id(&artifact_id())),
        }))
        .await
        .unwrap()
        .into_inner();
    let Some(pb::get_artifact_response::Result::Error(error)) = response.result else {
        panic!("owner drift must fail")
    };
    assert_eq!(error.code, core::ErrorCode::Forbidden as i32);
    assert_eq!(ports.required_reads.load(Ordering::SeqCst), 0);

    let valid = service(
        Arc::clone(&ports),
        PlatformRole::Researcher,
        vec!["artifacts:read"],
        actor(),
        vec![owner().owner_id().clone()],
    );
    let first = valid
        .read_artifact_lineage(Request::new(pb::ReadArtifactLineageRequest {
            artifact_id: Some(proto_id(&artifact_id())),
            page: Some(core::PageRequest {
                page_size: 1,
                cursor: String::new(),
            }),
        }))
        .await
        .unwrap()
        .into_inner();
    let Some(pb::read_artifact_lineage_response::Result::LineagePage(first)) = first.result else {
        panic!("valid cursor seed must succeed")
    };
    let cursor = first.page.unwrap().next_cursor;
    let drifted = valid
        .read_artifact_lineage(Request::new(pb::ReadArtifactLineageRequest {
            artifact_id: Some(proto_id(&artifact_id())),
            page: Some(core::PageRequest {
                page_size: 2,
                cursor,
            }),
        }))
        .await
        .unwrap()
        .into_inner();
    let Some(pb::read_artifact_lineage_response::Result::Error(error)) = drifted.result else {
        panic!("page-size cursor drift must fail")
    };
    assert_eq!(error.code, core::ErrorCode::Forbidden as i32);
}

#[tokio::test]
async fn payload_integrity_loss_returns_hash_mismatch_and_emits_once() {
    let ports = Arc::new(Ports::new(true));
    let service = service(
        Arc::clone(&ports),
        PlatformRole::Researcher,
        vec!["artifacts:read"],
        actor(),
        vec![owner().owner_id().clone()],
    );
    let response = service
        .get_artifact(Request::new(pb::GetArtifactRequest {
            artifact_id: Some(proto_id(&artifact_id())),
        }))
        .await
        .unwrap()
        .into_inner();
    let Some(pb::get_artifact_response::Result::Error(error)) = response.result else {
        panic!("tampered payload must fail")
    };
    assert_eq!(error.code, core::ErrorCode::HashMismatch as i32);
    assert_eq!(ports.integrity_events.load(Ordering::SeqCst), 1);
}

struct Ports {
    artifact: Artifact,
    signal: SignalSet,
    tamper: bool,
    artifact_reads: AtomicUsize,
    required_reads: AtomicUsize,
    integrity_events: AtomicUsize,
}

impl Ports {
    fn new(tamper: bool) -> Self {
        let data = LineageRef::versioned(data_id(), Version::new(1).unwrap());
        let universe = LineageRef::versioned(universe_id(), Version::new(1).unwrap());
        let rule = VersionRef::new(rule_id(), Version::new(1).unwrap());
        let input = LineageRef::versioned(input_id(), Version::new(1).unwrap());
        let hash = ContentHash::digest(PAYLOAD);
        let artifact = Artifact::new(
            artifact_id(),
            owner(),
            ArtifactKind::SignalSet,
            "application/vnd.ficant.signal-set",
            hash.clone(),
            PAYLOAD.len() as u64,
            vec![
                data.clone(),
                universe.clone(),
                LineageRef::versioned(rule.id().clone(), rule.version()),
                input.clone(),
            ],
        )
        .unwrap();
        let signal = SignalSet::new(SignalSetInput {
            signal_set_id: signal_id(),
            owner: owner(),
            artifact: LineageRef::content_addressed(artifact_id(), hash),
            experiment_run_id: run_id(),
            data_snapshot: data,
            universe_snapshot: universe,
            rule_packs: vec![rule],
            input_artifacts: vec![input],
            valid: EffectivePeriod::new(time(1), time(2)).unwrap(),
        })
        .unwrap();
        Self {
            artifact,
            signal,
            tamper,
            artifact_reads: AtomicUsize::new(0),
            required_reads: AtomicUsize::new(0),
            integrity_events: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl ArtifactRepository for Ports {
    async fn publish_verified_blob(&self, _: PublishArtifact) -> ApplicationResult<Artifact> {
        panic!("public Artifact query must never publish")
    }

    async fn get_metadata(
        &self,
        scope: &AccessScope,
        artifact_id: Ulid,
    ) -> ApplicationResult<Option<Artifact>> {
        self.artifact_reads.fetch_add(1, Ordering::SeqCst);
        scope.authorize(self.artifact.owner())?;
        Ok((artifact_id == *self.artifact.id()).then(|| self.artifact.clone()))
    }
}

#[async_trait]
impl SignalRepository for Ports {
    async fn publish(&self, _: PublishSignalSet) -> ApplicationResult<SignalSet> {
        panic!("public SignalSet query must never publish")
    }

    async fn get(
        &self,
        scope: &AccessScope,
        signal_set_id: Ulid,
    ) -> ApplicationResult<Option<SignalSet>> {
        scope.authorize(self.signal.owner())?;
        Ok((signal_set_id == *self.signal.id()).then(|| self.signal.clone()))
    }
}

#[async_trait]
impl SnapshotVerifiedReadMetadataRepository for Ports {
    async fn get_verified_read_metadata(
        &self,
        _: &AccessScope,
        _: Ulid,
    ) -> ApplicationResult<Option<SnapshotVerifiedReadMetadata>> {
        Ok(None)
    }
}

#[async_trait]
impl VerifiedBlobReader for Ports {
    async fn read_required(
        &self,
        request: &RequiredVerifiedBlobRead,
        sink: &dyn IntegrityEventSink,
    ) -> ApplicationResult<VerifiedBlobPayload> {
        self.required_reads.fetch_add(1, Ordering::SeqCst);
        let bytes = if self.tamper {
            b"tampered".to_vec()
        } else {
            PAYLOAD.to_vec()
        };
        request.verify_bytes(sink, bytes).await
    }
}

#[async_trait]
impl IntegrityEventSink for Ports {
    async fn emit(&self, _: IntegrityEvent) -> ApplicationResult<()> {
        self.integrity_events.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn service(
    ports: Arc<Ports>,
    role: PlatformRole,
    scopes: Vec<&str>,
    actor_id: Ulid,
    allowed_owner_ids: Vec<Ulid>,
) -> ArtifactGrpcService {
    let identity = TrustedIdentity::implicit(
        "artifact-test",
        actor_id,
        owner().tenant_id().clone(),
        allowed_owner_ids,
        role,
        scopes,
    )
    .unwrap();
    let platform: Arc<dyn PlatformPort> = Arc::new(
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
    ArtifactGrpcService::new(
        platform,
        ports.clone(),
        ports.clone(),
        ports.clone(),
        ports.clone(),
        ports,
        Arc::new(
            AeadCursorCodec::new(CursorKey::new("artifact-test", [7_u8; 32]).unwrap(), vec![])
                .unwrap(),
        ),
        KEY,
    )
    .unwrap()
}

fn time(hour: u32) -> MarketTime {
    MarketTime::new(
        Utc.with_ymd_and_hms(2026, 8, 19, hour, 0, 0)
            .single()
            .unwrap(),
        "Asia/Shanghai",
        NaiveDate::from_ymd_opt(2026, 8, 19).unwrap(),
    )
    .unwrap()
}

fn owner() -> OwnerRef {
    OwnerRef::new(
        id("01ARZ3NDEKTSV4RRFFQ69G5F01"),
        id("01ARZ3NDEKTSV4RRFFQ69G5F02"),
    )
}

fn actor() -> Ulid {
    id("01ARZ3NDEKTSV4RRFFQ69G5F00")
}

fn artifact_id() -> Ulid {
    id("01ARZ3NDEKTSV4RRFFQ69G5F18")
}

fn signal_id() -> Ulid {
    id("01ARZ3NDEKTSV4RRFFQ69G5F19")
}

fn run_id() -> Ulid {
    id("01ARZ3NDEKTSV4RRFFQ69G5F60")
}

fn data_id() -> Ulid {
    id("01ARZ3NDEKTSV4RRFFQ69G5F61")
}

fn universe_id() -> Ulid {
    id("01ARZ3NDEKTSV4RRFFQ69G5F62")
}

fn rule_id() -> Ulid {
    id("01ARZ3NDEKTSV4RRFFQ69G5F63")
}

fn input_id() -> Ulid {
    id("01ARZ3NDEKTSV4RRFFQ69G5F64")
}

fn id(value: &str) -> Ulid {
    Ulid::new(value).unwrap()
}

fn proto_id(value: &Ulid) -> core::Ulid {
    core::Ulid {
        value: value.as_str().to_owned(),
    }
}
