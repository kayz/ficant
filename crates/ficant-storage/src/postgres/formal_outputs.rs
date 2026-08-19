use async_trait::async_trait;
use chrono::{NaiveDate, TimeZone, Utc};
use ficant_application::ports::{
    AccessScope, ApplicationResult, FormalOutputRecord, FormalOutputRepository,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory, map_domain_error};
use ficant_contracts::ficant::core::v1 as pb;
use ficant_domain::primitives::{ContentHash, LineageRef, MarketTime, OwnerRef, Ulid, Version};
use ficant_runtime::{
    CodeBinding, FormalImplementationBinding, FormalInputBinding, FormalInputBindingInput,
    FormalInputKind, FormalInputReference, FormalOutputEvidence, FormalOutputEvidenceInput,
    NamedContentRef, RuntimeBinding,
};
use prost::Message;
use sqlx::postgres::PgRow;
use sqlx::{FromRow, Row};

use super::PostgresRepository;
use super::common::{application_error, map_sqlx_error};

#[derive(Debug)]
struct FormalOutputRow {
    output_identity: String,
    owner_id: String,
    schema_id: String,
    subject_id: String,
    subject_version: i64,
    subject_content_hash: String,
    code_commit_sha: String,
    code_tree_sha: String,
    code_digest: String,
    runtime_image_digest: String,
    environment_digest: String,
    parameters_hash: String,
    seed: Option<String>,
    result_hash: String,
    result_payload: Vec<u8>,
    formal_evidence: Vec<u8>,
}

impl<'row> FromRow<'row, PgRow> for FormalOutputRow {
    fn from_row(row: &'row PgRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            output_identity: row.try_get("output_identity")?,
            owner_id: row.try_get("owner_id")?,
            schema_id: row.try_get("schema_id")?,
            subject_id: row.try_get("subject_id")?,
            subject_version: row.try_get("subject_version")?,
            subject_content_hash: row.try_get("subject_content_hash")?,
            code_commit_sha: row.try_get("code_commit_sha")?,
            code_tree_sha: row.try_get("code_tree_sha")?,
            code_digest: row.try_get("code_digest")?,
            runtime_image_digest: row.try_get("runtime_image_digest")?,
            environment_digest: row.try_get("environment_digest")?,
            parameters_hash: row.try_get("parameters_hash")?,
            seed: row.try_get("seed")?,
            result_hash: row.try_get("result_hash")?,
            result_payload: row.try_get("result_payload")?,
            formal_evidence: row.try_get("formal_evidence")?,
        })
    }
}

#[async_trait]
impl FormalOutputRepository for PostgresRepository {
    async fn publish(
        &self,
        scope: &AccessScope,
        record: FormalOutputRecord,
    ) -> ApplicationResult<FormalOutputRecord> {
        scope.authorize(record.owner())?;
        record.verify()?;
        let evidence = record.evidence();
        let subject = object_reference(evidence.subject())?;
        let subject_version = subject
            .version()
            .ok_or_else(integrity_error)
            .and_then(|value| i64::try_from(value.get()).map_err(|_| validation_error()))?;
        let subject_hash = subject.content_hash().ok_or_else(integrity_error)?;
        let encoded_evidence = proto_evidence(evidence).encode_to_vec();
        let seed = evidence.seed().map(|value| value.to_string());
        sqlx::query(
            "INSERT INTO analytics.formal_outputs
             (tenant_id, output_identity, owner_id, schema_id,
              subject_id, subject_version, subject_content_hash,
              code_commit_sha, code_tree_sha, code_digest,
              runtime_image_digest, environment_digest, parameters_hash, seed,
              result_hash, result_payload, formal_evidence)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13,
                     $14::numeric, $15, $16, $17)
             ON CONFLICT (tenant_id, output_identity) DO NOTHING",
        )
        .bind(scope.tenant_id().as_str())
        .bind(hash_hex(record.output_identity()))
        .bind(record.owner().owner_id().as_str())
        .bind(evidence.schema_id())
        .bind(subject.object_id().as_str())
        .bind(subject_version)
        .bind(hash_hex(subject_hash))
        .bind(evidence.code().git_commit_sha())
        .bind(evidence.code().git_tree_sha())
        .bind(hash_hex(evidence.code().digest()))
        .bind(hash_hex(evidence.runtime().image_digest()))
        .bind(hash_hex(evidence.runtime().environment_digest()))
        .bind(hash_hex(evidence.parameters_hash()))
        .bind(seed)
        .bind(hash_hex(evidence.result_hash()))
        .bind(record.canonical_payload())
        .bind(&encoded_evidence)
        .execute(self.pool())
        .await
        .map_err(map_sqlx_error)?;

