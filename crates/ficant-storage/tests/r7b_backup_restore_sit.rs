mod support;

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use ficant_application::VerifiedReadFacade;
use ficant_application::ports::{
    ApplicationResult, ArtifactRepository, BeginBlobStage, BlobStore, FormalOutputRecord,
    FormalOutputRepository, IdempotencyKey, IntegrityEvent, IntegrityEventSink, PublishArtifact,
    SafeTraceContext, VerifyBlobStage,
};
use ficant_domain::ContentAddressed;
use ficant_domain::primitives::{ContentHash, LineageRef, OwnerRef, Ulid, Version};
use ficant_domain::research::{Artifact, ArtifactKind};
use ficant_runtime::{
    CodeBinding, FormalInputBinding, FormalInputBindingInput, FormalInputKind,
    FormalInputReference, FormalOutputEvidence, FormalOutputEvidenceInput, RuntimeBinding,
};
use ficant_storage::s3::{ImmutableObjectBackup, S3BlobStore};

const TENANT_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5R01";
const OWNER_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5R02";
const SUBJECT_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5R03";
const ARTIFACT_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5R04";
const RULE_PACK_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5R05";
const GRAPH_PAYLOAD: &[u8] = b"r7b-recovery-graph-output-v1";
const ANALYTICS_PAYLOAD: &[u8] = b"r7b-recovery-analytics-output-v1";

#[tokio::test]
async fn isolated_backup_restore_required_reads_are_bit_identical() {
    match required_env("FICANT_RECOVERY_PHASE").as_str() {
        "seed" => seed_source().await,
        "export-objects" => export_objects().await,
        "restore-objects" => restore_objects().await,
        "verify" => verify_restore().await,
        phase => panic!("unsupported FICANT_RECOVERY_PHASE '{phase}'"),
    }
}

async fn seed_source() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let repository = support::repository(pool.clone());
    seed_graph_lineage(&pool).await;
    let store = blob_store(pool);
    let scope = support::access_scope(&owner());

    let analytics = analytics_record();
    FormalOutputRepository::publish(&repository, &scope, analytics.clone())
        .await
        .expect("synchronous formal output publish");

    let graph = graph_artifact();
    let staged = BlobStore::begin_stage(
        &store,
        BeginBlobStage::new(
            scope.clone(),
            owner(),
            u64::try_from(GRAPH_PAYLOAD.len()).expect("size"),
            IdempotencyKey::new("r7b/recovery/graph-stage").expect("stage key"),
        )
        .expect("stage command"),
    )
    .await
    .expect("begin graph stage");
    BlobStore::append_chunk(&store, &scope, &staged, GRAPH_PAYLOAD.to_vec())
        .await
        .expect("append graph bytes");
    let verified = BlobStore::verify_and_promote(
        &store,
        VerifyBlobStage::new(
            scope,
            staged,
            graph.content_hash().clone(),
            u64::try_from(GRAPH_PAYLOAD.len()).expect("size"),
        )
        .expect("promote command"),
    )
    .await
    .expect("promote graph bytes");
    ArtifactRepository::publish_verified_blob(
        &repository,
        PublishArtifact::new_formal(
            graph.clone(),
            verified,
            IdempotencyKey::new("r7b/recovery/graph-artifact").expect("artifact key"),
            graph_evidence(),
        )
        .expect("artifact command"),
    )
    .await
    .expect("graph Artifact publish");

    let proof = format!(
        "graph_artifact_id\t{}\ngraph_output_identity\t{}\nanalytics_output_identity\t{}\n",
        graph.id(),
        hash_hex(graph_evidence().output_identity()),
        hash_hex(analytics.output_identity()),
    );
    fs::write(backup_root().join("proofs.tsv"), proof).expect("write recovery proof identities");
}

async fn export_objects() {
    let pool = support::postgres_pool().await;
    let store = blob_store(pool);
    let objects = store
        .list_immutable_objects()
        .await
        .expect("complete immutable-object listing");
    assert_eq!(objects.len(), 1, "isolated source must contain one object");
    let root = backup_root();
    let object_root = root.join("objects");
    fs::create_dir_all(&object_root).expect("create object backup directory");
    let mut index = String::new();
    for object in objects {
        let digest = hash_hex(object.content_hash());
        let relative = format!("objects/{digest}.blob");
        fs::write(root.join(&relative), object.bytes()).expect("write immutable object backup");
        writeln!(
            &mut index,
            "{}\t{}\t{}\t{}",
            object.key(),
            relative,
            object.size(),
            digest
        )
        .expect("write object index line");
    }
    fs::write(root.join("objects.tsv"), index).expect("write immutable object index");
}

