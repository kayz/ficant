use std::collections::BTreeMap;
use std::env;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ficant_api::{GrpcWebServerConfig, build_production_routes, serve_production_routes};
use ficant_application::ports::{
    AccessScope, AeadCursorCodec, BeginBlobStage, BlobStore, CursorKey, IdempotencyKey,
    PublishArtifact, PublishSignalSet, SignalRepository, VerifyBlobStage,
};
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::research::v1 as research;
use ficant_contracts::ficant::research::v1::artifact_service_client::ArtifactServiceClient;
use ficant_domain::governance::PlatformRole;
use ficant_domain::primitives::{
    ContentHash, EffectivePeriod, LineageRef, MarketTime, OwnerRef, Ulid, Version, VersionRef,
};
use ficant_domain::research::{Artifact, ArtifactKind, SignalSet, SignalSetInput};
use ficant_server::{ServerSettings, build_production_grpc_services};
use ficant_storage::postgres::PostgresRepository;
use ficant_storage::s3::S3BlobStore;
use object_store::ObjectStoreExt;
use object_store::aws::{AmazonS3, AmazonS3Builder};
use object_store::path::Path as ObjectPath;
use prost::Message;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tonic::Request;
use tonic::transport::Channel;

const KEY: &str = "3031323334353637383961626364656630313233343536373839616263646566";
const ALLOWED_ORIGIN: &str = "http://127.0.0.1:4174";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires the explicit check.ps1 -IncludeIntegration environment"]
#[allow(clippy::too_many_lines)]
async fn production_artifact_queries_verify_postgres_ceph_and_both_transports() {
    let Some(environment) = IntegrationEnvironment::load() else {
        eprintln!(
            "skipping R6B Artifact production SIT: integration environment is not configured"
        );
        return;
    };
    let pool = reset_and_migrate(&environment.database_url).await;
    let published = publish_server_owned_artifacts(&pool, &environment).await;

    let first_address = free_address();
    let first = RunningServer::start(first_address, &environment).await;
    let first_endpoint = format!("http://{first_address}");
    assert_native_exact_reads_and_pagination(&first_endpoint, &published).await;
    assert_grpc_web_exact_get(first_address, &published.generic_id).await;
    first.stop().await;

    let second_address = free_address();
    let second = RunningServer::start(second_address, &environment).await;
    let second_endpoint = format!("http://{second_address}");
    assert_native_exact_reads_and_pagination(&second_endpoint, &published).await;

    sqlx::query(
        "UPDATE research.artifacts SET media_type='application/r6b-tampered'
         WHERE tenant_id=$1 AND artifact_id=$2",
    )
    .bind(id('T').as_str())
    .bind(published.generic_id.as_str())
    .execute(&pool)
    .await
    .expect("metadata tamper is applied");
    assert_artifact_error(
        &second_endpoint,
        &published.generic_id,
        core::ErrorCode::ImmutableViolation,
    )
    .await;
    sqlx::query(
        "UPDATE research.artifacts SET media_type=$3
         WHERE tenant_id=$1 AND artifact_id=$2",
    )
    .bind(id('T').as_str())
    .bind(published.generic_id.as_str())
    .bind("application/vnd.ficant.r6b-generic")
    .execute(&pool)
    .await
    .expect("metadata is restored");

    sqlx::query(
        "UPDATE research.lineage_edges SET target_object_id=$3
         WHERE tenant_id=$1 AND source_object_id=$2 AND lineage_ordinal=0",
    )
    .bind(id('T').as_str())
    .bind(published.signal_artifact_id.as_str())
    .bind(id('Z').as_str())
    .execute(&pool)
    .await
    .expect("lineage edge tamper is applied");
    assert_artifact_error(
        &second_endpoint,
        &published.signal_artifact_id,
        core::ErrorCode::LineageIncomplete,
    )
    .await;
    sqlx::query(
        "UPDATE research.lineage_edges SET target_object_id=$3
         WHERE tenant_id=$1 AND source_object_id=$2 AND lineage_ordinal=0",
    )
    .bind(id('T').as_str())
    .bind(published.signal_artifact_id.as_str())
    .bind(published.data_snapshot_id.as_str())
    .execute(&pool)
    .await
    .expect("lineage edge is restored");

    sqlx::query(
        "UPDATE storage.blobs SET blob_size=blob_size + 1
         WHERE tenant_id=$1 AND content_hash=$2",
    )
    .bind(id('T').as_str())
    .bind(S3BlobStore::hash_hex(&published.signal_hash))
    .execute(&pool)
    .await
    .expect("blob reference size tamper is applied");
    assert_signal_error(
        &second_endpoint,
        &published.signal_id,
        core::ErrorCode::HashMismatch,
    )
    .await;
    sqlx::query(
        "UPDATE storage.blobs SET blob_size=$3
         WHERE tenant_id=$1 AND content_hash=$2",
    )
    .bind(id('T').as_str())
    .bind(S3BlobStore::hash_hex(&published.signal_hash))
    .bind(i64::try_from(published.signal_bytes.len()).expect("fixture size fits i64"))
    .execute(&pool)
    .await
    .expect("blob reference size is restored");

    let s3 = raw_s3_client(&environment);
    let signal_key = S3BlobStore::immutable_key(&published.signal_hash);
    s3.put(
        &ObjectPath::from(signal_key.as_str()),
        vec![b'x'; published.signal_bytes.len()].into(),
    )
    .await
    .expect("immutable bytes tamper is applied");
    assert_signal_error(
        &second_endpoint,
        &published.signal_id,
        core::ErrorCode::HashMismatch,
    )
    .await;
    s3.put(
        &ObjectPath::from(signal_key.as_str()),
        published.signal_bytes.clone().into(),
    )
    .await
    .expect("immutable bytes are restored");
    assert_signal_success(&second_endpoint, &published.signal_id).await;

    second.stop().await;
    pool.close().await;
}

