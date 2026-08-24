use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use ficant_domain::primitives::{ContentHash, LineageRef, MarketTime, OwnerRef, Ulid, Version};
use ficant_domain::research::{ResearchGraph, ResearchNode, TypedValue};
use ficant_domain::{DomainErrorCode, DomainResult};

use crate::RuntimeError;

const FORMAL_EVIDENCE_DOMAIN: &str = "ficant/formal-output-evidence/v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FormalInputKind {
    Subject,
    DataSnapshot,
    UniverseSnapshot,
    RulePack,
    Artifact,
    Definition,
    Instrument,
    Calendar,
    Unit,
    DataSource,
    CurveSnapshot,
    FactorDefinition,
    PositionSnapshot,
    DataHealthProfile,
    CurveNodeDefinition,
    Portfolio,
    Book,
    PortfolioGroup,
    Benchmark,
    PortfolioMetricConvention,
    Fact,
    PortfolioValuationSnapshot,
    BenchmarkLevelSnapshot,
    PortfolioPerformanceConvention,
}

impl FormalInputKind {
    const fn code(self) -> u8 {
        match self {
            Self::Subject => 1,
            Self::DataSnapshot => 2,
            Self::UniverseSnapshot => 3,
            Self::RulePack => 4,
            Self::Artifact => 5,
            Self::Definition => 6,
            Self::Instrument => 7,
            Self::Calendar => 8,
            Self::Unit => 9,
            Self::DataSource => 10,
            Self::CurveSnapshot => 11,
            Self::FactorDefinition => 12,
            Self::PositionSnapshot => 13,
            Self::DataHealthProfile => 14,
            Self::CurveNodeDefinition => 15,
            Self::Portfolio => 16,
            Self::Book => 17,
            Self::PortfolioGroup => 18,
            Self::Benchmark => 19,
            Self::PortfolioMetricConvention => 20,
            Self::Fact => 21,
            Self::PortfolioValuationSnapshot => 22,
            Self::BenchmarkLevelSnapshot => 23,
            Self::PortfolioPerformanceConvention => 24,
        }
    }

    const fn uses_named_reference(self) -> bool {
        matches!(self, Self::FactorDefinition | Self::CurveNodeDefinition)
    }