        let stored = load_row(self, scope, record.output_identity())
            .await?
            .ok_or_else(|| application_error(ApplicationErrorCategory::StorageUnavailable, true))?;
        if stored != record {
            return Err(application_error(
                ApplicationErrorCategory::ImmutableViolation,
                false,
            ));
        }
        Ok(stored)
    }

    async fn get(
        &self,
        scope: &AccessScope,
        output_identity: &ContentHash,
    ) -> ApplicationResult<Option<FormalOutputRecord>> {
        load_row(self, scope, output_identity).await
    }
}

async fn load_row(
    repository: &PostgresRepository,
    scope: &AccessScope,
    output_identity: &ContentHash,
) -> ApplicationResult<Option<FormalOutputRecord>> {
    let owners = scope
        .allowed_owner_ids()
        .iter()
        .map(|value| value.as_str().to_owned())
        .collect::<Vec<_>>();
    let row = sqlx::query_as::<_, FormalOutputRow>(
        "SELECT output_identity::text, owner_id::text, schema_id,
                subject_id::text, subject_version, subject_content_hash::text,
                code_commit_sha, code_tree_sha, code_digest::text,
                runtime_image_digest::text, environment_digest::text,
                parameters_hash::text, seed::text, result_hash::text,
                result_payload, formal_evidence
         FROM analytics.formal_outputs
         WHERE tenant_id=$1 AND output_identity=$2
           AND owner_id::text = ANY($3::text[])",
    )
    .bind(scope.tenant_id().as_str())
    .bind(hash_hex(output_identity))
    .bind(owners)
    .fetch_optional(repository.pool())
    .await
    .map_err(map_sqlx_error)?;
    row.map(|row| decode_row(scope, row)).transpose()
}

fn decode_row(scope: &AccessScope, row: FormalOutputRow) -> ApplicationResult<FormalOutputRecord> {
    let proto = pb::FormalOutputEvidence::decode(row.formal_evidence.as_slice())
        .map_err(|_| integrity_error())?;
    let evidence = domain_evidence(proto)?;
    let canonical_evidence = proto_evidence(&evidence).encode_to_vec();
    if canonical_evidence != row.formal_evidence {
        return Err(integrity_error());
    }
    let subject = object_reference(evidence.subject())?;
    let subject_version = subject.version().ok_or_else(integrity_error)?;
    let subject_hash = subject.content_hash().ok_or_else(integrity_error)?;
    let row_seed = row
        .seed
        .as_deref()
        .map(str::parse::<u64>)
        .transpose()
        .map_err(|_| integrity_error())?;
    if row.output_identity != hash_hex(evidence.output_identity())
        || row.schema_id != evidence.schema_id()
        || row.subject_id != subject.object_id().as_str()
        || u64::try_from(row.subject_version).ok() != Some(subject_version.get())
        || row.subject_content_hash != hash_hex(subject_hash)
        || row.code_commit_sha != evidence.code().git_commit_sha()
        || row.code_tree_sha != evidence.code().git_tree_sha()
        || row.code_digest != hash_hex(evidence.code().digest())
        || row.runtime_image_digest != hash_hex(evidence.runtime().image_digest())
        || row.environment_digest != hash_hex(evidence.runtime().environment_digest())
        || row.parameters_hash != hash_hex(evidence.parameters_hash())
        || row_seed != evidence.seed()
        || row.result_hash != hash_hex(evidence.result_hash())
    {
        return Err(integrity_error());
    }
    let owner = OwnerRef::new(
        scope.tenant_id().clone(),
        Ulid::new(row.owner_id).map_err(map_domain_error)?,
    );
    let record = FormalOutputRecord::new(owner, evidence, row.result_payload)?;
    record.verify()?;
    Ok(record)
}

