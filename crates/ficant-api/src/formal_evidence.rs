use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{NaiveDate, TimeZone, Utc};
use ficant_application::ports::{
    AccessScope, FormalOutputRecord, FormalOutputRepository, SubjectRepository,
    subject_record_content_hash,
};
use ficant_application::{
    ApplicationError, ApplicationErrorCategory, FormalOutputUseCase, RatesEvidenceBinding,
    RatesInputEvidence, RatesInputRole, RatesRequestEvidence, map_domain_error,
};
use ficant_contracts::ficant::core::v1 as pb;
use ficant_domain::primitives::{ContentHash, LineageRef, MarketTime, OwnerRef, Ulid, Version};
use ficant_runtime::{
    CodeBinding, FormalImplementationBinding, FormalInputBinding, FormalInputBindingInput,
    FormalInputKind, FormalInputReference, FormalOutputEvidence, FormalOutputEvidenceInput,
    NamedContentRef, RuntimeBinding,
};
use prost::Message;

#[derive(Clone)]
pub struct FormalOutputPublisher {
    repository: Arc<dyn FormalOutputRepository>,
    code: CodeBinding,
    runtime: RuntimeBinding,
}

pub(crate) async fn exact_subject_binding(
    subjects: &dyn SubjectRepository,
    scope: &AccessScope,
    owner: &OwnerRef,
    reference: &ficant_domain::primitives::VersionRef,
) -> Result<FormalInputBinding, ApplicationError> {
    let record = subjects
        .get_subject_scoped(scope, reference.clone())
        .await?
        .ok_or_else(not_found)?;
    if record.version().reference() != reference || record.subject().owner() != Some(owner) {
        return Err(lineage_incomplete());
    }
    object_binding(
        "subject",
        FormalInputKind::Subject,
        owner,
        reference.id(),
        Some(reference.version()),
        subject_record_content_hash(&record)?,
        FormalInputTimes::default(),
    )
}

#[derive(Clone, Debug, Default)]
pub(crate) struct FormalInputTimes {
    pub observed_at: Option<MarketTime>,
    pub visible_at: Option<MarketTime>,
    pub effective_from: Option<MarketTime>,
    pub effective_to: Option<MarketTime>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn object_binding(
    role: impl Into<String>,
    kind: FormalInputKind,
    owner: &OwnerRef,
    object_id: &Ulid,
    version: Option<Version>,
    content_hash: ContentHash,
    times: FormalInputTimes,
) -> Result<FormalInputBinding, ApplicationError> {
    let reference = LineageRef::new(object_id.clone(), version, Some(content_hash))
        .map_err(map_domain_error)?;
    FormalInputBinding::new(FormalInputBindingInput {
        role: role.into(),
        kind,
        owner: owner.clone(),
        reference: FormalInputReference::Object(reference),
        observed_at: times.observed_at,
        visible_at: times.visible_at,
        effective_from: times.effective_from,
        effective_to: times.effective_to,
    })
    .map_err(map_domain_error)
}

pub(crate) fn message_parameters_hash<M: Message>(domain: &str, value: &M) -> ContentHash {
    domain_separated_hash(domain, &[&value.encode_to_vec()])
}

pub(crate) fn implementation_binding(
    role: impl Into<String>,
    domain: &str,
    parts: &[&[u8]],
) -> Result<FormalImplementationBinding, ApplicationError> {
    FormalImplementationBinding::new(role, domain_separated_hash(domain, parts))
        .map_err(map_domain_error)
}

pub(crate) fn domain_separated_hash(domain: &str, parts: &[&[u8]]) -> ContentHash {
    let mut bytes = Vec::new();
    append_canonical(&mut bytes, domain.as_bytes());
    for part in parts {
        append_canonical(&mut bytes, part);
    }
    ContentHash::digest(&bytes)
}

fn append_canonical(output: &mut Vec<u8>, value: &[u8]) {
    output.extend_from_slice(&(value.len() as u64).to_be_bytes());
    output.extend_from_slice(value);
}

impl FormalOutputPublisher {
    #[must_use]
    pub fn new(
        repository: Arc<dyn FormalOutputRepository>,
        code: CodeBinding,
        runtime: RuntimeBinding,
    ) -> Self {
        Self {
            repository,
            code,
            runtime,
        }
    }