struct PublishedFixture {
    generic_id: Ulid,
    signal_artifact_id: Ulid,
    signal_id: Ulid,
    data_snapshot_id: Ulid,
    signal_hash: ContentHash,
    signal_bytes: Vec<u8>,
}

#[allow(clippy::too_many_lines)]
async fn publish_server_owned_artifacts(
    pool: &PgPool,
    environment: &IntegrationEnvironment,
) -> PublishedFixture {
    let owner = OwnerRef::new(id('T'), id('P'));
    let scope = AccessScope::new(id('T'), id('R'), vec![id('P')]).expect("scope is valid");
    let repository = PostgresRepository::new(pool.clone(), test_cursor());
    let store = S3BlobStore::new(
        &environment.s3_endpoint,
        environment.s3_bucket.clone(),
        &environment.s3_access_key,
        &environment.s3_secret_key,
        pool.clone(),
    )
    .expect("S3 adapter is valid");
    let authorities = seed_signal_authorities(pool, &owner).await;

    let generic_id = id('G');
    let generic_bytes = b"r6b generic server-owned artifact".to_vec();
    let generic_hash = ContentHash::digest(&generic_bytes);
    let generic_blob =
        stage_verified(&store, &scope, &owner, "r6b/generic/stage", &generic_bytes).await;
    let generic = Artifact::new(
        generic_id.clone(),
        owner.clone(),
        ArtifactKind::Generic,
        "application/vnd.ficant.r6b-generic",
        generic_hash.clone(),
        u64::try_from(generic_bytes.len()).expect("fixture size fits u64"),
        vec![LineageRef::versioned(
            authorities.rule_pack.id().clone(),
            authorities.rule_pack.version(),
        )],
    )
    .expect("generic Artifact is valid");
    repository
        .publish_verified_blob(
            PublishArtifact::new(
                generic,
                generic_blob,
                IdempotencyKey::new("r6b/generic/publish").expect("idempotency key is valid"),
            )
            .expect("verified generic publication is valid"),
        )
        .await
        .expect("server-owned generic Artifact publishes");

    let signal_artifact_id = id('A');
    let signal_id = id('S');
    let signal_bytes = b"r6b signal-set server-owned artifact".to_vec();
    let signal_hash = ContentHash::digest(&signal_bytes);
    let signal_blob =
        stage_verified(&store, &scope, &owner, "r6b/signal/stage", &signal_bytes).await;
    let data_ref =
        LineageRef::content_addressed(authorities.data_snapshot_id.clone(), authorities.data_hash);
    let universe_ref =
        LineageRef::content_addressed(authorities.universe_snapshot_id, authorities.universe_hash);
    let input_ref = LineageRef::content_addressed(generic_id.clone(), generic_hash);
    let signal_artifact = Artifact::new(
        signal_artifact_id.clone(),
        owner.clone(),
        ArtifactKind::SignalSet,
        "application/vnd.ficant.signal-set",
        signal_hash.clone(),
        u64::try_from(signal_bytes.len()).expect("fixture size fits u64"),
        vec![
            data_ref.clone(),
            universe_ref.clone(),
            LineageRef::versioned(
                authorities.rule_pack.id().clone(),
                authorities.rule_pack.version(),
            ),
            input_ref.clone(),
        ],
    )
    .expect("SignalSet Artifact is valid");
    repository
        .publish_verified_blob(
            PublishArtifact::new(
                signal_artifact,
                signal_blob.clone(),
                IdempotencyKey::new("r6b/signal-artifact/publish")
                    .expect("idempotency key is valid"),
            )
            .expect("verified SignalSet Artifact publication is valid"),
        )
        .await
        .expect("server-owned SignalSet Artifact publishes");
    let signal = SignalSet::new(SignalSetInput {
        signal_set_id: signal_id.clone(),
        owner,
        artifact: LineageRef::content_addressed(signal_artifact_id.clone(), signal_hash.clone()),
        experiment_run_id: authorities.experiment_run_id,
        data_snapshot: data_ref,
        universe_snapshot: universe_ref,
        rule_packs: vec![authorities.rule_pack],
        input_artifacts: vec![input_ref],
        valid: EffectivePeriod::new(
            market_time("2026-08-19T01:00:00Z"),
            market_time("2026-08-20T01:00:00Z"),
        )
        .expect("signal validity is ordered"),
    })
    .expect("SignalSet is valid");
    repository
        .publish(
            PublishSignalSet::new(
                signal,
                signal_blob,
                IdempotencyKey::new("r6b/signal-set/publish").expect("idempotency key is valid"),
            )
            .expect("verified SignalSet publication is valid"),
        )
        .await
        .expect("server-owned SignalSet publishes");

    PublishedFixture {
        generic_id,
        signal_artifact_id,
        signal_id,
        data_snapshot_id: authorities.data_snapshot_id,
        signal_hash,
        signal_bytes,
    }
}