fn object_reference(binding: &FormalInputBinding) -> ApplicationResult<&LineageRef> {
    match binding.reference() {
        FormalInputReference::Object(reference) => Ok(reference),
        FormalInputReference::Named(_) => Err(integrity_error()),
    }
}

fn proto_evidence(value: &FormalOutputEvidence) -> pb::FormalOutputEvidence {
    pb::FormalOutputEvidence {
        schema_id: value.schema_id().to_owned(),
        subject: Some(proto_input(value.subject())),
        consumed_inputs: value.consumed_inputs().iter().map(proto_input).collect(),
        code: Some(pb::CodeBinding {
            git_commit_sha: value.code().git_commit_sha().to_owned(),
            git_tree_sha: value.code().git_tree_sha().to_owned(),
            digest: Some(proto_hash(value.code().digest())),
        }),
        runtime: Some(pb::RuntimeBinding {
            image_digest: Some(proto_hash(value.runtime().image_digest())),
            environment_digest: Some(proto_hash(value.runtime().environment_digest())),
        }),
        implementations: value
            .implementations()
            .iter()
            .map(|binding| pb::FormalImplementationBinding {
                role: binding.role().to_owned(),
                digest: Some(proto_hash(binding.digest())),
            })
            .collect(),
        parameters_hash: Some(proto_hash(value.parameters_hash())),
        seed: value.seed(),
        result_hash: Some(proto_hash(value.result_hash())),
        output_identity: Some(proto_hash(value.output_identity())),
    }
}

fn proto_input(value: &FormalInputBinding) -> pb::FormalInputBinding {
    use pb::formal_input_binding::Reference;
    let reference = match value.reference() {
        FormalInputReference::Object(reference) => Reference::ObjectRef(pb::LineageRef {
            object_id: Some(pb::Ulid {
                value: reference.object_id().as_str().to_owned(),
            }),
            version: reference.version().map_or(0, Version::get),
            content_hash: reference.content_hash().map(proto_hash),
        }),
        FormalInputReference::Named(reference) => Reference::NamedRef(pb::NamedContentRef {
            identity: reference.identity().to_owned(),
            content_hash: Some(proto_hash(reference.content_hash())),
        }),
    };
    pb::FormalInputBinding {
        role: value.role().to_owned(),
        kind: proto_kind(value.kind()) as i32,
        owner: Some(proto_owner(value.owner())),
        observed_at: value.observed_at().map(proto_time),
        visible_at: value.visible_at().map(proto_time),
        effective_from: value.effective_from().map(proto_time),
        effective_to: value.effective_to().map(proto_time),
        reference: Some(reference),
    }
}

pub(super) fn encode_formal_input(value: &FormalInputBinding) -> Vec<u8> {
    proto_input(value).encode_to_vec()
}

pub(super) fn decode_formal_input(bytes: &[u8]) -> ApplicationResult<FormalInputBinding> {
    let proto = pb::FormalInputBinding::decode(bytes).map_err(|_| integrity_error())?;
    if proto.encode_to_vec() != bytes {
        return Err(integrity_error());
    }
    domain_input(proto)
}

pub(super) fn encode_formal_evidence(value: &FormalOutputEvidence) -> Vec<u8> {
    proto_evidence(value).encode_to_vec()
}

pub(super) fn decode_formal_evidence(bytes: &[u8]) -> ApplicationResult<FormalOutputEvidence> {
    let proto = pb::FormalOutputEvidence::decode(bytes).map_err(|_| integrity_error())?;
    if proto.encode_to_vec() != bytes {
        return Err(integrity_error());
    }
    domain_evidence(proto)
}