    #[must_use]
    pub fn code(&self) -> &CodeBinding {
        &self.code
    }

    #[must_use]
    pub fn runtime(&self) -> &RuntimeBinding {
        &self.runtime
    }

    /// Persists the canonical result bytes and their formal evidence before the caller exposes a
    /// successful transport response.
    ///
    /// The supplied message must be the result with its `formal_evidence` field still absent, so
    /// the result hash cannot recursively depend on the evidence that binds it.
    ///
    /// # Errors
    ///
    /// Returns validation, identity, authorization, idempotency, or storage failures. A repository
    /// failure is never converted to success.
    #[allow(clippy::too_many_arguments)]
    pub async fn publish_message<M: Message>(
        &self,
        scope: &AccessScope,
        owner: &OwnerRef,
        schema_id: impl Into<String>,
        subject: FormalInputBinding,
        consumed_inputs: Vec<FormalInputBinding>,
        implementations: Vec<FormalImplementationBinding>,
        parameters_hash: ContentHash,
        seed: Option<u64>,
        message: &M,
    ) -> Result<pb::FormalOutputEvidence, ApplicationError> {
        let payload = message.encode_to_vec();
        let result_hash = ContentHash::digest(&payload);
        let evidence = FormalOutputEvidence::new(FormalOutputEvidenceInput {
            schema_id: schema_id.into(),
            subject,
            consumed_inputs,
            code: self.code.clone(),
            runtime: self.runtime.clone(),
            implementations,
            parameters_hash,
            seed,
            result_hash,
        })
        .map_err(map_domain_error)?;
        let record = FormalOutputRecord::new(owner.clone(), evidence, payload)?;
        let stored = FormalOutputUseCase::new(self.repository.as_ref())
            .publish(scope, record)
            .await?;
        Ok(proto_formal_evidence(stored.evidence()))
    }
}

pub(crate) fn rates_formal_inputs(
    evidence: &RatesRequestEvidence,
) -> Result<(FormalInputBinding, Vec<FormalInputBinding>), ApplicationError> {
    let mut role_counts = BTreeMap::new();
    for input in evidence.consumed_inputs() {
        *role_counts.entry(input.role()).or_insert(0_usize) += 1;
    }
    let mut role_ordinals = BTreeMap::new();
    let mut subject = None;
    let mut consumed = Vec::new();
    for input in evidence.consumed_inputs() {
        let base_role = rates_role(input.role());
        let ordinal = role_ordinals.entry(input.role()).or_insert(0_usize);
        *ordinal += 1;
        let role = if role_counts[&input.role()] == 1 {
            base_role.to_owned()
        } else {
            format!("{base_role}.{:03}", *ordinal)
        };
        let binding = formal_input(input, role)?;
        if input.role() == RatesInputRole::Subject {
            if subject.replace(binding).is_some() {
                return Err(lineage_incomplete());
            }
        } else {
            consumed.push(binding);
        }
    }
    Ok((subject.ok_or_else(lineage_incomplete)?, consumed))
}

fn formal_input(
    value: &RatesInputEvidence,
    role: String,
) -> Result<FormalInputBinding, ApplicationError> {
    let (kind, reference) = match value.binding() {
        RatesEvidenceBinding::Object(reference) => {
            let kind = match value.role() {
                RatesInputRole::Subject => FormalInputKind::Subject,
                RatesInputRole::Unit => FormalInputKind::Unit,
                RatesInputRole::Bond | RatesInputRole::FuturesContract => {
                    FormalInputKind::Instrument
                }
                RatesInputRole::Calendar => FormalInputKind::Calendar,
                RatesInputRole::DataSource => FormalInputKind::DataSource,
                RatesInputRole::TaxRulePack
                | RatesInputRole::FundingRulePack
                | RatesInputRole::DeliveryRulePack
                | RatesInputRole::CurveRulePack => FormalInputKind::RulePack,
                _ => return Err(lineage_incomplete()),
            };
            let exact = LineageRef::new(
                reference.version_ref().id().clone(),
                Some(reference.version_ref().version()),
                Some(reference.content_hash().clone()),
            )
            .map_err(map_domain_error)?;
            (kind, FormalInputReference::Object(exact))
        }
        RatesEvidenceBinding::Snapshot(reference) => {
            let kind = match value.role() {
                RatesInputRole::CurveSnapshot => FormalInputKind::CurveSnapshot,
                RatesInputRole::DataSnapshot => FormalInputKind::DataSnapshot,
                _ => return Err(lineage_incomplete()),
            };
            let exact = LineageRef::new(
                reference.id().clone(),
                None,
                Some(reference.content_hash().clone()),
            )
            .map_err(map_domain_error)?;
            (kind, FormalInputReference::Object(exact))
        }
        RatesEvidenceBinding::Artifact(reference) => {
            let exact = LineageRef::new(
                reference.id().clone(),
                None,
                Some(reference.content_hash().clone()),
            )
            .map_err(map_domain_error)?;
            (
                FormalInputKind::Artifact,
                FormalInputReference::Object(exact),
            )
        }
        RatesEvidenceBinding::CurveNode(reference) => (
            FormalInputKind::CurveNodeDefinition,
            FormalInputReference::Named(
                NamedContentRef::new(reference.curve_node_id(), reference.content_hash().clone())
                    .map_err(map_domain_error)?,
            ),
        ),
    };
    FormalInputBinding::new(FormalInputBindingInput {
        role,
        kind,
        owner: value.owner().clone(),
        reference,
        observed_at: value.observed_at().cloned(),
        visible_at: value.visible_at().cloned(),
        effective_from: value.effective_from().cloned(),
        effective_to: value.effective_to().cloned(),
    })
    .map_err(map_domain_error)
}

const fn rates_role(value: RatesInputRole) -> &'static str {
    match value {
        RatesInputRole::Subject => "subject",
        RatesInputRole::Unit => "unit",
        RatesInputRole::Bond => "bond",
        RatesInputRole::Calendar => "calendar",
        RatesInputRole::CurveSnapshot => "curve-snapshot",
        RatesInputRole::DataSnapshot => "data-snapshot",
        RatesInputRole::DataSource => "data-source",
        RatesInputRole::TaxRulePack => "tax-rule-pack",
        RatesInputRole::FundingRulePack => "funding-rule-pack",
        RatesInputRole::DeliveryRulePack => "delivery-rule-pack",
        RatesInputRole::FuturesContract => "futures-contract",
        RatesInputRole::TargetRiskArtifact => "target-risk-artifact",
        RatesInputRole::DeliveryArtifact => "delivery-artifact",
        RatesInputRole::CtdAnalyticsArtifact => "ctd-analytics-artifact",
        RatesInputRole::CurveRulePack => "curve-rule-pack",
        RatesInputRole::CurveNodeDefinition => "curve-node-definition",
    }
}