struct SignalAuthorities {
    data_snapshot_id: Ulid,
    data_hash: ContentHash,
    universe_snapshot_id: Ulid,
    universe_hash: ContentHash,
    rule_pack: VersionRef,
    experiment_run_id: Ulid,
}

async fn seed_signal_authorities(pool: &PgPool, owner: &OwnerRef) -> SignalAuthorities {
    let data_snapshot_id = id('D');
    let universe_snapshot_id = id('U');
    let experiment_run_id = id('X');
    let rule_pack_id = id('Q');
    let data_hash = ContentHash::digest(b"r6b data snapshot authority");
    let manifest_hash = ContentHash::digest(b"r6b data manifest authority");
    let universe_hash = ContentHash::digest(b"r6b universe snapshot authority");
    for hash in [&data_hash, &manifest_hash, &universe_hash] {
        sqlx::query(
            "INSERT INTO storage.blobs (tenant_id, content_hash, object_key, blob_size)
             VALUES ($1, $2, $3, 1)",
        )
        .bind(owner.tenant_id().as_str())
        .bind(S3BlobStore::hash_hex(hash))
        .bind(S3BlobStore::immutable_key(hash))
        .execute(pool)
        .await
        .expect("snapshot authority blob reference is inserted");
    }
    sqlx::query(
        "INSERT INTO research.data_snapshots
         (tenant_id, data_snapshot_id, owner_id, visible_at, as_of, schema_hash,
          manifest_hash, content_hash, idempotency_key, fingerprint, payload)
         VALUES ($1, $2, $3, '2026-08-19T02:00:00Z', '2026-08-19T01:00:00Z',
                 $4, $5, $6, 'r6b/data-authority', $7, $8)",
    )
    .bind(owner.tenant_id().as_str())
    .bind(data_snapshot_id.as_str())
    .bind(owner.owner_id().as_str())
    .bind(S3BlobStore::hash_hex(&ContentHash::digest(b"r6b schema")))
    .bind(S3BlobStore::hash_hex(&manifest_hash))
    .bind(S3BlobStore::hash_hex(&data_hash))
    .bind(vec![1_u8; 32])
    .bind(b"r6b data snapshot authority".to_vec())
    .execute(pool)
    .await
    .expect("DataSnapshot authority is inserted");
    sqlx::query(
        "INSERT INTO research.universe_snapshots
         (tenant_id, universe_snapshot_id, owner_id, filter_digest, content_hash,
          idempotency_key, fingerprint, payload)
         VALUES ($1, $2, $3, $4, $5, 'r6b/universe-authority', $6, $7)",
    )
    .bind(owner.tenant_id().as_str())
    .bind(universe_snapshot_id.as_str())
    .bind(owner.owner_id().as_str())
    .bind(S3BlobStore::hash_hex(&ContentHash::digest(
        b"r6b universe filter",
    )))
    .bind(S3BlobStore::hash_hex(&universe_hash))
    .bind(vec![2_u8; 32])
    .bind(b"r6b universe snapshot authority".to_vec())
    .execute(pool)
    .await
    .expect("UniverseSnapshot authority is inserted");
    sqlx::query(
        "INSERT INTO market.market_rule_packs
         (tenant_id, rule_pack_id, version, owner_id, market, rule_type, source,
          effective_from, effective_to, verification_status, content_hash, payload)
         VALUES ($1, $2, 1, $3, 'CGB', 'R6B_SIT', 'fixture://r6b',
                 '2026-01-01T00:00:00Z', '2027-01-01T00:00:00Z', 'VERIFIED', $4, $5)",
    )
    .bind(owner.tenant_id().as_str())
    .bind(rule_pack_id.as_str())
    .bind(owner.owner_id().as_str())
    .bind(S3BlobStore::hash_hex(&ContentHash::digest(
        b"r6b rule pack",
    )))
    .bind(b"r6b rule pack".to_vec())
    .execute(pool)
    .await
    .expect("RulePack authority is inserted");
    sqlx::query(
        "INSERT INTO research.experiment_runs
         (tenant_id, experiment_run_id, owner_id, state, revision, idempotency_key,
          fingerprint, payload)
         VALUES ($1, $2, $3, 'SUCCEEDED', 1, 'r6b/run-authority', $4, $5)",
    )
    .bind(owner.tenant_id().as_str())
    .bind(experiment_run_id.as_str())
    .bind(owner.owner_id().as_str())
    .bind(vec![3_u8; 32])
    .bind(b"r6b experiment run authority".to_vec())
    .execute(pool)
    .await
    .expect("ExperimentRun authority is inserted");

    SignalAuthorities {
        data_snapshot_id,
        data_hash,
        universe_snapshot_id,
        universe_hash,
        rule_pack: VersionRef::new(rule_pack_id, Version::new(1).expect("version is valid")),
        experiment_run_id,
    }
}