fn domain_evidence(value: pb::FormalOutputEvidence) -> ApplicationResult<FormalOutputEvidence> {
    let code = value.code.ok_or_else(integrity_error)?;
    let runtime = value.runtime.ok_or_else(integrity_error)?;
    FormalOutputEvidence::from_claimed(
        FormalOutputEvidenceInput {
            schema_id: value.schema_id,
            subject: domain_input(value.subject.ok_or_else(integrity_error)?)?,
            consumed_inputs: value
                .consumed_inputs
                .into_iter()
                .map(domain_input)
                .collect::<ApplicationResult<Vec<_>>>()?,
            code: CodeBinding::from_claimed(
                code.git_commit_sha,
                code.git_tree_sha,
                domain_hash(code.digest)?,
            )
            .map_err(map_domain_error)?,
            runtime: RuntimeBinding::new(
                domain_hash(runtime.image_digest)?,
                domain_hash(runtime.environment_digest)?,
            ),
            implementations: value
                .implementations
                .into_iter()
                .map(|binding| {
                    FormalImplementationBinding::new(binding.role, domain_hash(binding.digest)?)
                        .map_err(map_domain_error)
                })
                .collect::<ApplicationResult<Vec<_>>>()?,
            parameters_hash: domain_hash(value.parameters_hash)?,
            seed: value.seed,
            result_hash: domain_hash(value.result_hash)?,
        },
        domain_hash(value.output_identity)?,
    )
    .map_err(map_domain_error)
}

fn domain_input(value: pb::FormalInputBinding) -> ApplicationResult<FormalInputBinding> {
    use pb::formal_input_binding::Reference;
    let reference = match value.reference.ok_or_else(integrity_error)? {
        Reference::ObjectRef(reference) => FormalInputReference::Object(
            LineageRef::new(
                Ulid::new(reference.object_id.ok_or_else(integrity_error)?.value)
                    .map_err(map_domain_error)?,
                (reference.version != 0)
                    .then(|| Version::new(reference.version).map_err(map_domain_error))
                    .transpose()?,
                reference
                    .content_hash
                    .map(|hash| ContentHash::from_bytes(&hash.value).map_err(map_domain_error))
                    .transpose()?,
            )
            .map_err(map_domain_error)?,
        ),
        Reference::NamedRef(reference) => FormalInputReference::Named(
            NamedContentRef::new(reference.identity, domain_hash(reference.content_hash)?)
                .map_err(map_domain_error)?,
        ),
    };
    FormalInputBinding::new(FormalInputBindingInput {
        role: value.role,
        kind: domain_kind(
            pb::FormalInputKind::try_from(value.kind).map_err(|_| integrity_error())?,
        )?,
        owner: domain_owner(value.owner)?,
        reference,
        observed_at: value.observed_at.map(domain_time).transpose()?,
        visible_at: value.visible_at.map(domain_time).transpose()?,
        effective_from: value.effective_from.map(domain_time).transpose()?,
        effective_to: value.effective_to.map(domain_time).transpose()?,
    })
    .map_err(map_domain_error)
}

const fn proto_kind(value: FormalInputKind) -> pb::FormalInputKind {
    match value {
        FormalInputKind::Subject => pb::FormalInputKind::Subject,
        FormalInputKind::DataSnapshot => pb::FormalInputKind::DataSnapshot,
        FormalInputKind::UniverseSnapshot => pb::FormalInputKind::UniverseSnapshot,
        FormalInputKind::RulePack => pb::FormalInputKind::RulePack,
        FormalInputKind::Artifact => pb::FormalInputKind::Artifact,
        FormalInputKind::Definition => pb::FormalInputKind::Definition,
        FormalInputKind::Instrument => pb::FormalInputKind::Instrument,
        FormalInputKind::Calendar => pb::FormalInputKind::Calendar,
        FormalInputKind::Unit => pb::FormalInputKind::Unit,
        FormalInputKind::DataSource => pb::FormalInputKind::DataSource,
        FormalInputKind::CurveSnapshot => pb::FormalInputKind::CurveSnapshot,
        FormalInputKind::FactorDefinition => pb::FormalInputKind::FactorDefinition,
        FormalInputKind::PositionSnapshot => pb::FormalInputKind::PositionSnapshot,
        FormalInputKind::DataHealthProfile => pb::FormalInputKind::DataHealthProfile,
        FormalInputKind::CurveNodeDefinition => pb::FormalInputKind::CurveNodeDefinition,
    }
}