async fn restore_objects() {
    let pool = support::postgres_pool().await;
    let store = blob_store(pool);
    let root = backup_root();
    let index = fs::read_to_string(root.join("objects.tsv")).expect("read object index");
    let mut expected = Vec::new();
    for line in index.lines() {
        let fields = line.split('\t').collect::<Vec<_>>();
        assert_eq!(fields.len(), 4, "object index shape");
        let key = fields[0];
        let relative = fields[1];
        let claimed_size = fields[2].parse::<u64>().expect("object size");
        let claimed_hash = fields[3];
        let bytes = fs::read(root.join(relative)).expect("read object backup bytes");
        assert_eq!(
            u64::try_from(bytes.len()).expect("size"),
            claimed_size,
            "object index size"
        );
        let object = ImmutableObjectBackup::new(key, bytes).expect("verified recovery object");
        assert_eq!(hash_hex(object.content_hash()), claimed_hash);
        expected.push(object.clone());
        store
            .restore_immutable_object(object)
            .await
            .expect("restore immutable object");
    }
    let actual = store
        .list_immutable_objects()
        .await
        .expect("restored immutable listing");
    assert_eq!(
        actual, expected,
        "fresh bucket must contain exactly the manifest"
    );
}

async fn verify_restore() {
    let pool = support::postgres_pool().await;
    let repository = support::repository(pool.clone());
    let store = blob_store(pool.clone());
    let scope = support::access_scope(&owner());
    let analytics = analytics_record();
    let loaded = FormalOutputRepository::get(&repository, &scope, analytics.output_identity())
        .await
        .expect("analytics required read")
        .expect("analytics output exists");
    assert_eq!(loaded, analytics, "analytics bytes/evidence/identity");

    let events = CountingIntegritySink::default();
    let facade = VerifiedReadFacade::new(&repository, &repository, &repository, &store, &events);
    let graph = facade
        .read_verified_artifact(
            &scope,
            id(ARTIFACT_ID),
            SafeTraceContext::new("0123456789abcdef0123456789abcdef").expect("trace"),
        )
        .await
        .expect("Graph Artifact required read");
    assert_eq!(graph.artifact(), &graph_artifact());
    assert_eq!(graph.payload().bytes(), GRAPH_PAYLOAD);
    assert_eq!(
        ArtifactRepository::get_formal_evidence(&repository, &scope, id(ARTIFACT_ID))
            .await
            .expect("Graph evidence read"),
        Some(graph_evidence())
    );
    assert_eq!(events.count.load(Ordering::SeqCst), 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM analytics.formal_outputs")
            .fetch_one(&pool)
            .await
            .expect("analytics count"),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM research.artifacts")
            .fetch_one(&pool)
            .await
            .expect("artifact count"),
        1
    );
}

fn analytics_record() -> FormalOutputRecord {
    FormalOutputRecord::new(
        owner(),
        evidence("ficant.rates.v1.BondAnalysisResult", ANALYTICS_PAYLOAD),
        ANALYTICS_PAYLOAD.to_vec(),
    )
    .expect("analytics formal output")
}

fn graph_artifact() -> Artifact {
    let rule_pack = match rule_pack_binding().reference() {
        FormalInputReference::Object(value) => value.clone(),
        FormalInputReference::Named(_) => panic!("RulePack must be object-backed"),
    };
    Artifact::new(
        id(ARTIFACT_ID),
        owner(),
        ArtifactKind::Generic,
        "application/vnd.ficant.graph-output.v1",
        ContentHash::digest(GRAPH_PAYLOAD),
        u64::try_from(GRAPH_PAYLOAD.len()).expect("size"),
        vec![rule_pack],
    )
    .expect("formal Graph Artifact")
}

fn graph_evidence() -> FormalOutputEvidence {
    evidence("ficant.research.v1.Artifact", GRAPH_PAYLOAD)
}

fn evidence(schema_id: &str, payload: &[u8]) -> FormalOutputEvidence {
    FormalOutputEvidence::new(FormalOutputEvidenceInput {
        schema_id: schema_id.to_owned(),
        subject: subject_binding(),
        consumed_inputs: vec![rule_pack_binding()],
        code: CodeBinding::new(
            required_env("FICANT_RECOVERY_CODE_COMMIT_SHA"),
            required_env("FICANT_RECOVERY_CODE_TREE_SHA"),
        )
        .expect("exact recovery Code binding"),
        runtime: RuntimeBinding::new(
            parse_digest(&required_env("FICANT_RECOVERY_RUNTIME_IMAGE_DIGEST")),
            ContentHash::digest(b"ficant.recovery.environment.v1"),
        ),
        implementations: Vec::new(),
        parameters_hash: ContentHash::digest(schema_id.as_bytes()),
        seed: None,
        result_hash: ContentHash::digest(payload),
    })
    .expect("formal output evidence")
}