async fn stage_verified(
    store: &S3BlobStore,
    scope: &AccessScope,
    owner: &OwnerRef,
    idempotency_key: &str,
    bytes: &[u8],
) -> ficant_application::ports::VerifiedBlobRef {
    let staged = store
        .begin_stage(
            BeginBlobStage::new(
                scope.clone(),
                owner.clone(),
                u64::try_from(bytes.len()).expect("fixture size fits u64"),
                IdempotencyKey::new(idempotency_key).expect("idempotency key is valid"),
            )
            .expect("blob stage is valid"),
        )
        .await
        .expect("blob stage begins");
    store
        .append_chunk(scope, &staged, bytes.to_vec())
        .await
        .expect("blob bytes append");
    store
        .verify_and_promote(
            VerifyBlobStage::new(
                scope.clone(),
                staged,
                ContentHash::digest(bytes),
                u64::try_from(bytes.len()).expect("fixture size fits u64"),
            )
            .expect("blob verification intent is valid"),
        )
        .await
        .expect("blob verifies and promotes")
}

async fn assert_native_exact_reads_and_pagination(endpoint: &str, value: &PublishedFixture) {
    let mut client = ArtifactServiceClient::connect(endpoint.to_owned())
        .await
        .expect("Artifact client connects");
    let artifact = client
        .get_artifact(Request::new(research::GetArtifactRequest {
            artifact_id: Some(proto_id(&value.generic_id)),
        }))
        .await
        .expect("native Artifact transport succeeds")
        .into_inner();
    let Some(research::get_artifact_response::Result::Artifact(artifact)) = artifact.result else {
        panic!("verified Generic Artifact must be returned");
    };
    assert_eq!(artifact.artifact_id, Some(proto_id(&value.generic_id)));

    assert_signal_success_with_client(&mut client, &value.signal_id).await;
    let first_page = client
        .read_signal_set_lineage(Request::new(research::ReadSignalSetLineageRequest {
            signal_set_id: Some(proto_id(&value.signal_id)),
            page: Some(core::PageRequest {
                page_size: 2,
                cursor: String::new(),
            }),
        }))
        .await
        .expect("first lineage page transport succeeds")
        .into_inner();
    let Some(research::read_signal_set_lineage_response::Result::LineagePage(first_page)) =
        first_page.result
    else {
        panic!("first verified SignalSet lineage page must be returned");
    };
    assert_eq!(first_page.lineage.len(), 2);
    let cursor = first_page.page.expect("page metadata exists").next_cursor;
    assert!(!cursor.is_empty());
    let second_page = client
        .read_signal_set_lineage(Request::new(research::ReadSignalSetLineageRequest {
            signal_set_id: Some(proto_id(&value.signal_id)),
            page: Some(core::PageRequest {
                page_size: 2,
                cursor,
            }),
        }))
        .await
        .expect("second lineage page transport succeeds")
        .into_inner();
    let Some(research::read_signal_set_lineage_response::Result::LineagePage(second_page)) =
        second_page.result
    else {
        panic!("second verified SignalSet lineage page must be returned");
    };
    assert_eq!(second_page.lineage.len(), 2);
    let cursor = second_page.page.expect("page metadata exists").next_cursor;
    assert!(!cursor.is_empty());
    let third_page = client
        .read_signal_set_lineage(Request::new(research::ReadSignalSetLineageRequest {
            signal_set_id: Some(proto_id(&value.signal_id)),
            page: Some(core::PageRequest {
                page_size: 2,
                cursor,
            }),
        }))
        .await
        .expect("third lineage page transport succeeds")
        .into_inner();
    let Some(research::read_signal_set_lineage_response::Result::LineagePage(third_page)) =
        third_page.result
    else {
        panic!("third verified SignalSet lineage page must be returned");
    };
    assert_eq!(third_page.lineage.len(), 1);
    assert!(
        third_page
            .page
            .expect("page metadata exists")
            .next_cursor
            .is_empty()
    );
}