fn domain_kind(value: pb::FormalInputKind) -> ApplicationResult<FormalInputKind> {
    Ok(match value {
        pb::FormalInputKind::Unspecified => return Err(integrity_error()),
        pb::FormalInputKind::Subject => FormalInputKind::Subject,
        pb::FormalInputKind::DataSnapshot => FormalInputKind::DataSnapshot,
        pb::FormalInputKind::UniverseSnapshot => FormalInputKind::UniverseSnapshot,
        pb::FormalInputKind::RulePack => FormalInputKind::RulePack,
        pb::FormalInputKind::Artifact => FormalInputKind::Artifact,
        pb::FormalInputKind::Definition => FormalInputKind::Definition,
        pb::FormalInputKind::Instrument => FormalInputKind::Instrument,
        pb::FormalInputKind::Calendar => FormalInputKind::Calendar,
        pb::FormalInputKind::Unit => FormalInputKind::Unit,
        pb::FormalInputKind::DataSource => FormalInputKind::DataSource,
        pb::FormalInputKind::CurveSnapshot => FormalInputKind::CurveSnapshot,
        pb::FormalInputKind::FactorDefinition => FormalInputKind::FactorDefinition,
        pb::FormalInputKind::PositionSnapshot => FormalInputKind::PositionSnapshot,
        pb::FormalInputKind::DataHealthProfile => FormalInputKind::DataHealthProfile,
        pb::FormalInputKind::CurveNodeDefinition => FormalInputKind::CurveNodeDefinition,
    })
}

fn proto_owner(value: &OwnerRef) -> pb::OwnerRef {
    pb::OwnerRef {
        tenant_id: Some(pb::Ulid {
            value: value.tenant_id().as_str().to_owned(),
        }),
        owner_id: Some(pb::Ulid {
            value: value.owner_id().as_str().to_owned(),
        }),
    }
}

fn domain_owner(value: Option<pb::OwnerRef>) -> ApplicationResult<OwnerRef> {
    let value = value.ok_or_else(integrity_error)?;
    Ok(OwnerRef::new(
        Ulid::new(value.tenant_id.ok_or_else(integrity_error)?.value).map_err(map_domain_error)?,
        Ulid::new(value.owner_id.ok_or_else(integrity_error)?.value).map_err(map_domain_error)?,
    ))
}

fn proto_hash(value: &ContentHash) -> pb::Sha256 {
    pb::Sha256 {
        value: value.as_bytes().to_vec(),
    }
}

fn domain_hash(value: Option<pb::Sha256>) -> ApplicationResult<ContentHash> {
    ContentHash::from_bytes(&value.ok_or_else(integrity_error)?.value).map_err(map_domain_error)
}

fn proto_time(value: &MarketTime) -> pb::MarketTime {
    let mut result = pb::MarketTime {
        instant: None,
        market_timezone: value.market_timezone().to_owned(),
        local_trading_date: value.local_trading_date().to_string(),
    };
    let instant = result.instant.get_or_insert_default();
    instant.seconds = value.instant().timestamp();
    instant.nanos = i32::try_from(value.instant().timestamp_subsec_nanos())
        .expect("nanoseconds always fit i32");
    result
}

fn domain_time(value: pb::MarketTime) -> ApplicationResult<MarketTime> {
    let instant = value.instant.ok_or_else(integrity_error)?;
    let nanos = u32::try_from(instant.nanos)
        .ok()
        .filter(|value| *value < 1_000_000_000)
        .ok_or_else(integrity_error)?;
    let instant = Utc
        .timestamp_opt(instant.seconds, nanos)
        .single()
        .ok_or_else(integrity_error)?;
    let local_date = NaiveDate::parse_from_str(&value.local_trading_date, "%Y-%m-%d")
        .map_err(|_| integrity_error())?;
    MarketTime::new(instant, value.market_timezone, local_date).map_err(map_domain_error)
}

fn hash_hex(hash: &ContentHash) -> String {
    crate::s3::content_addressed::hash_hex(hash)
}

fn validation_error() -> ApplicationError {
    application_error(ApplicationErrorCategory::ValidationFailed, false)
}

fn integrity_error() -> ApplicationError {
    application_error(ApplicationErrorCategory::HashMismatch, false)
}