fn subject_binding() -> FormalInputBinding {
    FormalInputBinding::new(FormalInputBindingInput {
        role: "subject".to_owned(),
        kind: FormalInputKind::Subject,
        owner: owner(),
        reference: FormalInputReference::Object(
            LineageRef::new(
                id(SUBJECT_ID),
                Some(Version::new(1).expect("version")),
                Some(ContentHash::digest(b"r7b-recovery-subject-v1")),
            )
            .expect("subject ref"),
        ),
        observed_at: None,
        visible_at: None,
        effective_from: None,
        effective_to: None,
    })
    .expect("Subject binding")
}

fn rule_pack_binding() -> FormalInputBinding {
    FormalInputBinding::new(FormalInputBindingInput {
        role: "rule_pack".to_owned(),
        kind: FormalInputKind::RulePack,
        owner: owner(),
        reference: FormalInputReference::Object(
            LineageRef::new(
                id(RULE_PACK_ID),
                Some(Version::new(1).expect("version")),
                Some(ContentHash::digest(b"r7b-recovery-rule-pack-v1")),
            )
            .expect("RulePack ref"),
        ),
        observed_at: None,
        visible_at: None,
        effective_from: None,
        effective_to: None,
    })
    .expect("RulePack binding")
}

async fn seed_graph_lineage(pool: &sqlx::PgPool) {
    let binding = rule_pack_binding();
    let reference = match binding.reference() {
        FormalInputReference::Object(value) => value,
        FormalInputReference::Named(_) => panic!("RulePack must be object-backed"),
    };
    sqlx::query(
        "INSERT INTO market.market_rule_packs
         (tenant_id,rule_pack_id,version,owner_id,market,rule_type,source,
          effective_from,effective_to,verification_status,content_hash,payload)
         VALUES ($1,$2,$3,$4,'CGB','recovery','r7b',
                 CURRENT_TIMESTAMP-INTERVAL '1 day',CURRENT_TIMESTAMP+INTERVAL '1 day',
                 'VERIFIED',$5,$6)",
    )
    .bind(owner().tenant_id().as_str())
    .bind(reference.object_id().as_str())
    .bind(i64::try_from(reference.version().expect("RulePack version").get()).expect("version"))
    .bind(owner().owner_id().as_str())
    .bind(hash_hex(
        reference.content_hash().expect("RulePack content hash"),
    ))
    .bind(vec![1_u8])
    .execute(pool)
    .await
    .expect("RulePack lineage target");
}

fn blob_store(pool: sqlx::PgPool) -> S3BlobStore {
    let (endpoint, bucket, access_key, secret_key) = support::s3_environment();
    S3BlobStore::new(&endpoint, bucket, &access_key, &secret_key, pool).expect("S3 adapter")
}

fn owner() -> OwnerRef {
    OwnerRef::new(id(TENANT_ID), id(OWNER_ID))
}

fn id(value: &str) -> Ulid {
    Ulid::new(value).expect("fixture ULID")
}

fn required_env(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| panic!("{name} is required"))
}

fn backup_root() -> PathBuf {
    let value = required_env("FICANT_RECOVERY_BACKUP_ROOT");
    let root = Path::new(&value);
    assert!(root.is_absolute(), "recovery backup root must be absolute");
    root.to_path_buf()
}

fn parse_digest(value: &str) -> ContentHash {
    let hex = value.strip_prefix("sha256:").expect("sha256 digest prefix");
    assert_eq!(hex.len(), 64, "digest length");
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).expect("digest hex"))
        .collect::<Vec<_>>();
    ContentHash::from_bytes(&bytes).expect("ContentHash")
}

fn hash_hex(value: &ContentHash) -> String {
    value
        .as_bytes()
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(&mut output, "{byte:02x}").expect("String write");
            output
        })
}

#[derive(Default)]
struct CountingIntegritySink {
    count: AtomicUsize,
}

#[async_trait]
impl IntegrityEventSink for CountingIntegritySink {
    async fn emit(&self, _event: IntegrityEvent) -> ApplicationResult<()> {
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