async fn assert_signal_success(endpoint: &str, signal_id: &Ulid) {
    let mut client = ArtifactServiceClient::connect(endpoint.to_owned())
        .await
        .expect("Artifact client connects");
    assert_signal_success_with_client(&mut client, signal_id).await;
}

async fn assert_signal_success_with_client(
    client: &mut ArtifactServiceClient<Channel>,
    signal_id: &Ulid,
) {
    let response = client
        .get_signal_set(Request::new(research::GetSignalSetRequest {
            signal_set_id: Some(proto_id(signal_id)),
        }))
        .await
        .expect("native SignalSet transport succeeds")
        .into_inner();
    let Some(research::get_signal_set_response::Result::SignalSet(signal)) = response.result else {
        panic!("verified SignalSet must be returned");
    };
    assert_eq!(signal.signal_set_id, Some(proto_id(signal_id)));
}

async fn assert_artifact_error(endpoint: &str, artifact_id: &Ulid, expected: core::ErrorCode) {
    let mut client = ArtifactServiceClient::connect(endpoint.to_owned())
        .await
        .expect("Artifact client connects");
    let response = client
        .get_artifact(Request::new(research::GetArtifactRequest {
            artifact_id: Some(proto_id(artifact_id)),
        }))
        .await
        .expect("integrity rejection stays in typed transport")
        .into_inner();
    let Some(research::get_artifact_response::Result::Error(error)) = response.result else {
        panic!("tampered Artifact must fail closed");
    };
    assert_eq!(error.code, expected as i32);
}