pub(crate) fn proto_formal_evidence(value: &FormalOutputEvidence) -> pb::FormalOutputEvidence {
    pb::FormalOutputEvidence {
        schema_id: value.schema_id().to_owned(),
        subject: Some(proto_formal_input(value.subject())),
        consumed_inputs: value
            .consumed_inputs()
            .iter()
            .map(proto_formal_input)
            .collect(),
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

pub(crate) fn proto_formal_input(value: &FormalInputBinding) -> pb::FormalInputBinding {
    use pb::formal_input_binding::Reference;
    let reference = match value.reference() {
        FormalInputReference::Object(reference) => Reference::ObjectRef(pb::LineageRef {
            object_id: Some(pb::Ulid {
                value: reference.object_id().as_str().to_owned(),
            }),
            version: reference
                .version()
                .map_or(0, ficant_domain::primitives::Version::get),
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
        owner: Some(pb::OwnerRef {
            tenant_id: Some(pb::Ulid {
                value: value.owner().tenant_id().as_str().to_owned(),
            }),
            owner_id: Some(pb::Ulid {
                value: value.owner().owner_id().as_str().to_owned(),
            }),
        }),
        observed_at: value.observed_at().map(proto_time),
        visible_at: value.visible_at().map(proto_time),
        effective_from: value.effective_from().map(proto_time),
        effective_to: value.effective_to().map(proto_time),
        reference: Some(reference),
    }
}

pub(crate) fn proto_code_binding(value: &CodeBinding) -> pb::CodeBinding {
    pb::CodeBinding {
        git_commit_sha: value.git_commit_sha().to_owned(),
        git_tree_sha: value.git_tree_sha().to_owned(),
        digest: Some(proto_hash(value.digest())),
    }
}

pub(crate) fn parse_formal_input(
    value: pb::FormalInputBinding,
) -> Result<FormalInputBinding, ApplicationError> {
    use pb::formal_input_binding::Reference;
    let reference = match value.reference.ok_or_else(lineage_incomplete)? {
        Reference::ObjectRef(reference) => FormalInputReference::Object(
            LineageRef::new(
                Ulid::new(reference.object_id.ok_or_else(lineage_incomplete)?.value)
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
            NamedContentRef::new(reference.identity, parse_hash(reference.content_hash)?)
                .map_err(map_domain_error)?,
        ),
    };
    let owner = value.owner.ok_or_else(lineage_incomplete)?;
    let owner = OwnerRef::new(
        Ulid::new(owner.tenant_id.ok_or_else(lineage_incomplete)?.value)
            .map_err(map_domain_error)?,
        Ulid::new(owner.owner_id.ok_or_else(lineage_incomplete)?.value)
            .map_err(map_domain_error)?,
    );
    FormalInputBinding::new(FormalInputBindingInput {
        role: value.role,
        kind: parse_kind(value.kind)?,
        owner,
        reference,
        observed_at: value.observed_at.map(parse_time).transpose()?,
        visible_at: value.visible_at.map(parse_time).transpose()?,
        effective_from: value.effective_from.map(parse_time).transpose()?,
        effective_to: value.effective_to.map(parse_time).transpose()?,
    })
    .map_err(map_domain_error)
}

fn parse_kind(value: i32) -> Result<FormalInputKind, ApplicationError> {
    let value = pb::FormalInputKind::try_from(value).map_err(|_| lineage_incomplete())?;
    Ok(match value {
        pb::FormalInputKind::Unspecified => return Err(lineage_incomplete()),
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

fn parse_hash(value: Option<pb::Sha256>) -> Result<ContentHash, ApplicationError> {
    ContentHash::from_bytes(&value.ok_or_else(lineage_incomplete)?.value).map_err(map_domain_error)
}

fn parse_time(value: pb::MarketTime) -> Result<MarketTime, ApplicationError> {
    let instant = value.instant.ok_or_else(lineage_incomplete)?;
    let nanos = u32::try_from(instant.nanos)
        .ok()
        .filter(|nanos| *nanos < 1_000_000_000)
        .ok_or_else(lineage_incomplete)?;
    let instant = Utc
        .timestamp_opt(instant.seconds, nanos)
        .single()
        .ok_or_else(lineage_incomplete)?;
    let local_date = NaiveDate::parse_from_str(&value.local_trading_date, "%Y-%m-%d")
        .map_err(|_| lineage_incomplete())?;
    MarketTime::new(instant, value.market_timezone, local_date).map_err(map_domain_error)
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

fn proto_hash(value: &ContentHash) -> pb::Sha256 {
    pb::Sha256 {
        value: value.as_bytes().to_vec(),
    }
}

fn proto_time(value: &ficant_domain::primitives::MarketTime) -> pb::MarketTime {
    let mut result = pb::MarketTime {
        instant: Some(prost_types::Timestamp::default()),
        market_timezone: value.market_timezone().to_owned(),
        local_trading_date: value.local_trading_date().to_string(),
    };
    let instant = result
        .instant
        .as_mut()
        .expect("formal MarketTime encoder creates timestamp");
    instant.seconds = value.instant().timestamp();
    instant.nanos = i32::try_from(value.instant().timestamp_subsec_nanos())
        .expect("nanoseconds always fit i32");
    result
}

fn lineage_incomplete() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::LineageIncomplete, false)
}

fn not_found() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::NotFound, false)
}