    const fn uses_versioned_object(self) -> bool {
        matches!(
            self,
            Self::Subject
                | Self::RulePack
                | Self::Definition
                | Self::Instrument
                | Self::Calendar
                | Self::Unit
                | Self::DataSource
                | Self::DataHealthProfile
                | Self::Portfolio
                | Self::Book
                | Self::PortfolioGroup
                | Self::Benchmark
                | Self::PortfolioMetricConvention
                | Self::PortfolioPerformanceConvention
                | Self::Fact
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamedContentRef {
    identity: String,
    content_hash: ContentHash,
}

impl NamedContentRef {
    /// Creates a stable named reference with exact content identity.
    ///
    /// # Errors
    ///
    /// Returns `InvalidValue` when the identity is not canonical ASCII.
    pub fn new(identity: impl Into<String>, content_hash: ContentHash) -> DomainResult<Self> {
        let identity = identity.into();
        ensure_stable_identity(&identity)?;
        Ok(Self {
            identity,
            content_hash,
        })
    }

    #[must_use]
    pub fn identity(&self) -> &str {
        &self.identity
    }

    #[must_use]
    pub fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormalInputReference {
    Object(LineageRef),
    Named(NamedContentRef),
}

impl FormalInputReference {
    fn canonical_bytes(&self) -> Vec<u8> {
        let mut writer = CanonicalWriter::new("formal-input-reference/v1");
        match self {
            Self::Object(reference) => {
                writer.field(2, &[1]);
                writer.field(3, &object_reference_bytes(reference));
            }
            Self::Named(reference) => {
                writer.field(2, &[2]);
                writer.field(3, reference.identity.as_bytes());
                writer.field(4, reference.content_hash.as_bytes());
            }
        }
        writer.finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormalInputBindingInput {
    pub role: String,
    pub kind: FormalInputKind,
    pub owner: OwnerRef,
    pub reference: FormalInputReference,
    pub observed_at: Option<MarketTime>,
    pub visible_at: Option<MarketTime>,
    pub effective_from: Option<MarketTime>,
    pub effective_to: Option<MarketTime>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormalInputBinding {
    role: String,
    kind: FormalInputKind,
    owner: OwnerRef,
    reference: FormalInputReference,
    observed_at: Option<MarketTime>,
    visible_at: Option<MarketTime>,
    effective_from: Option<MarketTime>,
    effective_to: Option<MarketTime>,
}

impl FormalInputBinding {
    /// Creates one typed, role-bearing exact input binding.
    ///
    /// # Errors
    ///
    /// Returns a domain error for invalid roles, reference shapes, or time intervals.
    pub fn new(input: FormalInputBindingInput) -> DomainResult<Self> {
        ensure_role(&input.role)?;
        validate_reference(input.kind, &input.reference)?;
        validate_times(
            input.observed_at.as_ref(),
            input.visible_at.as_ref(),
            input.effective_from.as_ref(),
            input.effective_to.as_ref(),
        )?;
        Ok(Self {
            role: input.role,
            kind: input.kind,
            owner: input.owner,
            reference: input.reference,
            observed_at: input.observed_at,
            visible_at: input.visible_at,
            effective_from: input.effective_from,
            effective_to: input.effective_to,
        })
    }

    #[must_use]
    pub fn role(&self) -> &str {
        &self.role
    }

    #[must_use]
    pub const fn kind(&self) -> FormalInputKind {
        self.kind
    }

    #[must_use]
    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    #[must_use]
    pub fn reference(&self) -> &FormalInputReference {
        &self.reference
    }

    #[must_use]
    pub fn observed_at(&self) -> Option<&MarketTime> {
        self.observed_at.as_ref()
    }

    #[must_use]
    pub fn visible_at(&self) -> Option<&MarketTime> {
        self.visible_at.as_ref()
    }

    #[must_use]
    pub fn effective_from(&self) -> Option<&MarketTime> {
        self.effective_from.as_ref()
    }

    #[must_use]
    pub fn effective_to(&self) -> Option<&MarketTime> {
        self.effective_to.as_ref()
    }

    #[must_use]
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut writer = CanonicalWriter::new("formal-input-binding/v1");
        writer.field(2, self.role.as_bytes());
        writer.field(3, &[self.kind.code()]);
        writer.field(4, &owner_bytes(&self.owner));
        writer.field(5, &self.reference.canonical_bytes());
        writer.optional(6, self.observed_at.as_ref().map(market_time_bytes));
        writer.optional(7, self.visible_at.as_ref().map(market_time_bytes));
        writer.optional(8, self.effective_from.as_ref().map(market_time_bytes));
        writer.optional(9, self.effective_to.as_ref().map(market_time_bytes));
        writer.finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeBinding {
    git_commit_sha: String,
    git_tree_sha: String,
    digest: ContentHash,
}

impl CodeBinding {
    /// Creates a public source-code binding from exact Git commit and tree SHAs.
    ///
    /// # Errors
    ///
    /// Returns `InvalidValue` unless both values are canonical 40-character lowercase SHAs.
    pub fn new(
        git_commit_sha: impl Into<String>,
        git_tree_sha: impl Into<String>,
    ) -> DomainResult<Self> {
        let git_commit_sha = git_commit_sha.into();
        let git_tree_sha = git_tree_sha.into();
        ensure_git_sha(&git_commit_sha)?;
        ensure_git_sha(&git_tree_sha)?;
        let digest = code_digest(&git_commit_sha, &git_tree_sha);
        Ok(Self {
            git_commit_sha,
            git_tree_sha,
            digest,
        })
    }

    /// Reconstructs a code binding and checks a claimed digest.
    ///
    /// # Errors
    ///
    /// Returns a domain error for invalid SHAs or a mismatched claimed digest.
    #[allow(clippy::needless_pass_by_value)]
    pub fn from_claimed(
        git_commit_sha: impl Into<String>,
        git_tree_sha: impl Into<String>,
        claimed_digest: ContentHash,
    ) -> DomainResult<Self> {
        let value = Self::new(git_commit_sha, git_tree_sha)?;
        if value.digest != claimed_digest {
            return Err(DomainErrorCode::ContentHashMismatch);
        }
        Ok(value)
    }

    #[must_use]
    pub fn git_commit_sha(&self) -> &str {
        &self.git_commit_sha
    }

    #[must_use]
    pub fn git_tree_sha(&self) -> &str {
        &self.git_tree_sha
    }

    #[must_use]
    pub fn digest(&self) -> &ContentHash {
        &self.digest
    }

    #[must_use]
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut writer = CanonicalWriter::new("code-binding/v1");
        writer.field(2, self.git_commit_sha.as_bytes());
        writer.field(3, self.git_tree_sha.as_bytes());
        writer.field(4, self.digest.as_bytes());
        writer.finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeBinding {
    image_digest: ContentHash,
    environment_digest: ContentHash,
}

impl RuntimeBinding {
    #[must_use]
    pub const fn new(image_digest: ContentHash, environment_digest: ContentHash) -> Self {
        Self {
            image_digest,
            environment_digest,
        }
    }

    #[must_use]
    pub fn image_digest(&self) -> &ContentHash {
        &self.image_digest
    }

    #[must_use]
    pub fn environment_digest(&self) -> &ContentHash {
        &self.environment_digest
    }

    fn canonical_bytes(&self) -> Vec<u8> {
        let mut writer = CanonicalWriter::new("runtime-binding/v1");
        writer.field(2, self.image_digest.as_bytes());
        writer.field(3, self.environment_digest.as_bytes());
        writer.finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormalImplementationBinding {
    role: String,
    digest: ContentHash,
}

impl FormalImplementationBinding {
    /// Creates a stable role-to-implementation digest binding.
    ///
    /// # Errors
    ///
    /// Returns `InvalidValue` when the role is not canonical.
    pub fn new(role: impl Into<String>, digest: ContentHash) -> DomainResult<Self> {
        let role = role.into();
        ensure_role(&role)?;
        Ok(Self { role, digest })
    }

    #[must_use]
    pub fn role(&self) -> &str {
        &self.role
    }

    #[must_use]
    pub fn digest(&self) -> &ContentHash {
        &self.digest
    }

    fn canonical_bytes(&self) -> Vec<u8> {
        let mut writer = CanonicalWriter::new("formal-implementation-binding/v1");
        writer.field(2, self.role.as_bytes());
        writer.field(3, self.digest.as_bytes());
        writer.finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormalOutputEvidenceInput {
    pub schema_id: String,
    pub subject: FormalInputBinding,
    pub consumed_inputs: Vec<FormalInputBinding>,
    pub code: CodeBinding,
    pub runtime: RuntimeBinding,
    pub implementations: Vec<FormalImplementationBinding>,
    pub parameters_hash: ContentHash,
    pub seed: Option<u64>,
    pub result_hash: ContentHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormalOutputEvidence {
    schema_id: String,
    subject: FormalInputBinding,
    consumed_inputs: Vec<FormalInputBinding>,
    code: CodeBinding,
    runtime: RuntimeBinding,
    implementations: Vec<FormalImplementationBinding>,
    parameters_hash: ContentHash,
    seed: Option<u64>,
    result_hash: ContentHash,
    output_identity: ContentHash,
}

impl FormalOutputEvidence {
    /// Creates canonical evidence and derives its stable output identity.
    ///
    /// # Errors
    ///
    /// Returns a domain error for invalid schema, Subject, duplicate roles, or implementations.
    pub fn new(mut input: FormalOutputEvidenceInput) -> DomainResult<Self> {
        ensure_stable_identity(&input.schema_id)?;
        if input.subject.kind != FormalInputKind::Subject || input.subject.role != "subject" {
            return Err(DomainErrorCode::BrokenLineage);
        }

        input.consumed_inputs.sort_by(compare_inputs);
        if input
            .consumed_inputs
            .windows(2)
            .any(|pair| pair[0].role == pair[1].role)
            || input
                .consumed_inputs
                .iter()
                .any(|binding| binding.role == "subject")
        {
            return Err(DomainErrorCode::BrokenLineage);
        }
        input
            .implementations
            .sort_by(|left, right| left.role.cmp(&right.role));
        if input
            .implementations
            .windows(2)
            .any(|pair| pair[0].role == pair[1].role)
        {
            return Err(DomainErrorCode::InvalidValue);
        }

        let mut result = Self {
            schema_id: input.schema_id,
            subject: input.subject,
            consumed_inputs: input.consumed_inputs,
            code: input.code,
            runtime: input.runtime,
            implementations: input.implementations,
            parameters_hash: input.parameters_hash,
            seed: input.seed,
            result_hash: input.result_hash,
            output_identity: ContentHash::digest(b"uninitialized"),
        };
        result.output_identity = ContentHash::digest(&result.identity_bytes());
        Ok(result)
    }

    /// Reconstructs canonical evidence and checks its claimed output identity.
    ///
    /// # Errors
    ///
    /// Returns a domain error for invalid evidence or a mismatched identity.
    #[allow(clippy::needless_pass_by_value)]
    pub fn from_claimed(
        input: FormalOutputEvidenceInput,
        claimed_identity: ContentHash,
    ) -> DomainResult<Self> {
        let value = Self::new(input)?;
        if value.output_identity != claimed_identity {
            return Err(DomainErrorCode::ContentHashMismatch);
        }
        Ok(value)
    }

    #[must_use]
    pub fn schema_id(&self) -> &str {
        &self.schema_id
    }

    #[must_use]
    pub fn subject(&self) -> &FormalInputBinding {
        &self.subject
    }

    #[must_use]
    pub fn consumed_inputs(&self) -> &[FormalInputBinding] {
        &self.consumed_inputs
    }

    #[must_use]
    pub fn code(&self) -> &CodeBinding {
        &self.code
    }

    #[must_use]
    pub fn runtime(&self) -> &RuntimeBinding {
        &self.runtime
    }

    #[must_use]
    pub fn implementations(&self) -> &[FormalImplementationBinding] {
        &self.implementations
    }

    #[must_use]
    pub fn parameters_hash(&self) -> &ContentHash {
        &self.parameters_hash
    }

    #[must_use]
    pub const fn seed(&self) -> Option<u64> {
        self.seed
    }

    #[must_use]
    pub fn result_hash(&self) -> &ContentHash {
        &self.result_hash
    }

    #[must_use]
    pub fn output_identity(&self) -> &ContentHash {
        &self.output_identity
    }

    #[must_use]
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut writer = CanonicalWriter::new("formal-output-evidence-record/v1");
        writer.field(2, &self.identity_bytes());
        writer.field(3, self.output_identity.as_bytes());
        writer.finish()
    }

    fn identity_bytes(&self) -> Vec<u8> {
        let mut writer = CanonicalWriter::new(FORMAL_EVIDENCE_DOMAIN);
        writer.field(2, self.schema_id.as_bytes());
        writer.field(3, &self.subject.canonical_bytes());
        writer.u64(4, self.consumed_inputs.len() as u64);
        for input in &self.consumed_inputs {
            writer.field(5, &input.canonical_bytes());
        }
        writer.field(6, &self.code.canonical_bytes());
        writer.field(7, &self.runtime.canonical_bytes());
        writer.u64(8, self.implementations.len() as u64);
        for implementation in &self.implementations {
            writer.field(9, &implementation.canonical_bytes());
        }
        writer.field(10, self.parameters_hash.as_bytes());
        writer.optional_u64(11, self.seed);
        writer.field(12, self.result_hash.as_bytes());
        writer.finish()
    }
}

fn validate_reference(kind: FormalInputKind, reference: &FormalInputReference) -> DomainResult<()> {
    match (kind.uses_named_reference(), reference) {
        (true, FormalInputReference::Named(_)) => Ok(()),
        (true, FormalInputReference::Object(_)) | (false, FormalInputReference::Named(_)) => {
            Err(DomainErrorCode::BrokenLineage)
        }
        (false, FormalInputReference::Object(reference)) => {
            if reference.content_hash().is_none()
                || reference.version().is_some() != kind.uses_versioned_object()
            {
                return Err(DomainErrorCode::BrokenLineage);
            }
            Ok(())
        }
    }
}

fn validate_times(
    observed_at: Option<&MarketTime>,
    visible_at: Option<&MarketTime>,
    effective_from: Option<&MarketTime>,
    effective_to: Option<&MarketTime>,
) -> DomainResult<()> {
    if observed_at
        .zip(visible_at)
        .is_some_and(|(observed, visible)| observed.instant() > visible.instant())
        || effective_from.is_some() != effective_to.is_some()
        || effective_from
            .zip(effective_to)
            .is_some_and(|(from, to)| from.instant() >= to.instant())
    {
        return Err(DomainErrorCode::InvalidEffectiveTime);
    }
    Ok(())
}

fn compare_inputs(left: &FormalInputBinding, right: &FormalInputBinding) -> Ordering {
    left.role
        .cmp(&right.role)
        .then_with(|| left.canonical_bytes().cmp(&right.canonical_bytes()))
}

fn code_digest(commit: &str, tree: &str) -> ContentHash {
    let mut writer = CanonicalWriter::new("code-binding-digest/v1");
    writer.field(2, commit.as_bytes());
    writer.field(3, tree.as_bytes());
    ContentHash::digest(&writer.finish())
}

fn object_reference_bytes(reference: &LineageRef) -> Vec<u8> {
    let mut writer = CanonicalWriter::new("exact-lineage-reference/v1");
    writer.field(2, reference.object_id().as_str().as_bytes());
    writer.optional_u64(3, reference.version().map(Version::get));
    writer.optional(
        4,
        reference
            .content_hash()
            .map(|hash| hash.as_bytes().to_vec()),
    );
    writer.finish()
}

fn owner_bytes(owner: &OwnerRef) -> Vec<u8> {
    let mut writer = CanonicalWriter::new("owner-ref/v1");
    writer.field(2, owner.tenant_id().as_str().as_bytes());
    writer.field(3, owner.owner_id().as_str().as_bytes());
    writer.finish()
}

fn market_time_bytes(time: &MarketTime) -> Vec<u8> {
    let mut writer = CanonicalWriter::new("market-time/v1");
    writer.field(2, &time.instant().timestamp().to_be_bytes());
    writer.field(3, &time.instant().timestamp_subsec_nanos().to_be_bytes());
    writer.field(4, time.market_timezone().as_bytes());
    writer.field(5, time.local_trading_date().to_string().as_bytes());
    writer.finish()
}

fn ensure_git_sha(value: &str) -> DomainResult<()> {
    if value.len() != 40
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(DomainErrorCode::InvalidValue);
    }
    Ok(())
}

fn ensure_role(value: &str) -> DomainResult<()> {
    if value.is_empty()
        || value.len() > 128
        || value.trim() != value
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
    {
        return Err(DomainErrorCode::InvalidValue);
    }
    Ok(())
}

fn ensure_stable_identity(value: &str) -> DomainResult<()> {
    if value.is_empty()
        || value.len() > 256
        || value.trim() != value
        || !value.is_ascii()
        || value.bytes().any(|byte| byte.is_ascii_whitespace())
    {
        return Err(DomainErrorCode::InvalidValue);
    }
    Ok(())
}

struct CanonicalWriter {
    bytes: Vec<u8>,
}

impl CanonicalWriter {
    fn new(domain: &str) -> Self {
        let mut value = Self {
            bytes: b"FICANT-EVIDENCE\0".to_vec(),
        };
        value.field(1, domain.as_bytes());
        value
    }

    fn field(&mut self, tag: u16, value: &[u8]) {
        self.bytes.extend_from_slice(&tag.to_be_bytes());
        self.bytes
            .extend_from_slice(&(value.len() as u64).to_be_bytes());
        self.bytes.extend_from_slice(value);
    }

    fn u64(&mut self, tag: u16, value: u64) {
        self.field(tag, &value.to_be_bytes());
    }

    fn optional_u64(&mut self, tag: u16, value: Option<u64>) {
        let mut bytes = Vec::with_capacity(9);
        match value {
            Some(value) => {
                bytes.push(1);
                bytes.extend_from_slice(&value.to_be_bytes());
            }
            None => bytes.push(0),
        }
        self.field(tag, &bytes);
    }

    fn optional(&mut self, tag: u16, value: Option<Vec<u8>>) {
        let mut bytes = Vec::new();
        match value {
            Some(value) => {
                bytes.push(1);
                bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
                bytes.extend_from_slice(&value);
            }
            None => bytes.push(0),
        }
        self.field(tag, &bytes);
    }

    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeImplementation {
    pub node_id: Ulid,
    pub implementation_digest: ContentHash,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RulePackBinding {
    pub rule_pack_id: String,
    pub version: Version,
    pub content_hash: ContentHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionExternalInput {
    input_id: String,
    value_type: TypedValue,
    payload: Vec<u8>,
    content_hash: ContentHash,
}

impl ExecutionExternalInput {
    /// Binds a typed external payload to one declared graph input.
    ///
    /// # Errors
    ///
    /// Returns `InvalidValue` for an empty/padded identifier or empty payload.
    pub fn new(
        input_id: impl Into<String>,
        value_type: TypedValue,
        payload: Vec<u8>,
    ) -> Result<Self, RuntimeError> {
        let input_id = input_id.into();
        if input_id.is_empty() || input_id.trim() != input_id || payload.is_empty() {
            return Err(invalid());
        }
        let content_hash = ContentHash::digest(&payload);
        Ok(Self {
            input_id,
            value_type,
            payload,
            content_hash,
        })
    }
    #[must_use]
    pub fn input_id(&self) -> &str {
        &self.input_id
    }
    #[must_use]
    pub fn value_type(&self) -> &TypedValue {
        &self.value_type
    }
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
    #[must_use]
    pub fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReproducibilityIdentityInput {
    pub external_inputs: Vec<ExecutionExternalInput>,
    pub data_snapshot_hash: ContentHash,
    pub universe_snapshot_hash: ContentHash,
    pub parameters_hash: ContentHash,
    pub runtime_image_digest: ContentHash,
    pub environment_digest: ContentHash,
    pub seed: u64,
    pub rule_pack_bindings: Vec<RulePackBinding>,
    pub node_implementations: Vec<NodeImplementation>,
}

/// Compatibility input for callers migrating from the original run-bound identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionIdentityInput {
    pub run_id: Ulid,
    pub data_snapshot_hash: ContentHash,
    pub universe_snapshot_hash: ContentHash,
    pub parameters_hash: ContentHash,
    pub runtime_image_digest: ContentHash,
    pub environment_digest: ContentHash,
    pub seed: u64,
    pub node_implementations: Vec<NodeImplementation>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReproducibilityIdentity {
    external_inputs: Vec<ExecutionExternalInput>,
    rule_pack_bindings: Vec<RulePackBinding>,
    data_snapshot_hash: ContentHash,
    universe_snapshot_hash: ContentHash,
    graph_digest: ContentHash,
    parameters_hash: ContentHash,
    runtime_image_digest: ContentHash,
    environment_digest: ContentHash,
    seed: u64,
    node_implementations: Vec<NodeImplementation>,
    subject: Option<FormalInputBinding>,
    code: Option<CodeBinding>,
    digest: ContentHash,
}

impl ReproducibilityIdentity {
    /// Freezes every input that may change a native graph result.
    ///
    /// # Errors
    ///
    /// Returns `InvalidValue` unless implementation bindings exactly cover the graph and rule pack
    /// identifiers are non-empty and unique.
    pub fn new(
        graph: &ResearchGraph,
        input: ReproducibilityIdentityInput,
    ) -> Result<Self, RuntimeError> {
        Self::build(graph, input, None, None)
    }

    /// Freezes the exact Subject and source code identity in addition to the legacy graph inputs.
    ///
    /// # Errors
    ///
    /// Returns `InvalidValue` unless `subject` is the exact binding for the canonical `subject`
    /// role, or unless the remaining graph bindings fail the same checks as [`Self::new`].
    pub fn new_formal(
        graph: &ResearchGraph,
        input: ReproducibilityIdentityInput,
        subject: FormalInputBinding,
        code: CodeBinding,
    ) -> Result<Self, RuntimeError> {
        if subject.kind() != FormalInputKind::Subject || subject.role() != "subject" {
            return Err(invalid());
        }
        Self::build(graph, input, Some(subject), Some(code))
    }

    fn build(
        graph: &ResearchGraph,
        input: ReproducibilityIdentityInput,
        subject: Option<FormalInputBinding>,
        code: Option<CodeBinding>,
    ) -> Result<Self, RuntimeError> {
        let mut external_inputs = input.external_inputs;
        external_inputs.sort_by(|left, right| left.input_id.cmp(&right.input_id));
        if external_inputs
            .windows(2)
            .any(|pair| pair[0].input_id == pair[1].input_id)
            || external_inputs.len() != graph.external_inputs().len()
            || !external_inputs
                .iter()
                .zip(graph.external_inputs())
                .all(|(binding, declaration)| {
                    binding.input_id == declaration.input_id()
                        && binding.value_type == *declaration.value_type()
                })
        {
            return Err(invalid());
        }
        let mut implementations = input.node_implementations;
        implementations.sort_by(|left, right| left.node_id.cmp(&right.node_id));
        if implementations
            .windows(2)
            .any(|pair| pair[0].node_id == pair[1].node_id)
            || implementations.len() != graph.nodes().len()
            || !implementations
                .iter()
                .zip(graph.nodes())
                .all(|(binding, node)| binding.node_id == *node.node_id())
        {
            return Err(invalid());
        }
        let mut rule_pack_bindings = input.rule_pack_bindings;
        for binding in &rule_pack_bindings {
            if binding.rule_pack_id.is_empty()
                || binding.rule_pack_id.trim() != binding.rule_pack_id
            {
                return Err(invalid());
            }
        }
        rule_pack_bindings.sort();
        if rule_pack_bindings.windows(2).any(|pair| {
            pair[0].rule_pack_id == pair[1].rule_pack_id && pair[0].version == pair[1].version
        }) {
            return Err(invalid());
        }
        let mut result = Self {
            external_inputs,
            rule_pack_bindings,
            data_snapshot_hash: input.data_snapshot_hash,
            universe_snapshot_hash: input.universe_snapshot_hash,
            graph_digest: graph.digest().clone(),
            parameters_hash: input.parameters_hash,
            runtime_image_digest: input.runtime_image_digest,
            environment_digest: input.environment_digest,
            seed: input.seed,
            node_implementations: implementations,
            subject,
            code,
            digest: ContentHash::digest(b"uninitialized"),
        };
        result.digest = ContentHash::digest(&result.canonical_bytes());
        Ok(result)
    }

    #[must_use]
    pub fn digest(&self) -> &ContentHash {
        &self.digest
    }
    #[must_use]
    pub fn external_inputs(&self) -> &[ExecutionExternalInput] {
        &self.external_inputs
    }
    #[must_use]
    pub fn rule_pack_bindings(&self) -> &[RulePackBinding] {
        &self.rule_pack_bindings
    }
    #[must_use]
    pub fn graph_digest(&self) -> &ContentHash {
        &self.graph_digest
    }
    #[must_use]
    pub fn data_snapshot_hash(&self) -> &ContentHash {
        &self.data_snapshot_hash
    }
    #[must_use]
    pub fn universe_snapshot_hash(&self) -> &ContentHash {
        &self.universe_snapshot_hash
    }
    #[must_use]
    pub fn parameters_hash(&self) -> &ContentHash {
        &self.parameters_hash
    }
    #[must_use]
    pub fn runtime_image_digest(&self) -> &ContentHash {
        &self.runtime_image_digest
    }
    #[must_use]
    pub fn environment_digest(&self) -> &ContentHash {
        &self.environment_digest
    }
    #[must_use]
    pub fn seed(&self) -> u64 {
        self.seed
    }
    #[must_use]
    pub fn node_implementations(&self) -> &[NodeImplementation] {
        &self.node_implementations
    }
    #[must_use]
    pub fn subject(&self) -> Option<&FormalInputBinding> {
        self.subject.as_ref()
    }
    #[must_use]
    pub fn code(&self) -> Option<&CodeBinding> {
        self.code.as_ref()
    }

    fn implementation(&self, node_id: &Ulid) -> Option<&ContentHash> {
        self.node_implementations
            .binary_search_by(|binding| binding.node_id.cmp(node_id))
            .ok()
            .map(|index| &self.node_implementations[index].implementation_digest)
    }

    fn canonical_bytes(&self) -> Vec<u8> {
        let formal = self.subject.is_some() && self.code.is_some();
        let mut bytes = if formal {
            b"ficant/reproducibility-identity/v2".to_vec()
        } else {
            b"ficant/reproducibility-identity/v1".to_vec()
        };
        push_u64(&mut bytes, self.external_inputs.len() as u64);
        for input in &self.external_inputs {
            push_str(&mut bytes, &input.input_id);
            push_typed_value(&mut bytes, &input.value_type);
            bytes.extend_from_slice(input.content_hash().as_bytes());
        }
        push_u64(&mut bytes, self.rule_pack_bindings.len() as u64);
        for binding in &self.rule_pack_bindings {
            push_str(&mut bytes, &binding.rule_pack_id);
            push_u64(&mut bytes, binding.version.get());
            bytes.extend_from_slice(binding.content_hash.as_bytes());
        }
        for hash in [
            &self.graph_digest,
            &self.data_snapshot_hash,
            &self.universe_snapshot_hash,
            &self.parameters_hash,
            &self.runtime_image_digest,
            &self.environment_digest,
        ] {
            bytes.extend_from_slice(hash.as_bytes());
        }
        bytes.extend_from_slice(&self.seed.to_be_bytes());
        push_u64(&mut bytes, self.node_implementations.len() as u64);
        for binding in &self.node_implementations {
            push_str(&mut bytes, binding.node_id.as_str());
            bytes.extend_from_slice(binding.implementation_digest.as_bytes());
        }
        if let (Some(subject), Some(code)) = (&self.subject, &self.code) {
            let subject_bytes = subject.canonical_bytes();
            push_u64(&mut bytes, subject_bytes.len() as u64);
            bytes.extend_from_slice(&subject_bytes);
            let code_bytes = code.canonical_bytes();
            push_u64(&mut bytes, code_bytes.len() as u64);
            bytes.extend_from_slice(&code_bytes);
        }
        bytes
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionInstanceIdentity {
    run_id: Ulid,
    reproducibility: ReproducibilityIdentity,
    digest: ContentHash,
}

impl ExecutionInstanceIdentity {
    /// Compatibility constructor. New callers should construct a `ReproducibilityIdentity` and use
    /// `from_reproducibility`.
    ///
    /// # Errors
    ///
    /// Returns a stable validation error when the supplied frozen inputs do not cover the graph.
    pub fn new(graph: &ResearchGraph, input: ExecutionIdentityInput) -> Result<Self, RuntimeError> {
        let reproducibility = ReproducibilityIdentity::new(
            graph,
            ReproducibilityIdentityInput {
                external_inputs: vec![],
                data_snapshot_hash: input.data_snapshot_hash,
                universe_snapshot_hash: input.universe_snapshot_hash,
                parameters_hash: input.parameters_hash,
                runtime_image_digest: input.runtime_image_digest,
                environment_digest: input.environment_digest,
                seed: input.seed,
                rule_pack_bindings: vec![],
                node_implementations: input.node_implementations,
            },
        )?;
        Ok(Self::from_reproducibility(input.run_id, reproducibility))
    }

    #[must_use]
    pub fn from_reproducibility(run_id: Ulid, reproducibility: ReproducibilityIdentity) -> Self {
        let mut bytes = b"ficant/execution-instance-identity/v1".to_vec();
        push_str(&mut bytes, run_id.as_str());
        bytes.extend_from_slice(reproducibility.digest().as_bytes());
        Self {
            run_id,
            reproducibility,
            digest: ContentHash::digest(&bytes),
        }
    }

    #[must_use]
    pub fn run_id(&self) -> &Ulid {
        &self.run_id
    }
    #[must_use]
    pub fn reproducibility(&self) -> &ReproducibilityIdentity {
        &self.reproducibility
    }
    #[must_use]
    pub fn reproducibility_digest(&self) -> &ContentHash {
        self.reproducibility.digest()
    }
    #[must_use]
    pub fn digest(&self) -> &ContentHash {
        &self.digest
    }
}

pub type ExecutionIdentity = ExecutionInstanceIdentity;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativePortValue {
    port_name: String,
    value_type: TypedValue,
    payload: Vec<u8>,
    content_hash: ContentHash,
}

impl NativePortValue {
    /// Creates one immutable typed value.
    ///
    /// # Errors
    ///
    /// Returns `InvalidValue` for an empty/padded port or empty payload.
    pub fn new(
        port_name: impl Into<String>,
        value_type: TypedValue,
        payload: Vec<u8>,
    ) -> Result<Self, RuntimeError> {
        let port_name = port_name.into();
        if port_name.is_empty() || port_name.trim() != port_name || payload.is_empty() {
            return Err(invalid());
        }
        let content_hash = ContentHash::digest(&payload);
        Ok(Self {
            port_name,
            value_type,
            payload,
            content_hash,
        })
    }
    #[must_use]
    pub fn port_name(&self) -> &str {
        &self.port_name
    }
    #[must_use]
    pub fn value_type(&self) -> &TypedValue {
        &self.value_type
    }
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
    #[must_use]
    pub fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

pub struct NativeNodeRequest<'a> {
    node: &'a ResearchNode,
    identity: &'a ReproducibilityIdentity,
    inputs: &'a [NativePortValue],
}

impl NativeNodeRequest<'_> {
    #[must_use]
    pub fn node(&self) -> &ResearchNode {
        self.node
    }
    #[must_use]
    pub fn identity(&self) -> &ReproducibilityIdentity {
        self.identity
    }
    #[must_use]
    pub fn inputs(&self) -> &[NativePortValue] {
        self.inputs
    }
}

pub trait NativeNode {
    fn node_id(&self) -> &Ulid;
    fn implementation_digest(&self) -> &ContentHash;
    /// Executes one validated request without external side effects.
    ///
    /// # Errors
    ///
    /// Returns a stable runtime/domain error when execution cannot produce the declared outputs.
    fn execute(
        &self,
        request: &NativeNodeRequest<'_>,
    ) -> Result<Vec<NativePortValue>, RuntimeError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeNodeArtifact {
    node_id: Ulid,
    contract_digest: ContentHash,
    implementation_digest: ContentHash,
    input_artifacts: Vec<ContentHash>,
    output_hashes: Vec<ContentHash>,
    output_envelope_hash: ContentHash,
    artifact_digest: ContentHash,
}

impl NativeNodeArtifact {
    #[must_use]
    pub fn node_id(&self) -> &Ulid {
        &self.node_id
    }
    #[must_use]
    pub fn contract_digest(&self) -> &ContentHash {
        &self.contract_digest
    }
    #[must_use]
    pub fn implementation_digest(&self) -> &ContentHash {
        &self.implementation_digest
    }
    #[must_use]
    pub fn input_artifacts(&self) -> &[ContentHash] {
        &self.input_artifacts
    }
    #[must_use]
    pub fn output_hashes(&self) -> &[ContentHash] {
        &self.output_hashes
    }
    #[must_use]
    pub fn output_envelope_hash(&self) -> &ContentHash {
        &self.output_envelope_hash
    }
    #[must_use]
    pub fn artifact_digest(&self) -> &ContentHash {
        &self.artifact_digest
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeNodeExecution {
    outputs: Vec<NativePortValue>,
    output_envelope: Vec<u8>,
    artifact: NativeNodeArtifact,
}

impl NativeNodeExecution {
    #[must_use]
    pub fn outputs(&self) -> &[NativePortValue] {
        &self.outputs
    }
    #[must_use]
    pub fn output_envelope(&self) -> &[u8] {
        &self.output_envelope
    }
    #[must_use]
    pub fn artifact(&self) -> &NativeNodeArtifact {
        &self.artifact
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeExecutionResult {
    identity: ExecutionInstanceIdentity,
    artifacts: Vec<NativeNodeArtifact>,
    result_digest: ContentHash,
}

impl NativeExecutionResult {
    #[must_use]
    pub fn identity(&self) -> &ExecutionInstanceIdentity {
        &self.identity
    }
    #[must_use]
    pub fn artifacts(&self) -> &[NativeNodeArtifact] {
        &self.artifacts
    }
    #[must_use]
    pub fn result_digest(&self) -> &ContentHash {
        &self.result_digest
    }
}

/// Executes one node after the caller has assembled and verified its external/upstream inputs.
///
/// # Errors
///
/// Returns a stable error for implementation, input, or output contract drift.
pub fn execute_native_node(
    node: &ResearchNode,
    identity: &ReproducibilityIdentity,
    executor: &dyn NativeNode,
    mut inputs: Vec<NativePortValue>,
    mut input_artifacts: Vec<ContentHash>,
) -> Result<NativeNodeExecution, RuntimeError> {
    if executor.node_id() != node.node_id()
        || identity.implementation(node.node_id()) != Some(executor.implementation_digest())
    {
        return Err(invalid());
    }
    inputs.sort_by(|left, right| left.port_name.cmp(&right.port_name));
    if inputs.len() != node.contract().input_types().len()
        || !inputs
            .iter()
            .zip(node.contract().input_types())
            .all(|(value, port)| {
                value.port_name == port.port_name() && value.value_type == *port.value_type()
            })
    {
        return Err(invalid());
    }
    input_artifacts.sort();
    input_artifacts.dedup();
    let request = NativeNodeRequest {
        node,
        identity,
        inputs: &inputs,
    };
    let mut outputs = executor.execute(&request)?;
    outputs.sort_by(|left, right| left.port_name.cmp(&right.port_name));
    if outputs.len() != node.contract().output_types().len()
        || !outputs
            .iter()
            .zip(node.contract().output_types())
            .all(|(value, port)| {
                value.port_name == port.port_name() && value.value_type == *port.value_type()
            })
    {
        return Err(invalid());
    }
    let output_hashes = outputs
        .iter()
        .map(|value| value.content_hash.clone())
        .collect::<Vec<_>>();
    let output_envelope = canonical_output_bytes(&outputs);
    let output_envelope_hash = ContentHash::digest(&output_envelope);
    let artifact_digest = artifact_digest(
        identity.digest(),
        node,
        executor.implementation_digest(),
        &input_artifacts,
        &output_hashes,
        &output_envelope_hash,
    );
    Ok(NativeNodeExecution {
        outputs,
        output_envelope,
        artifact: NativeNodeArtifact {
            node_id: node.node_id().clone(),
            contract_digest: node.contract().digest().clone(),
            implementation_digest: executor.implementation_digest().clone(),
            input_artifacts,
            output_hashes,
            output_envelope_hash,
            artifact_digest,
        },
    })
}

/// Executes a graph without external inputs. Retained for source-node compatibility.
///
/// # Errors
///
/// Returns a stable error if the graph declares any external input.
pub fn execute_native_graph(
    graph: &ResearchGraph,
    identity: &ExecutionInstanceIdentity,
    executors: &[&dyn NativeNode],
) -> Result<NativeExecutionResult, RuntimeError> {
    execute_native_graph_with_external_inputs(graph, identity, executors, &[])
}

/// Executes every native node exactly once in deterministic graph order.
///
/// # Errors
///
/// Returns a stable error for external input drift, missing/duplicate implementations, or any
/// input/output contract drift.
pub fn execute_native_graph_with_external_inputs(
    graph: &ResearchGraph,
    identity: &ExecutionInstanceIdentity,
    executors: &[&dyn NativeNode],
    external_inputs: &[ExecutionExternalInput],
) -> Result<NativeExecutionResult, RuntimeError> {
    let reproducibility = identity.reproducibility();
    if reproducibility.graph_digest != *graph.digest() || executors.len() != graph.nodes().len() {
        return Err(invalid());
    }
    let external_registry = external_input_registry(graph, reproducibility, external_inputs)?;
    let registry = executor_registry(executors)?;
    let nodes = research_nodes(graph);
    let mut outputs: BTreeMap<(Ulid, String), NativePortValue> = BTreeMap::new();
    let mut artifacts: BTreeMap<Ulid, NativeNodeArtifact> = BTreeMap::new();
    for node_id in graph.topological_order() {
        let node = nodes.get(node_id).ok_or_else(broken)?;
        let executor = registry.get(node_id).ok_or_else(broken)?;
        let mut inputs = Vec::new();
        let mut input_artifacts = Vec::new();
        for edge in graph
            .edges()
            .iter()
            .filter(|edge| edge.to_node() == node_id)
        {
            let source = outputs
                .get(&(edge.from_node().clone(), edge.from_port().to_owned()))
                .ok_or_else(broken)?;
            inputs.push(NativePortValue::new(
                edge.to_port(),
                source.value_type.clone(),
                source.payload.clone(),
            )?);
            let artifact = artifacts.get(edge.from_node()).ok_or_else(broken)?;
            input_artifacts.push(artifact.artifact_digest.clone());
        }
        for binding in graph
            .external_input_bindings()
            .iter()
            .filter(|binding| binding.to_node() == node_id)
        {
            let external = external_registry
                .get(binding.input_id())
                .ok_or_else(broken)?;
            inputs.push(NativePortValue::new(
                binding.to_port(),
                external.value_type.clone(),
                external.payload.clone(),
            )?);
            input_artifacts.push(external.content_hash.clone());
        }
        let execution =
            execute_native_node(node, reproducibility, *executor, inputs, input_artifacts)?;
        for output in execution.outputs.iter().cloned() {
            outputs.insert((node_id.clone(), output.port_name.clone()), output);
        }
        artifacts.insert(node_id.clone(), execution.artifact);
    }
    let ordered = graph
        .topological_order()
        .iter()
        .map(|id| artifacts.remove(id).ok_or_else(broken))
        .collect::<Result<Vec<_>, _>>()?;
    let result_digest = result_digest(reproducibility.digest(), &ordered);
    Ok(NativeExecutionResult {
        identity: identity.clone(),
        artifacts: ordered,
        result_digest,
    })
}

fn external_input_registry<'a>(
    graph: &ResearchGraph,
    identity: &ReproducibilityIdentity,
    external_inputs: &'a [ExecutionExternalInput],
) -> Result<BTreeMap<&'a str, &'a ExecutionExternalInput>, RuntimeError> {
    if external_inputs.len() != graph.external_inputs().len() {
        return Err(invalid());
    }
    let mut registry = BTreeMap::new();
    for input in external_inputs {
        if registry.insert(input.input_id(), input).is_some() {
            return Err(invalid());
        }
    }
    for declaration in graph.external_inputs() {
        let actual = registry.get(declaration.input_id()).ok_or_else(broken)?;
        let frozen = identity
            .external_inputs()
            .binary_search_by(|candidate| candidate.input_id().cmp(declaration.input_id()))
            .ok()
            .and_then(|index| identity.external_inputs().get(index))
            .ok_or_else(broken)?;
        if actual.value_type != *declaration.value_type()
            || actual.value_type != *frozen.value_type()
            || actual.content_hash != *frozen.content_hash()
        {
            return Err(RuntimeError::Domain(DomainErrorCode::ContentHashMismatch));
        }
    }
    Ok(registry)
}

/// Encodes already validated, ordered node outputs into the canonical persistence envelope.
#[must_use]
pub fn canonical_output_bytes(outputs: &[NativePortValue]) -> Vec<u8> {
    let mut bytes = b"ficant/native-node-output-envelope/v1".to_vec();
    push_u64(&mut bytes, outputs.len() as u64);
    for output in outputs {
        push_str(&mut bytes, &output.port_name);
        push_typed_value(&mut bytes, &output.value_type);
        bytes.extend_from_slice(output.content_hash.as_bytes());
        push_u64(&mut bytes, output.payload.len() as u64);
        bytes.extend_from_slice(&output.payload);
    }
    bytes
}

/// Decodes a canonical persisted node-output envelope and optionally verifies its total hash.
///
/// # Errors
///
/// Fails closed on any format, length, UTF-8, type, schema, content-hash, ordering, duplicate-port,
/// or trailing-byte drift. A supplied envelope hash must match before decoded values are returned.
pub fn decode_canonical_output_bytes(
    bytes: &[u8],
    expected_envelope_hash: Option<&ContentHash>,
) -> Result<Vec<NativePortValue>, RuntimeError> {
    const MAGIC: &[u8] = b"ficant/native-node-output-envelope/v1";
    if expected_envelope_hash.is_some_and(|expected| ContentHash::digest(bytes) != *expected) {
        return Err(hash_mismatch());
    }
    let mut decoder = EnvelopeDecoder::new(bytes);
    if decoder.take(MAGIC.len())? != MAGIC {
        return Err(invalid());
    }
    let count = decoder.usize()?;
    if count == 0 {
        return Err(invalid());
    }
    let mut outputs = Vec::with_capacity(count.min(64));
    let mut previous_port: Option<String> = None;
    for _ in 0..count {
        let port_name = decoder.string()?;
        if previous_port
            .as_ref()
            .is_some_and(|previous| previous >= &port_name)
        {
            return Err(invalid());
        }
        let type_id = decoder.string()?;
        let type_version = Version::new(decoder.try_u64()?).map_err(RuntimeError::Domain)?;
        let schema_hash =
            ContentHash::from_bytes(decoder.take(32)?).map_err(RuntimeError::Domain)?;
        let declared_content_hash =
            ContentHash::from_bytes(decoder.take(32)?).map_err(RuntimeError::Domain)?;
        let payload = decoder.bytes()?;
        let value_type =
            TypedValue::new(type_id, type_version, schema_hash).map_err(RuntimeError::Domain)?;
        let output = NativePortValue::new(port_name.clone(), value_type, payload)?;
        if output.content_hash() != &declared_content_hash {
            return Err(hash_mismatch());
        }
        previous_port = Some(port_name);
        outputs.push(output);
    }
    if !decoder.finished() {
        return Err(invalid());
    }
    Ok(outputs)
}

struct EnvelopeDecoder<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> EnvelopeDecoder<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], RuntimeError> {
        let end = self.offset.checked_add(length).ok_or_else(invalid)?;
        let value = self.bytes.get(self.offset..end).ok_or_else(invalid)?;
        self.offset = end;
        Ok(value)
    }

    fn usize(&mut self) -> Result<usize, RuntimeError> {
        usize::try_from(self.try_u64()?).map_err(|_| invalid())
    }

    fn try_u64(&mut self) -> Result<u64, RuntimeError> {
        let mut value = [0_u8; 8];
        value.copy_from_slice(self.take(8)?);
        Ok(u64::from_be_bytes(value))
    }

    fn string(&mut self) -> Result<String, RuntimeError> {
        let bytes = self.bytes()?;
        String::from_utf8(bytes).map_err(|_| invalid())
    }

    fn bytes(&mut self) -> Result<Vec<u8>, RuntimeError> {
        let length = self.usize()?;
        Ok(self.take(length)?.to_vec())
    }

    const fn finished(&self) -> bool {
        self.offset == self.bytes.len()
    }
}

fn result_digest(identity: &ContentHash, artifacts: &[NativeNodeArtifact]) -> ContentHash {
    let mut bytes = b"ficant/native-execution-result/v1".to_vec();
    bytes.extend_from_slice(identity.as_bytes());
    for artifact in artifacts {
        bytes.extend_from_slice(artifact.artifact_digest.as_bytes());
    }
    ContentHash::digest(&bytes)
}

fn executor_registry<'a>(
    executors: &[&'a dyn NativeNode],
) -> Result<BTreeMap<Ulid, &'a dyn NativeNode>, RuntimeError> {
    let mut registry = BTreeMap::new();
    for executor in executors {
        if registry
            .insert(executor.node_id().clone(), *executor)
            .is_some()
        {
            return Err(invalid());
        }
    }
    Ok(registry)
}

fn research_nodes(graph: &ResearchGraph) -> BTreeMap<Ulid, &ResearchNode> {
    graph
        .nodes()
        .iter()
        .map(|node| (node.node_id().clone(), node))
        .collect()
}

/// Confirms that a replay produced the same reproducibility identity and node lineage.
///
/// # Errors
///
/// Returns `ContentHashMismatch` for any reproducibility, artifact, or final result drift.
pub fn verify_native_replay(
    expected: &NativeExecutionResult,
    replayed: &NativeExecutionResult,
) -> Result<(), RuntimeError> {
    if expected.identity.reproducibility != replayed.identity.reproducibility
        || expected.artifacts != replayed.artifacts
        || expected.result_digest != replayed.result_digest
    {
        return Err(RuntimeError::Domain(DomainErrorCode::ContentHashMismatch));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ComparisonDimension {
    ExternalInput,
    RulePack,
    DataSnapshot,
    UniverseSnapshot,
    Graph,
    Parameters,
    RuntimeImage,
    Environment,
    Seed,
    Implementation,
    Result,
    Subject,
    Code,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExperimentComparison {
    differences: Vec<ComparisonDimension>,
}
impl ExperimentComparison {
    #[must_use]
    pub fn differences(&self) -> &[ComparisonDimension] {
        &self.differences
    }
    #[must_use]
    pub fn identical(&self) -> bool {
        self.differences.is_empty()
    }
}

#[must_use]
pub fn compare_experiments(
    left: &NativeExecutionResult,
    right: &NativeExecutionResult,
) -> ExperimentComparison {
    let mut differences = BTreeSet::new();
    let l = left.identity.reproducibility();
    let r = right.identity.reproducibility();
    if l.external_inputs != r.external_inputs {
        differences.insert(ComparisonDimension::ExternalInput);
    }
    if l.rule_pack_bindings != r.rule_pack_bindings {
        differences.insert(ComparisonDimension::RulePack);
    }
    if l.data_snapshot_hash != r.data_snapshot_hash {
        differences.insert(ComparisonDimension::DataSnapshot);
    }
    if l.universe_snapshot_hash != r.universe_snapshot_hash {
        differences.insert(ComparisonDimension::UniverseSnapshot);
    }
    if l.graph_digest != r.graph_digest {
        differences.insert(ComparisonDimension::Graph);
    }
    if l.parameters_hash != r.parameters_hash {
        differences.insert(ComparisonDimension::Parameters);
    }
    if l.runtime_image_digest != r.runtime_image_digest {
        differences.insert(ComparisonDimension::RuntimeImage);
    }
    if l.environment_digest != r.environment_digest {
        differences.insert(ComparisonDimension::Environment);
    }
    if l.seed != r.seed {
        differences.insert(ComparisonDimension::Seed);
    }
    if l.node_implementations != r.node_implementations {
        differences.insert(ComparisonDimension::Implementation);
    }
    if l.subject != r.subject {
        differences.insert(ComparisonDimension::Subject);
    }
    if l.code != r.code {
        differences.insert(ComparisonDimension::Code);
    }
    if left.result_digest != right.result_digest {
        differences.insert(ComparisonDimension::Result);
    }
    ExperimentComparison {
        differences: differences.into_iter().collect(),
    }
}

fn artifact_digest(
    identity: &ContentHash,
    node: &ResearchNode,
    implementation: &ContentHash,
    inputs: &[ContentHash],
    outputs: &[ContentHash],
    output_envelope_hash: &ContentHash,
) -> ContentHash {
    let mut bytes = b"ficant/native-node-artifact/v2".to_vec();
    bytes.extend_from_slice(identity.as_bytes());
    push_str(&mut bytes, node.node_id().as_str());
    bytes.extend_from_slice(node.contract().digest().as_bytes());
    bytes.extend_from_slice(implementation.as_bytes());
    push_u64(&mut bytes, inputs.len() as u64);
    for hash in inputs {
        bytes.extend_from_slice(hash.as_bytes());
    }
    push_u64(&mut bytes, outputs.len() as u64);
    for hash in outputs {
        bytes.extend_from_slice(hash.as_bytes());
    }
    bytes.extend_from_slice(output_envelope_hash.as_bytes());
    ContentHash::digest(&bytes)
}

fn push_typed_value(bytes: &mut Vec<u8>, value: &TypedValue) {
    push_str(bytes, value.type_id());
    push_u64(bytes, value.type_version().get());
    bytes.extend_from_slice(value.schema_hash().as_bytes());
}

fn push_str(bytes: &mut Vec<u8>, value: &str) {
    push_u64(bytes, value.len() as u64);
    bytes.extend_from_slice(value.as_bytes());
}
fn push_u64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_be_bytes());
}
fn invalid() -> RuntimeError {
    RuntimeError::Domain(DomainErrorCode::InvalidValue)
}
fn hash_mismatch() -> RuntimeError {
    RuntimeError::Domain(DomainErrorCode::ContentHashMismatch)
}
fn broken() -> RuntimeError {
    RuntimeError::Domain(DomainErrorCode::BrokenLineage)
}