async fn assert_signal_error(endpoint: &str, signal_id: &Ulid, expected: core::ErrorCode) {
    let mut client = ArtifactServiceClient::connect(endpoint.to_owned())
        .await
        .expect("Artifact client connects");
    let response = client
        .get_signal_set(Request::new(research::GetSignalSetRequest {
            signal_set_id: Some(proto_id(signal_id)),
        }))
        .await
        .expect("integrity rejection stays in typed transport")
        .into_inner();
    let Some(research::get_signal_set_response::Result::Error(error)) = response.result else {
        panic!("tampered SignalSet must fail closed");
    };
    assert_eq!(error.code, expected as i32);
}

async fn assert_grpc_web_exact_get(address: SocketAddr, artifact_id: &Ulid) {
    let payload = research::GetArtifactRequest {
        artifact_id: Some(proto_id(artifact_id)),
    }
    .encode_to_vec();
    let mut frame = Vec::with_capacity(payload.len() + 5);
    frame.push(0);
    frame.extend_from_slice(
        &u32::try_from(payload.len())
            .expect("protobuf request length fits u32")
            .to_be_bytes(),
    );
    frame.extend_from_slice(&payload);
    let path = "/ficant.research.v1.ArtifactService/GetArtifact";
    let mut request = format!(
        "POST {path} HTTP/1.1\r\nHost: {address}\r\nOrigin: {ALLOWED_ORIGIN}\r\nContent-Type: application/grpc-web+proto\r\nX-Grpc-Web: 1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        frame.len()
    )
    .into_bytes();
    request.extend_from_slice(&frame);
    let response = exchange(address, request).await;
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("HTTP response contains headers");
    let headers = String::from_utf8_lossy(&response[..header_end]).to_ascii_lowercase();
    assert!(headers.starts_with("http/1.1 200 ok\r\n"));
    assert!(headers.contains("content-type: application/grpc-web+proto\r\n"));
    assert!(headers.contains(&format!(
        "access-control-allow-origin: {}\r\n",
        ALLOWED_ORIGIN.to_ascii_lowercase()
    )));
    assert!(
        response[(header_end + 4)..]
            .windows(artifact_id.as_str().len())
            .any(|window| window == artifact_id.as_str().as_bytes()),
        "gRPC-Web body must contain the verified Artifact response"
    );
}

async fn exchange(address: SocketAddr, request: Vec<u8>) -> Vec<u8> {
    tokio::task::spawn_blocking(move || {
        let mut stream =
            TcpStream::connect_timeout(&address, Duration::from_secs(2)).expect("client connects");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("read timeout is configured");
        stream.write_all(&request).expect("request writes");
        let mut response = Vec::new();
        stream.read_to_end(&mut response).expect("response reads");
        response
    })
    .await
    .expect("blocking exchange joins")
}

struct RunningServer {
    handle: tokio::task::JoinHandle<Result<(), ficant_api::GrpcWebServeError>>,
}

impl RunningServer {
    async fn start(address: SocketAddr, environment: &IntegrationEnvironment) -> Self {
        let settings = ServerSettings::try_from_values(&server_values(address, environment))
            .expect("R6B SIT settings are valid");
        let services =
            build_production_grpc_services(&settings).expect("production services compose");
        let routes = build_production_routes(services).expect("production routes are unique");
        let handle = tokio::spawn(serve_production_routes(
            GrpcWebServerConfig {
                bind: address,
                allowed_origins: vec![ALLOWED_ORIGIN.to_owned()],
            },
            routes,
        ));
        wait_until_listening(address).await;
        Self { handle }
    }

    async fn stop(self) {
        self.handle.abort();
        let _ = self.handle.await;
    }
}

struct IntegrationEnvironment {
    database_url: String,
    s3_endpoint: String,
    s3_bucket: String,
    s3_access_key: String,
    s3_secret_key: String,
    runtime_digest: String,
}

impl IntegrationEnvironment {
    fn load() -> Option<Self> {
        Some(Self {
            database_url: env::var("FICANT_TEST_DATABASE_URL").ok()?,
            s3_endpoint: env::var("FICANT_TEST_S3_ENDPOINT").ok()?,
            s3_bucket: env::var("FICANT_TEST_S3_BUCKET").ok()?,
            s3_access_key: env::var("FICANT_TEST_S3_ACCESS_KEY").ok()?,
            s3_secret_key: env::var("FICANT_TEST_S3_SECRET_KEY").ok()?,
            runtime_digest: env::var("FICANT_TEST_RUNTIME_IMAGE_DIGEST").ok()?,
        })
    }
}

async fn reset_and_migrate(database_url: &str) -> PgPool {
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(database_url)
        .await
        .expect("integration PostgreSQL is reachable");
    sqlx::raw_sql(
        "DROP SCHEMA IF EXISTS data CASCADE;
         DROP SCHEMA IF EXISTS storage CASCADE;
         DROP SCHEMA IF EXISTS research CASCADE;
         DROP SCHEMA IF EXISTS market CASCADE;
         DROP SCHEMA IF EXISTS core CASCADE;
         DROP TABLE IF EXISTS public._sqlx_migrations;",
    )
    .execute(&pool)
    .await
    .expect("integration database reset succeeds");
    let migrations = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../migrations/postgresql");
    sqlx::migrate::Migrator::new(migrations)
        .await
        .expect("migration directory is readable")
        .run(&pool)
        .await
        .expect("R6B migrations apply");
    pool
}

fn raw_s3_client(environment: &IntegrationEnvironment) -> AmazonS3 {
    AmazonS3Builder::new()
        .with_endpoint(&environment.s3_endpoint)
        .with_bucket_name(&environment.s3_bucket)
        .with_access_key_id(&environment.s3_access_key)
        .with_secret_access_key(&environment.s3_secret_key)
        .with_region("us-east-1")
        .with_allow_http(environment.s3_endpoint.starts_with("http://"))
        .with_virtual_hosted_style_request(false)
        .build()
        .expect("raw S3 client is valid")
}

fn server_values(
    address: SocketAddr,
    environment: &IntegrationEnvironment,
) -> BTreeMap<String, String> {
    let empty_input_root = env::temp_dir().join("ficant-r6b-no-input-adapter-use");
    BTreeMap::from([
        ("FICANT_GRPC_BIND".to_owned(), address.to_string()),
        (
            "FICANT_GRPC_WEB_ALLOWED_ORIGINS".to_owned(),
            ALLOWED_ORIGIN.to_owned(),
        ),
        ("FICANT_PLATFORM_SIGNING_KEY_HEX".to_owned(), KEY.to_owned()),
        ("FICANT_PLATFORM_TRACE_KEY_HEX".to_owned(), KEY.to_owned()),
        (
            "FICANT_CODE_COMMIT_SHA".to_owned(),
            ficant_server::compiled_git_commit_sha().to_owned(),
        ),
        (
            "FICANT_CODE_TREE_SHA".to_owned(),
            ficant_server::compiled_git_tree_sha().to_owned(),
        ),
        (
            "FICANT_SERVER_RUNTIME_IMAGE_DIGEST".to_owned(),
            format!("sha256:{}", "ab".repeat(32)),
        ),
        (
            "FICANT_SERVER_ENVIRONMENT_ATTESTATION".to_owned(),
            format!("sha256:{}", "cd".repeat(32)),
        ),
        (
            "FICANT_EXPERIMENT_DATABASE_URL".to_owned(),
            environment.database_url.clone(),
        ),
        (
            "FICANT_EXPERIMENT_S3_ENDPOINT".to_owned(),
            environment.s3_endpoint.clone(),
        ),
        (
            "FICANT_EXPERIMENT_S3_BUCKET".to_owned(),
            environment.s3_bucket.clone(),
        ),
        (
            "FICANT_EXPERIMENT_S3_ACCESS_KEY".to_owned(),
            environment.s3_access_key.clone(),
        ),
        (
            "FICANT_EXPERIMENT_S3_SECRET_KEY".to_owned(),
            environment.s3_secret_key.clone(),
        ),
        (
            "FICANT_EXPERIMENT_CURSOR_KEY_HEX".to_owned(),
            KEY.to_owned(),
        ),
        (
            "FICANT_EXPERIMENT_TENANT_ID".to_owned(),
            id('T').to_string(),
        ),
        ("FICANT_EXPERIMENT_OWNER_ID".to_owned(), id('P').to_string()),
        ("FICANT_EXPERIMENT_ACTOR_ID".to_owned(), id('R').to_string()),
        (
            "FICANT_EXPERIMENT_RUNTIME_IMAGE_DIGEST".to_owned(),
            environment.runtime_digest.clone(),
        ),
        (
            "FICANT_EXPERIMENT_ENVIRONMENT_ATTESTATION".to_owned(),
            "ficant.worker.environment.v1\narch=amd64\nos=linux\nprofile=test".to_owned(),
        ),
        (
            "FICANT_EXPERIMENT_NATIVE_SOURCE_DIGEST".to_owned(),
            ficant_native_nodes::native_node_source_digest_attestation(),
        ),
        (
            "FICANT_INPUT_FILE_NDJSON_ROOT".to_owned(),
            empty_input_root.to_string_lossy().into_owned(),
        ),
        (
            "FICANT_INPUT_FILE_CONNECTION_BINDING".to_owned(),
            "r6b-unused-file".to_owned(),
        ),
        (
            "FICANT_INPUT_POSTGRES_CONNECTION_BINDING".to_owned(),
            "r6b-unused-postgres".to_owned(),
        ),
        (
            "FICANT_LOOPBACK_SUBJECT".to_owned(),
            "r6b-researcher".to_owned(),
        ),
        ("FICANT_LOOPBACK_ACTOR_ID".to_owned(), id('R').to_string()),
        ("FICANT_LOOPBACK_TENANT_ID".to_owned(), id('T').to_string()),
        (
            "FICANT_LOOPBACK_ALLOWED_OWNER_IDS".to_owned(),
            id('P').to_string(),
        ),
        (
            "FICANT_LOOPBACK_ACTIVE_ROLE".to_owned(),
            role_name(PlatformRole::Researcher).to_owned(),
        ),
        (
            "FICANT_LOOPBACK_SCOPES".to_owned(),
            "artifacts:read".to_owned(),
        ),
    ])
}

fn test_cursor() -> Arc<AeadCursorCodec> {
    Arc::new(
        AeadCursorCodec::new(
            CursorKey::new("r6b-artifact-sit", [11_u8; 32]).expect("cursor key is valid"),
            vec![],
        )
        .expect("cursor codec is valid"),
    )
}

fn proto_id(value: &Ulid) -> core::Ulid {
    core::Ulid {
        value: value.as_str().to_owned(),
    }
}

fn id(suffix: char) -> Ulid {
    let suffix = if suffix == 'U' { 'V' } else { suffix };
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FB{suffix}"))
        .expect("fixture suffix forms a valid ULID")
}

fn market_time(instant: &str) -> MarketTime {
    MarketTime::new(
        instant.parse().expect("fixture instant is valid"),
        "Asia/Shanghai",
        instant[0..10].parse().expect("fixture date is valid"),
    )
    .expect("fixture MarketTime is valid")
}

const fn role_name(role: PlatformRole) -> &'static str {
    match role {
        PlatformRole::PlatformAdmin => "PLATFORM_ADMIN",
        PlatformRole::Researcher => "RESEARCHER",
    }
}

fn free_address() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("ephemeral listener binds");
    listener.local_addr().expect("listener has an address")
}

async fn wait_until_listening(address: SocketAddr) {
    for _ in 0..200 {
        if TcpStream::connect_timeout(&address, Duration::from_millis(20)).is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("R6B production server did not listen on {address}");
}
