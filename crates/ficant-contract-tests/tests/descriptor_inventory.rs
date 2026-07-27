use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use prost::Message;
use prost_types::field_descriptor_proto::{Label, Type};
use prost_types::{
    DescriptorProto, EnumDescriptorProto, FileDescriptorSet, ServiceDescriptorProto,
};

use ficant_contracts::ficant::app::v1::AppRegistry;
use ficant_contracts::ficant::core::v1::{
    DecimalValue, Subject, SubjectStateSnapshot, SubjectVersion,
};
use ficant_contracts::ficant::market::v1::{Instrument, InstrumentKind};
use ficant_contracts::ficant::rates::v1::AnalyzeBondRequest;
use ficant_contracts::ficant::research::v1::{
    ExecutionInstanceIdentity, ExperimentRun, ReproducibilityIdentity, ResearchGraph, RunState,
};

const DEFAULT_BUF: &str = "/usr/local/bin/buf";
const BUF_VERSION: &str = "1.56.0";

static DESCRIPTOR_SET: OnceLock<FileDescriptorSet> = OnceLock::new();

const PHASE1_OBJECTS: [&str; 17] = [
    "ficant.market.v1.Instrument",
    "ficant.market.v1.Bond",
    "ficant.market.v1.FuturesContract",
    "ficant.market.v1.Cashflow",
    "ficant.market.v1.Calendar",
    "ficant.market.v1.Unit",
    "ficant.market.v1.Quote",
    "ficant.market.v1.Trade",
    "ficant.market.v1.Valuation",
    "ficant.market.v1.CurveSnapshot",
    "ficant.market.v1.MarketRulePack",
    "ficant.research.v1.DataSnapshot",
    "ficant.research.v1.UniverseSnapshot",
    "ficant.research.v1.ExperimentRun",
    "ficant.research.v1.Artifact",
    "ficant.research.v1.SignalSet",
    "ficant.research.v1.RunJournal",
];

#[test]
fn generated_rust_consumer_exports_representative_contracts() {
    let instrument = Instrument::default();
    let decimal = DecimalValue::default();
    let run = ExperimentRun::default();
    let registry = AppRegistry::default();
    let rates = AnalyzeBondRequest::default();
    let graph = ResearchGraph::default();
    let reproducibility = ReproducibilityIdentity::default();
    let execution = ExecutionInstanceIdentity::default();
    let subject = Subject::default();
    let subject_version = SubjectVersion::default();
    let subject_state = SubjectStateSnapshot::default();

    assert!(instrument.instrument_id.is_none());
    assert_eq!(instrument.kind, InstrumentKind::Unspecified as i32);
    assert!(decimal.unit.is_none());
    assert_eq!(run.state, RunState::Unspecified as i32);
    assert!(registry.apps.is_empty());
    assert!(rates.context.is_none());
    assert!(graph.nodes.is_empty());
    assert!(reproducibility.node_implementations.is_empty());
    assert!(execution.reproducibility.is_none());
    assert!(subject.subject_id.is_none());
    assert!(subject_version.subject_ref.is_none());
    assert!(subject_state.snapshot_id.is_none());
}

#[derive(Clone, Copy)]
struct ExpectedField {
    name: &'static str,
    field_type: Type,
    type_name: Option<&'static str>,
    repeated: bool,
    oneof: Option<&'static str>,
}

impl ExpectedField {
    const fn scalar(name: &'static str, field_type: Type) -> Self {
        Self {
            name,
            field_type,
            type_name: None,
            repeated: false,
            oneof: None,
        }
    }

    const fn message(name: &'static str, type_name: &'static str) -> Self {
        Self {
            name,
            field_type: Type::Message,
            type_name: Some(type_name),
            repeated: false,
            oneof: None,
        }
    }

    const fn repeated_message(name: &'static str, type_name: &'static str) -> Self {
        Self {
            name,
            field_type: Type::Message,
            type_name: Some(type_name),
            repeated: true,
            oneof: None,
        }
    }

    const fn repeated_scalar(name: &'static str, field_type: Type) -> Self {
        Self {
            name,
            field_type,
            type_name: None,
            repeated: true,
            oneof: None,
        }
    }

    const fn enumeration(name: &'static str, type_name: &'static str) -> Self {
        Self {
            name,
            field_type: Type::Enum,
            type_name: Some(type_name),
            repeated: false,
            oneof: None,
        }
    }

    const fn oneof_message(
        name: &'static str,
        type_name: &'static str,
        oneof: &'static str,
    ) -> Self {
        Self {
            name,
            field_type: Type::Message,
            type_name: Some(type_name),
            repeated: false,
            oneof: Some(oneof),
        }
    }

    const fn oneof_scalar(name: &'static str, field_type: Type, oneof: &'static str) -> Self {
        Self {
            name,
            field_type,
            type_name: None,
            repeated: false,
            oneof: Some(oneof),
        }
    }
}

#[derive(Clone, Copy)]
struct ExpectedMethod {
    name: &'static str,
    input: &'static str,
    output: &'static str,
}

impl ExpectedMethod {
    const fn new(name: &'static str, input: &'static str, output: &'static str) -> Self {
        Self {
            name,
            input,
            output,
        }
    }
}

#[test]
fn descriptor_inventory_is_unique_and_preserves_phase1_semantics() {
    let descriptor_set = descriptor_set();
    let messages = top_level_messages(descriptor_set);

    let missing: Vec<_> = PHASE1_OBJECTS
        .iter()
        .filter(|name| !messages.contains_key(**name))
        .copied()
        .collect();
    assert!(
        missing.is_empty(),
        "descriptor missing 17-object inventory entries:\n{}",
        missing.join("\n")
    );

    assert_eq!(
        PHASE1_OBJECTS
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .len(),
        PHASE1_OBJECTS.len(),
        "the expected inventory itself must not contain aliases"
    );

    assert_allowed_packages(&descriptor_set);
    assert_no_floating_point_fields(descriptor_set);
    assert_no_parallel_contract_representations(descriptor_set);
    assert_shared_types(&messages);
    assert_subject_contracts(&messages);
    assert_phase1_objects(&messages);
    assert_service_inventory(descriptor_set);
}

#[test]
fn registry_service_has_exact_four_unary_rpcs() {
    assert_exact_service(
        descriptor_set(),
        "ficant.core.v1.RegistryService",
        &registry_methods(),
    );
}

#[test]
fn market_definition_query_service_has_exact_signatures() {
    let descriptor_set = descriptor_set();
    assert_exact_service(
        descriptor_set,
        "ficant.market.v1.MarketDefinitionService",
        &market_definition_methods(),
    );
}

#[test]
fn market_fact_query_service_has_exact_signatures() {
    let descriptor_set = descriptor_set();
    assert_exact_service(
        descriptor_set,
        "ficant.market.v1.MarketFactService",
        &market_fact_methods(),
    );
}

#[test]
fn artifact_lineage_query_service_has_exact_signatures() {
    let descriptor_set = descriptor_set();
    assert_exact_service(
        descriptor_set,
        "ficant.research.v1.ArtifactService",
        &artifact_methods(),
    );
}

#[test]
fn architecture_query_messages_have_exact_schemas() {
    let descriptor_set = descriptor_set();
    let messages = top_level_messages(descriptor_set);
    assert_query_contracts(&messages);
}

#[test]
fn phase4_graph_and_execution_messages_have_exact_schemas() {
    let messages = top_level_messages(descriptor_set());
    assert_phase4_contracts(&messages);
}

#[test]
fn platform_service_has_exact_seven_rpc_security_contract() {
    let descriptor_set = descriptor_set();
    assert_exact_service(
        descriptor_set,
        "ficant.app.v1.PlatformService",
        &platform_methods(),
    );
}

#[test]
fn rates_analytics_service_has_exact_phase2e_signatures() {
    let descriptor_set = descriptor_set();
    assert_exact_service(
        descriptor_set,
        "ficant.rates.v1.RatesAnalyticsService",
        &rates_analytics_methods(),
    );
}

#[test]
fn platform_messages_fields_enums_and_oneofs_are_exact() {
    let descriptor_set = descriptor_set();
    let messages = top_level_messages(descriptor_set);

    assert_platform_contracts(&messages);
}

#[test]
fn platform_error_enum_has_exact_numeric_values() {
    let enums = top_level_enums(descriptor_set());
    assert_enum(
        &enums,
        "ficant.app.v1.ErrorCode",
        &[
            ("ERROR_CODE_UNSPECIFIED", 0),
            ("ERROR_CODE_UNAUTHENTICATED", 1),
            ("ERROR_CODE_FORBIDDEN", 2),
            ("ERROR_CODE_NOT_FOUND", 3),
            ("ERROR_CODE_INVALID_REQUEST", 4),
            ("ERROR_CODE_EXPIRED", 5),
            ("ERROR_CODE_UNAVAILABLE", 6),
            ("ERROR_CODE_INTERNAL", 7),
        ],
    );
}

#[test]
fn shared_and_domain_enums_and_unaffected_services_are_exact() {
    let descriptor_set = descriptor_set();
    let enums = top_level_enums(descriptor_set);

    assert_domain_enums(&enums);
    assert_exact_service(
        descriptor_set,
        "ficant.research.v1.SnapshotService",
        &snapshot_methods(),
    );
    assert_exact_service(
        descriptor_set,
        "ficant.research.v1.ExperimentService",
        &experiment_methods(),
    );
}

fn descriptor_set() -> &'static FileDescriptorSet {
    DESCRIPTOR_SET.get_or_init(build_descriptor)
}

fn build_descriptor() -> FileDescriptorSet {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("contract test crate must remain two levels below the repository root");

    let buf = std::env::var_os("FICANT_BUF")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_BUF));
    let version = Command::new(&buf)
        .arg("--version")
        .output()
        .expect("fixed Buf binary must be executable");
    assert!(version.status.success(), "fixed Buf version check failed");
    assert_eq!(
        String::from_utf8_lossy(&version.stdout).trim(),
        BUF_VERSION,
        "descriptor test requires the Delivery-pinned Buf version"
    );

    let descriptor_path = descriptor_path();
    let _ = fs::remove_file(&descriptor_path);
    let output = Command::new(&buf)
        .args(["build", "interface", "--as-file-descriptor-set", "-o"])
        .arg(&descriptor_path)
        .current_dir(repo_root)
        .output()
        .expect("fixed Buf binary must build the descriptor");

    assert!(
        output.status.success(),
        "descriptor build must succeed before inventory assertions; stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let bytes = fs::read(&descriptor_path).expect("Buf must write the descriptor set");
    fs::remove_file(&descriptor_path).expect("descriptor test must clean its /tmp output");
    FileDescriptorSet::decode(bytes.as_slice())
        .expect("Buf output must decode as FileDescriptorSet")
}

fn descriptor_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "ficant-task2d-descriptor-{}.bin",
        std::process::id()
    ))
}

fn top_level_messages<'a>(
    descriptor_set: &'a FileDescriptorSet,
) -> BTreeMap<String, &'a DescriptorProto> {
    let mut messages = BTreeMap::new();
    for file in &descriptor_set.file {
        let package = file.package.as_deref().unwrap_or_default();
        for message in &file.message_type {
            let name = message
                .name
                .as_deref()
                .expect("every message must be named");
            let fqn = format!("{package}.{name}");
            assert!(
                messages.insert(fqn.clone(), message).is_none(),
                "duplicate top-level message {fqn}"
            );
        }
    }
    messages
}

fn top_level_enums<'a>(
    descriptor_set: &'a FileDescriptorSet,
) -> BTreeMap<String, &'a EnumDescriptorProto> {
    let mut enums = BTreeMap::new();
    for file in &descriptor_set.file {
        let package = file.package.as_deref().unwrap_or_default();
        for enumeration in &file.enum_type {
            let name = enumeration
                .name
                .as_deref()
                .expect("every enum must be named");
            let fqn = format!("{package}.{name}");
            assert!(
                enums.insert(fqn.clone(), enumeration).is_none(),
                "duplicate top-level enum {fqn}"
            );
        }
    }
    enums
}

fn top_level_services<'a>(
    descriptor_set: &'a FileDescriptorSet,
) -> BTreeMap<String, &'a ServiceDescriptorProto> {
    let mut services = BTreeMap::new();
    for file in &descriptor_set.file {
        let package = file.package.as_deref().unwrap_or_default();
        for service in &file.service {
            let name = service
                .name
                .as_deref()
                .expect("every service must be named");
            let fqn = format!("{package}.{name}");
            assert!(
                services.insert(fqn.clone(), service).is_none(),
                "duplicate top-level service {fqn}"
            );
        }
    }
    services
}

fn assert_allowed_packages(descriptor_set: &FileDescriptorSet) {
    let allowed = BTreeSet::from([
        "ficant.core.v1",
        "ficant.market.v1",
        "ficant.research.v1",
        "ficant.app.v1",
        "ficant.rates.v1",
    ]);
    for file in &descriptor_set.file {
        let Some(name) = file.name.as_deref() else {
            continue;
        };
        if name.starts_with("ficant/") {
            let package = file.package.as_deref().unwrap_or_default();
            assert!(
                allowed.contains(package),
                "unexpected ficant package {package}"
            );
        }
    }
}

fn assert_no_floating_point_fields(descriptor_set: &FileDescriptorSet) {
    for file in &descriptor_set.file {
        let package = file.package.as_deref().unwrap_or_default();
        for message in &file.message_type {
            assert_message_has_no_floating_point(message, &format!("{package}.{}", message.name()));
        }
    }
}

fn assert_message_has_no_floating_point(message: &DescriptorProto, fqn: &str) {
    for field in &message.field {
        let field_type = field.r#type();
        assert!(
            !matches!(field_type, Type::Float | Type::Double),
            "floating-point field is forbidden: {fqn}.{}",
            field.name()
        );
    }
    for nested in &message.nested_type {
        assert_message_has_no_floating_point(nested, &format!("{fqn}.{}", nested.name()));
    }
}

fn assert_no_parallel_contract_representations(descriptor_set: &FileDescriptorSet) {
    let canonical = PHASE1_OBJECTS
        .iter()
        .copied()
        .chain([
            "ficant.core.v1.Ulid",
            "ficant.core.v1.VersionRef",
            "ficant.core.v1.Sha256",
            "ficant.core.v1.OwnerRef",
            "ficant.core.v1.UnitRef",
            "ficant.core.v1.LineageRef",
            "ficant.core.v1.DecimalValue",
            "ficant.core.v1.MarketTime",
            "ficant.core.v1.PageRequest",
            "ficant.core.v1.PageResponse",
            "ficant.core.v1.ErrorDetail",
        ])
        .collect::<BTreeSet<_>>();
    let protected_names = canonical
        .iter()
        .map(|fqn| {
            fqn.rsplit('.')
                .next()
                .expect("canonical type must be named")
        })
        .collect::<BTreeSet<_>>();

    for file in &descriptor_set.file {
        let package = file.package.as_deref().unwrap_or_default();
        for message in &file.message_type {
            let fqn = format!("{package}.{}", message.name());
            if protected_names.contains(message.name()) {
                assert!(
                    canonical.contains(fqn.as_str()),
                    "parallel contract representation is forbidden: {fqn}"
                );
            }
            assert_no_parallel_nested_message(message, &protected_names, &fqn);
        }
    }
}

fn assert_no_parallel_nested_message(
    message: &DescriptorProto,
    protected_names: &BTreeSet<&str>,
    fqn: &str,
) {
    for nested in &message.nested_type {
        let nested_fqn = format!("{fqn}.{}", nested.name());
        assert!(
            !protected_names.contains(nested.name()),
            "parallel nested contract representation is forbidden: {nested_fqn}"
        );
        assert_no_parallel_nested_message(nested, protected_names, &nested_fqn);
    }
}

fn assert_shared_types(messages: &BTreeMap<String, &DescriptorProto>) {
    assert_fields(
        messages,
        "ficant.core.v1.Ulid",
        &[ExpectedField::scalar("value", Type::String)],
    );
    assert_fields(
        messages,
        "ficant.core.v1.VersionRef",
        &[
            ExpectedField::message("id", ".ficant.core.v1.Ulid"),
            ExpectedField::scalar("version", Type::Uint64),
        ],
    );
    assert_fields(
        messages,
        "ficant.core.v1.Sha256",
        &[ExpectedField::scalar("value", Type::Bytes)],
    );
    assert_fields(
        messages,
        "ficant.core.v1.OwnerRef",
        &[
            ExpectedField::message("tenant_id", ".ficant.core.v1.Ulid"),
            ExpectedField::message("owner_id", ".ficant.core.v1.Ulid"),
        ],
    );
    assert_fields(
        messages,
        "ficant.core.v1.UnitRef",
        &[
            ExpectedField::message("unit_id", ".ficant.core.v1.Ulid"),
            ExpectedField::scalar("version", Type::Uint64),
        ],
    );
    assert_fields(
        messages,
        "ficant.core.v1.LineageRef",
        &[
            ExpectedField::message("object_id", ".ficant.core.v1.Ulid"),
            ExpectedField::scalar("version", Type::Uint64),
            ExpectedField::message("content_hash", ".ficant.core.v1.Sha256"),
        ],
    );
    assert_fields(
        messages,
        "ficant.core.v1.DecimalValue",
        &[
            ExpectedField::scalar("coefficient", Type::String),
            ExpectedField::scalar("scale", Type::Uint32),
            ExpectedField::message("unit", ".ficant.core.v1.UnitRef"),
        ],
    );
    let decimal = messages
        .get("ficant.core.v1.DecimalValue")
        .expect("DecimalValue must exist");
    assert_eq!(
        decimal
            .field
            .iter()
            .map(|field| field.name())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["coefficient", "scale", "unit"]),
        "DecimalValue must have exactly coefficient + scale + UnitRef"
    );
    assert_fields(
        messages,
        "ficant.core.v1.MarketTime",
        &[
            ExpectedField::message("instant", ".google.protobuf.Timestamp"),
            ExpectedField::scalar("market_timezone", Type::String),
            ExpectedField::scalar("local_trading_date", Type::String),
        ],
    );
    assert_fields(
        messages,
        "ficant.core.v1.PageRequest",
        &[
            ExpectedField::scalar("page_size", Type::Uint32),
            ExpectedField::scalar("cursor", Type::String),
        ],
    );
    assert_fields(
        messages,
        "ficant.core.v1.PageResponse",
        &[ExpectedField::scalar("next_cursor", Type::String)],
    );
    assert_fields(
        messages,
        "ficant.core.v1.FieldViolation",
        &[
            ExpectedField::scalar("field", Type::String),
            ExpectedField::scalar("description", Type::String),
        ],
    );
    assert_fields(
        messages,
        "ficant.core.v1.ErrorDetail",
        &[
            ExpectedField::enumeration("code", ".ficant.core.v1.ErrorCode"),
            ExpectedField::scalar("message", Type::String),
            ExpectedField::scalar("trace_id", Type::String),
            ExpectedField::scalar("retryable", Type::Bool),
            ExpectedField::scalar("resource_ref", Type::String),
            ExpectedField::repeated_message("field_violations", ".ficant.core.v1.FieldViolation"),
        ],
    );
}

fn assert_subject_contracts(messages: &BTreeMap<String, &DescriptorProto>) {
    let id = ".ficant.core.v1.Ulid";
    let version = ".ficant.core.v1.VersionRef";
    let decimal = ".ficant.core.v1.DecimalValue";
    let timestamp = ".google.protobuf.Timestamp";
    let error = ".ficant.core.v1.ErrorDetail";

    assert_fields(
        messages,
        "ficant.core.v1.Subject",
        &[
            ExpectedField::message("subject_id", id),
            ExpectedField::scalar("display_name", Type::String),
        ],
    );
    assert_fields(
        messages,
        "ficant.core.v1.SubjectVersion",
        &[
            ExpectedField::message("subject_ref", version),
            ExpectedField::message("access_set", ".ficant.core.v1.AccessSet"),
            ExpectedField::enumeration("funding_tier", ".ficant.core.v1.FundingTier"),
            ExpectedField::message("tax_treatment", ".ficant.core.v1.TaxTreatment"),
            ExpectedField::scalar("assessment_mechanism", Type::String),
            ExpectedField::scalar("liability_profile", Type::String),
            ExpectedField::message("constraint_set_ref", ".ficant.core.v1.ConstraintSetRef"),
        ],
    );
    assert_fields(
        messages,
        "ficant.core.v1.AccessSet",
        &[
            ExpectedField::repeated_scalar("market_codes", Type::String),
            ExpectedField::repeated_scalar("tool_codes", Type::String),
        ],
    );
    assert_fields(
        messages,
        "ficant.core.v1.TaxTreatment",
        &[
            ExpectedField::scalar("value_added_tax_profile", Type::String),
            ExpectedField::scalar("income_tax_profile", Type::String),
        ],
    );
    assert_fields(
        messages,
        "ficant.core.v1.ConstraintSetRef",
        &[ExpectedField::message("ref", version)],
    );
    assert_fields(
        messages,
        "ficant.core.v1.SubjectRecord",
        &[
            ExpectedField::message("subject", ".ficant.core.v1.Subject"),
            ExpectedField::message("subject_version", ".ficant.core.v1.SubjectVersion"),
        ],
    );
    assert_fields(
        messages,
        "ficant.core.v1.SubjectStateSnapshot",
        &[
            ExpectedField::message("snapshot_id", id),
            ExpectedField::message("subject_ref", version),
            ExpectedField::message("net_capital", decimal),
            ExpectedField::repeated_message("limit_ceilings", ".ficant.core.v1.LimitCeiling"),
            ExpectedField::message("observed_at", timestamp),
            ExpectedField::message("visible_at", timestamp),
            ExpectedField::scalar("market_timezone", Type::String),
        ],
    );
    assert_fields(
        messages,
        "ficant.core.v1.LimitCeiling",
        &[
            ExpectedField::scalar("limit_code", Type::String),
            ExpectedField::message("ceiling", decimal),
        ],
    );
    assert_fields(
        messages,
        "ficant.core.v1.RegisterSubjectRequest",
        &[
            ExpectedField::message("subject", ".ficant.core.v1.Subject"),
            ExpectedField::message("subject_version", ".ficant.core.v1.SubjectVersion"),
        ],
    );
    assert_fields(
        messages,
        "ficant.core.v1.RegisterSubjectResponse",
        &[
            ExpectedField::oneof_message("subject", ".ficant.core.v1.SubjectRecord", "result"),
            ExpectedField::oneof_message("error", error, "result"),
        ],
    );
    assert_fields(
        messages,
        "ficant.core.v1.GetSubjectRequest",
        &[ExpectedField::message("subject_ref", version)],
    );
    assert_fields(
        messages,
        "ficant.core.v1.GetSubjectResponse",
        &[
            ExpectedField::oneof_message("subject", ".ficant.core.v1.SubjectRecord", "result"),
            ExpectedField::oneof_message("error", error, "result"),
        ],
    );
    assert_fields(
        messages,
        "ficant.core.v1.RegisterSubjectStateRequest",
        &[ExpectedField::message(
            "snapshot",
            ".ficant.core.v1.SubjectStateSnapshot",
        )],
    );
    assert_fields(
        messages,
        "ficant.core.v1.RegisterSubjectStateResponse",
        &[
            ExpectedField::oneof_message(
                "snapshot",
                ".ficant.core.v1.SubjectStateSnapshot",
                "result",
            ),
            ExpectedField::oneof_message("error", error, "result"),
        ],
    );
    assert_fields(
        messages,
        "ficant.core.v1.GetSubjectStateRequest",
        &[
            ExpectedField::message("snapshot_id", id),
            ExpectedField::message("knowledge_at", timestamp),
        ],
    );
    assert_fields(
        messages,
        "ficant.core.v1.GetSubjectStateResponse",
        &[
            ExpectedField::oneof_message(
                "snapshot",
                ".ficant.core.v1.SubjectStateSnapshot",
                "result",
            ),
            ExpectedField::oneof_message("error", error, "result"),
        ],
    );
    assert_fields(
        messages,
        "ficant.rates.v1.ResultMetadata",
        &[
            ExpectedField::scalar("schema_id", Type::String),
            ExpectedField::scalar("engine_id", Type::String),
            ExpectedField::scalar("engine_version", Type::String),
            ExpectedField::message("algorithm", ".ficant.rates.v1.AlgorithmBinding"),
            ExpectedField::message("subject_ref", version),
        ],
    );
    assert_fields(
        messages,
        "ficant.rates.v1.AnalyzeBondRequest",
        &[
            ExpectedField::message("context", ".ficant.rates.v1.AnalysisContext"),
            ExpectedField::message("bond", ".ficant.rates.v1.ObjectBinding"),
            ExpectedField::message("valuation_at", ".ficant.core.v1.MarketTime"),
            ExpectedField::scalar("settlement_date", Type::String),
            ExpectedField::enumeration(
                "calendar_requirement",
                ".ficant.rates.v1.CalendarRequirement",
            ),
            ExpectedField::message("calendar", ".ficant.rates.v1.CalendarBinding"),
            ExpectedField::message("terms", ".ficant.rates.v1.BondTerms"),
            ExpectedField::oneof_message(
                "yield_to_maturity",
                ".ficant.core.v1.DecimalValue",
                "input",
            ),
            ExpectedField::oneof_message("clean_price", ".ficant.core.v1.DecimalValue", "input"),
            ExpectedField::message("subject_ref", version),
        ],
    );
}

fn assert_phase1_objects(messages: &BTreeMap<String, &DescriptorProto>) {
    let id = ".ficant.core.v1.Ulid";
    let version = ".ficant.core.v1.VersionRef";
    let owner = ".ficant.core.v1.OwnerRef";
    let decimal = ".ficant.core.v1.DecimalValue";
    let time = ".ficant.core.v1.MarketTime";
    let hash = ".ficant.core.v1.Sha256";
    let lineage = ".ficant.core.v1.LineageRef";

    let specs: [(&str, &[ExpectedField]); 17] = [
        (
            "ficant.market.v1.Instrument",
            &[
                ExpectedField::message("instrument_id", id),
                ExpectedField::scalar("version", Type::Uint64),
                ExpectedField::message("owner", owner),
                ExpectedField::enumeration("kind", ".ficant.market.v1.InstrumentKind"),
                ExpectedField::scalar("market", Type::String),
                ExpectedField::scalar("symbol", Type::String),
                ExpectedField::message("currency", ".ficant.core.v1.UnitRef"),
                ExpectedField::message("calendar", version),
            ],
        ),
        (
            "ficant.market.v1.Bond",
            &[
                ExpectedField::message("instrument", version),
                ExpectedField::scalar("issue_date", Type::String),
                ExpectedField::scalar("maturity_date", Type::String),
                ExpectedField::message("face_value", decimal),
            ],
        ),
        (
            "ficant.market.v1.FuturesContract",
            &[
                ExpectedField::message("instrument", version),
                ExpectedField::message("last_trade_time", time),
                ExpectedField::message("expiry_time", time),
                ExpectedField::message("settlement_time", time),
                ExpectedField::message("multiplier", decimal),
                ExpectedField::message("rule_pack", version),
            ],
        ),
        (
            "ficant.market.v1.Cashflow",
            &[
                ExpectedField::message("cashflow_id", id),
                ExpectedField::message("bond", version),
                ExpectedField::message("payment_time", time),
                ExpectedField::message("amount", decimal),
                ExpectedField::message("owner", owner),
                ExpectedField::message("source", ".ficant.market.v1.FactSource"),
                ExpectedField::message("supersedes_id", id),
                ExpectedField::scalar("schedule_id", Type::String),
                ExpectedField::scalar("sequence", Type::Uint64),
            ],
        ),
        (
            "ficant.market.v1.Calendar",
            &[
                ExpectedField::message("calendar_id", id),
                ExpectedField::scalar("version", Type::Uint64),
                ExpectedField::message("owner", owner),
                ExpectedField::scalar("market", Type::String),
                ExpectedField::scalar("market_timezone", Type::String),
                ExpectedField::message("effective_from", time),
                ExpectedField::message("effective_to", time),
                ExpectedField::repeated_message("sessions", ".ficant.market.v1.CalendarSession"),
            ],
        ),
        (
            "ficant.market.v1.Unit",
            &[
                ExpectedField::message("unit_id", id),
                ExpectedField::scalar("version", Type::Uint64),
                ExpectedField::message("owner", owner),
                ExpectedField::scalar("code", Type::String),
                ExpectedField::scalar("dimension", Type::String),
                ExpectedField::scalar("scale", Type::Uint32),
                ExpectedField::scalar("precision", Type::Uint32),
            ],
        ),
        (
            "ficant.market.v1.Quote",
            &[
                ExpectedField::message("quote_id", id),
                ExpectedField::message("instrument", version),
                ExpectedField::message("owner", owner),
                ExpectedField::message("source", ".ficant.market.v1.FactSource"),
                ExpectedField::message("observed_at", time),
                ExpectedField::message("received_at", time),
                ExpectedField::message("bid", decimal),
                ExpectedField::message("ask", decimal),
                ExpectedField::message("supersedes_id", id),
            ],
        ),
        (
            "ficant.market.v1.Trade",
            &[
                ExpectedField::message("trade_id", id),
                ExpectedField::message("instrument", version),
                ExpectedField::message("owner", owner),
                ExpectedField::message("source", ".ficant.market.v1.FactSource"),
                ExpectedField::message("executed_at", time),
                ExpectedField::message("price", decimal),
                ExpectedField::message("quantity", decimal),
                ExpectedField::message("supersedes_id", id),
            ],
        ),
        (
            "ficant.market.v1.Valuation",
            &[
                ExpectedField::message("valuation_id", id),
                ExpectedField::message("instrument", version),
                ExpectedField::message("owner", owner),
                ExpectedField::message("source", ".ficant.market.v1.FactSource"),
                ExpectedField::message("valuation_at", time),
                ExpectedField::scalar("method", Type::String),
                ExpectedField::message("rule_pack", version),
                ExpectedField::repeated_message("values", decimal),
                ExpectedField::message("supersedes_id", id),
            ],
        ),
        (
            "ficant.market.v1.CurveSnapshot",
            &[
                ExpectedField::message("curve_snapshot_id", id),
                ExpectedField::message("owner", owner),
                ExpectedField::message("as_of", time),
                ExpectedField::message("currency", ".ficant.core.v1.UnitRef"),
                ExpectedField::scalar("curve_kind", Type::String),
                ExpectedField::message("calendar", version),
                ExpectedField::message("rule_pack", version),
                ExpectedField::scalar("point_schema", Type::String),
                ExpectedField::message("content_hash", hash),
                ExpectedField::repeated_message("lineage", lineage),
            ],
        ),
        (
            "ficant.market.v1.MarketRulePack",
            &[
                ExpectedField::message("rule_pack_id", id),
                ExpectedField::scalar("version", Type::Uint64),
                ExpectedField::message("owner", owner),
                ExpectedField::scalar("market", Type::String),
                ExpectedField::scalar("rule_type", Type::String),
                ExpectedField::scalar("source", Type::String),
                ExpectedField::message("effective_from", time),
                ExpectedField::message("effective_to", time),
                ExpectedField::enumeration(
                    "verification_status",
                    ".ficant.market.v1.VerificationStatus",
                ),
                ExpectedField::message("content_hash", hash),
            ],
        ),
        (
            "ficant.research.v1.DataSnapshot",
            &[
                ExpectedField::message("data_snapshot_id", id),
                ExpectedField::message("owner", owner),
                ExpectedField::message("visible_at", time),
                ExpectedField::message("as_of", time),
                ExpectedField::message("schema_hash", hash),
                ExpectedField::message("manifest_hash", hash),
                ExpectedField::message("blob_content_hash", hash),
                ExpectedField::repeated_message("lineage", lineage),
            ],
        ),
        (
            "ficant.research.v1.UniverseSnapshot",
            &[
                ExpectedField::message("universe_snapshot_id", id),
                ExpectedField::message("owner", owner),
                ExpectedField::repeated_message("instrument_versions", version),
                ExpectedField::message("filter_digest", hash),
                ExpectedField::message("content_hash", hash),
                ExpectedField::repeated_message("lineage", lineage),
            ],
        ),
        (
            "ficant.research.v1.ExperimentRun",
            &[
                ExpectedField::message("experiment_run_id", id),
                ExpectedField::message("owner", owner),
                ExpectedField::message("data_snapshot", lineage),
                ExpectedField::message("universe_snapshot", lineage),
                ExpectedField::repeated_message("rule_packs", version),
                ExpectedField::message("runtime_image_digest", hash),
                ExpectedField::message("parameters_hash", hash),
                ExpectedField::scalar("seed", Type::Uint64),
                ExpectedField::enumeration("state", ".ficant.research.v1.RunState"),
                ExpectedField::scalar("revision", Type::Uint64),
            ],
        ),
        (
            "ficant.research.v1.Artifact",
            &[
                ExpectedField::message("artifact_id", id),
                ExpectedField::message("owner", owner),
                ExpectedField::enumeration("kind", ".ficant.research.v1.ArtifactKind"),
                ExpectedField::scalar("media_type", Type::String),
                ExpectedField::message("content_hash", hash),
                ExpectedField::scalar("blob_size", Type::Uint64),
                ExpectedField::repeated_message("lineage", lineage),
            ],
        ),
        (
            "ficant.research.v1.SignalSet",
            &[
                ExpectedField::message("signal_set_id", id),
                ExpectedField::message("owner", owner),
                ExpectedField::message("artifact", lineage),
                ExpectedField::message("experiment_run_id", id),
                ExpectedField::message("data_snapshot", lineage),
                ExpectedField::message("universe_snapshot", lineage),
                ExpectedField::repeated_message("rule_packs", version),
                ExpectedField::repeated_message("input_artifacts", lineage),
                ExpectedField::message("valid_from", time),
                ExpectedField::message("valid_to", time),
            ],
        ),
        (
            "ficant.research.v1.RunJournal",
            &[
                ExpectedField::message("journal_event_id", id),
                ExpectedField::message("run_id", id),
                ExpectedField::scalar("sequence", Type::Uint64),
                ExpectedField::enumeration("event_type", ".ficant.research.v1.JournalEventType"),
                ExpectedField::message("occurred_at", time),
                ExpectedField::scalar("payload_type", Type::String),
                ExpectedField::scalar("payload_schema", Type::String),
                ExpectedField::scalar("payload", Type::Bytes),
                ExpectedField::message("prev_hash", hash),
                ExpectedField::message("event_hash", hash),
            ],
        ),
    ];

    for (message, fields) in specs {
        assert_fields(messages, message, fields);
    }
}

fn assert_platform_contracts(messages: &BTreeMap<String, &DescriptorProto>) {
    assert_fields(
        messages,
        "ficant.app.v1.SafeError",
        &[
            ExpectedField::enumeration("code", ".ficant.app.v1.ErrorCode"),
            ExpectedField::scalar("safe_message", Type::String),
            ExpectedField::scalar("trace_id", Type::String),
            ExpectedField::scalar("retryable", Type::Bool),
        ],
    );
    assert_fields(
        messages,
        "ficant.app.v1.CspDirective",
        &[
            ExpectedField::scalar("name", Type::String),
            ExpectedField::repeated_scalar("values", Type::String),
        ],
    );
    assert_fields(
        messages,
        "ficant.app.v1.AppDescriptor",
        &[
            ExpectedField::scalar("app_id", Type::String),
            ExpectedField::scalar("display_name", Type::String),
            ExpectedField::scalar("entrypoint", Type::String),
            ExpectedField::scalar("allowed_origin", Type::String),
            ExpectedField::repeated_scalar("capabilities", Type::String),
        ],
    );
    assert_fields(
        messages,
        "ficant.app.v1.AppRegistry",
        &[ExpectedField::repeated_message(
            "apps",
            ".ficant.app.v1.AppDescriptor",
        )],
    );
    assert_fields(
        messages,
        "ficant.app.v1.Session",
        &[
            ExpectedField::scalar("session_id", Type::String),
            ExpectedField::scalar("subject_id", Type::String),
            ExpectedField::repeated_scalar("scopes", Type::String),
            ExpectedField::message("issued_at", ".google.protobuf.Timestamp"),
            ExpectedField::message("expires_at", ".google.protobuf.Timestamp"),
        ],
    );
    assert_fields(
        messages,
        "ficant.app.v1.AppLaunchGrant",
        &[
            ExpectedField::scalar("app_id", Type::String),
            ExpectedField::scalar("entrypoint", Type::String),
            ExpectedField::scalar("allowed_origin", Type::String),
            ExpectedField::repeated_scalar("scopes", Type::String),
            ExpectedField::message("issued_at", ".google.protobuf.Timestamp"),
            ExpectedField::message("expires_at", ".google.protobuf.Timestamp"),
            ExpectedField::scalar("launch_credential", Type::Bytes),
            ExpectedField::repeated_message("csp_directives", ".ficant.app.v1.CspDirective"),
            ExpectedField::repeated_scalar("sandbox_tokens", Type::String),
        ],
    );
    assert_fields(
        messages,
        "ficant.app.v1.SessionRevocation",
        &[ExpectedField::message(
            "revoked_at",
            ".google.protobuf.Timestamp",
        )],
    );
    assert_fields(
        messages,
        "ficant.app.v1.AppLaunchRevocation",
        &[
            ExpectedField::scalar("app_id", Type::String),
            ExpectedField::message("revoked_at", ".google.protobuf.Timestamp"),
        ],
    );

    for empty_request in [
        "ficant.app.v1.GetAppRegistryRequest",
        "ficant.app.v1.GetCurrentSessionRequest",
        "ficant.app.v1.RefreshSessionRequest",
        "ficant.app.v1.RevokeSessionRequest",
    ] {
        assert_fields(messages, empty_request, &[]);
    }
    for app_request in [
        "ficant.app.v1.AuthorizeAppLaunchRequest",
        "ficant.app.v1.RefreshAppLaunchRequest",
        "ficant.app.v1.RevokeAppLaunchRequest",
    ] {
        assert_fields(
            messages,
            app_request,
            &[ExpectedField::scalar("app_id", Type::String)],
        );
    }

    assert_fields(
        messages,
        "ficant.app.v1.GetAppRegistryResponse",
        &[
            ExpectedField::oneof_message("registry", ".ficant.app.v1.AppRegistry", "result"),
            ExpectedField::oneof_message("error", ".ficant.app.v1.SafeError", "result"),
        ],
    );
    for response in [
        "ficant.app.v1.GetCurrentSessionResponse",
        "ficant.app.v1.RefreshSessionResponse",
    ] {
        assert_fields(
            messages,
            response,
            &[
                ExpectedField::oneof_message("session", ".ficant.app.v1.Session", "result"),
                ExpectedField::oneof_message("error", ".ficant.app.v1.SafeError", "result"),
            ],
        );
    }
    assert_fields(
        messages,
        "ficant.app.v1.RevokeSessionResponse",
        &[
            ExpectedField::oneof_message(
                "revocation",
                ".ficant.app.v1.SessionRevocation",
                "result",
            ),
            ExpectedField::oneof_message("error", ".ficant.app.v1.SafeError", "result"),
        ],
    );
    assert_fields(
        messages,
        "ficant.app.v1.AppLaunchAuthorizationResponse",
        &[
            ExpectedField::oneof_message("grant", ".ficant.app.v1.AppLaunchGrant", "result"),
            ExpectedField::oneof_message("error", ".ficant.app.v1.SafeError", "result"),
        ],
    );
    assert_fields(
        messages,
        "ficant.app.v1.RevokeAppLaunchResponse",
        &[
            ExpectedField::oneof_message(
                "revocation",
                ".ficant.app.v1.AppLaunchRevocation",
                "result",
            ),
            ExpectedField::oneof_message("error", ".ficant.app.v1.SafeError", "result"),
        ],
    );
}

fn assert_phase4_contracts(messages: &BTreeMap<String, &DescriptorProto>) {
    let id = ".ficant.core.v1.Ulid";
    let owner = ".ficant.core.v1.OwnerRef";
    let hash = ".ficant.core.v1.Sha256";
    let lineage = ".ficant.core.v1.LineageRef";
    let typed_value = ".ficant.research.v1.TypedValue";
    let execution = ".ficant.research.v1.ExecutionInstanceIdentity";

    let specs: &[(&str, &[ExpectedField])] = &[
        (
            "ficant.research.v1.TypedValue",
            &[
                ExpectedField::scalar("type_id", Type::String),
                ExpectedField::scalar("type_version", Type::Uint64),
                ExpectedField::message("schema_hash", hash),
            ],
        ),
        (
            "ficant.research.v1.PortType",
            &[
                ExpectedField::scalar("port_name", Type::String),
                ExpectedField::message("value_type", typed_value),
            ],
        ),
        (
            "ficant.research.v1.NodePermissions",
            &[
                ExpectedField::scalar("network", Type::Bool),
                ExpectedField::scalar("database", Type::Bool),
                ExpectedField::enumeration(
                    "filesystem",
                    ".ficant.research.v1.FilesystemPermission",
                ),
            ],
        ),
        (
            "ficant.research.v1.ResourceLimits",
            &[
                ExpectedField::scalar("cpu_cores", Type::Uint32),
                ExpectedField::scalar("memory_mb", Type::Uint32),
                ExpectedField::scalar("timeout_seconds", Type::Uint32),
            ],
        ),
        (
            "ficant.research.v1.ResearchNodeContract",
            &[
                ExpectedField::scalar("contract_id", Type::String),
                ExpectedField::scalar("contract_version", Type::Uint64),
                ExpectedField::repeated_message("input_types", ".ficant.research.v1.PortType"),
                ExpectedField::repeated_message("output_types", ".ficant.research.v1.PortType"),
                ExpectedField::message("state_schema", hash),
                ExpectedField::message("parameter_schema", hash),
                ExpectedField::enumeration(
                    "determinism_class",
                    ".ficant.research.v1.DeterminismClass",
                ),
                ExpectedField::message("permissions", ".ficant.research.v1.NodePermissions"),
                ExpectedField::message("resource_limits", ".ficant.research.v1.ResourceLimits"),
                ExpectedField::repeated_scalar("required_invariants", Type::String),
                ExpectedField::message("digest", hash),
            ],
        ),
        (
            "ficant.research.v1.ExternalInputDeclaration",
            &[
                ExpectedField::scalar("input_id", Type::String),
                ExpectedField::message("value_type", typed_value),
            ],
        ),
        (
            "ficant.research.v1.ResearchNode",
            &[
                ExpectedField::message("node_id", id),
                ExpectedField::message("contract", ".ficant.research.v1.ResearchNodeContract"),
                ExpectedField::message("parameters_hash", hash),
            ],
        ),
        (
            "ficant.research.v1.ResearchEdge",
            &[
                ExpectedField::message("from_node_id", id),
                ExpectedField::scalar("from_port", Type::String),
                ExpectedField::message("to_node_id", id),
                ExpectedField::scalar("to_port", Type::String),
            ],
        ),
        (
            "ficant.research.v1.ExternalInputBinding",
            &[
                ExpectedField::scalar("input_id", Type::String),
                ExpectedField::message("to_node_id", id),
                ExpectedField::scalar("to_port", Type::String),
            ],
        ),
        (
            "ficant.research.v1.ResearchGraph",
            &[
                ExpectedField::message("graph_id", id),
                ExpectedField::scalar("version", Type::Uint64),
                ExpectedField::message("owner", owner),
                ExpectedField::repeated_message("nodes", ".ficant.research.v1.ResearchNode"),
                ExpectedField::repeated_message("edges", ".ficant.research.v1.ResearchEdge"),
                ExpectedField::repeated_message(
                    "external_inputs",
                    ".ficant.research.v1.ExternalInputDeclaration",
                ),
                ExpectedField::repeated_message(
                    "external_input_bindings",
                    ".ficant.research.v1.ExternalInputBinding",
                ),
                ExpectedField::repeated_message("topological_order", id),
                ExpectedField::message("digest", hash),
            ],
        ),
        (
            "ficant.research.v1.NodeImplementationBinding",
            &[
                ExpectedField::message("node_id", id),
                ExpectedField::message("implementation_digest", hash),
            ],
        ),
        (
            "ficant.research.v1.RulePackBinding",
            &[
                ExpectedField::message("rule_pack_id", id),
                ExpectedField::scalar("version", Type::Uint64),
                ExpectedField::message("content_hash", hash),
            ],
        ),
        (
            "ficant.research.v1.ExecutionExternalInput",
            &[
                ExpectedField::scalar("input_id", Type::String),
                ExpectedField::message("value_type", typed_value),
                ExpectedField::message("resolved_artifact", lineage),
                ExpectedField::message("content_hash", hash),
            ],
        ),
        (
            "ficant.research.v1.UpstreamNodeOutput",
            &[
                ExpectedField::message("node_id", id),
                ExpectedField::scalar("port_name", Type::String),
            ],
        ),
        (
            "ficant.research.v1.NodeInputBinding",
            &[
                ExpectedField::message("node_id", id),
                ExpectedField::scalar("port_name", Type::String),
                ExpectedField::message("value_type", typed_value),
                ExpectedField::oneof_scalar("external_input_id", Type::String, "declared_source"),
                ExpectedField::oneof_message(
                    "upstream_output",
                    ".ficant.research.v1.UpstreamNodeOutput",
                    "declared_source",
                ),
                ExpectedField::message("resolved_artifact", lineage),
                ExpectedField::message("content_hash", hash),
            ],
        ),
        (
            "ficant.research.v1.ReproducibilityIdentity",
            &[
                ExpectedField::message("graph_digest", hash),
                ExpectedField::message("data_snapshot_hash", hash),
                ExpectedField::message("universe_snapshot_hash", hash),
                ExpectedField::message("parameters_hash", hash),
                ExpectedField::message("runtime_image_digest", hash),
                ExpectedField::message("environment_digest", hash),
                ExpectedField::scalar("seed", Type::Uint64),
                ExpectedField::repeated_message(
                    "rule_packs",
                    ".ficant.research.v1.RulePackBinding",
                ),
                ExpectedField::repeated_message(
                    "node_implementations",
                    ".ficant.research.v1.NodeImplementationBinding",
                ),
                ExpectedField::repeated_message(
                    "external_inputs",
                    ".ficant.research.v1.ExecutionExternalInput",
                ),
                ExpectedField::message("digest", hash),
            ],
        ),
        (
            "ficant.research.v1.ExecutionInstanceIdentity",
            &[
                ExpectedField::message("run_id", id),
                ExpectedField::message(
                    "reproducibility",
                    ".ficant.research.v1.ReproducibilityIdentity",
                ),
                ExpectedField::message("digest", hash),
            ],
        ),
        (
            "ficant.research.v1.NodeOutputBinding",
            &[
                ExpectedField::scalar("port_name", Type::String),
                ExpectedField::message("value_type", typed_value),
                ExpectedField::message("artifact", lineage),
                ExpectedField::message("content_hash", hash),
            ],
        ),
        (
            "ficant.research.v1.NodeOutputManifestContent",
            &[
                ExpectedField::message("reproducibility_digest", hash),
                ExpectedField::message("node_id", id),
                ExpectedField::message("node_contract_digest", hash),
                ExpectedField::message("implementation_digest", hash),
                ExpectedField::repeated_message("inputs", ".ficant.research.v1.NodeInputBinding"),
                ExpectedField::repeated_message("outputs", ".ficant.research.v1.NodeOutputBinding"),
                ExpectedField::message("manifest_hash", hash),
            ],
        ),
        (
            "ficant.research.v1.NodeOutputManifest",
            &[
                ExpectedField::message("execution", execution),
                ExpectedField::scalar("attempt", Type::Uint32),
                ExpectedField::message("content", ".ficant.research.v1.NodeOutputManifestContent"),
            ],
        ),
        (
            "ficant.research.v1.NodeCheckpoint",
            &[
                ExpectedField::message("execution", execution),
                ExpectedField::message("node_id", id),
                ExpectedField::scalar("attempt", Type::Uint32),
                ExpectedField::message("output_manifest", ".ficant.research.v1.NodeOutputManifest"),
                ExpectedField::scalar("journal_sequence", Type::Uint64),
                ExpectedField::message("journal_hash", hash),
                ExpectedField::message("checkpoint_hash", hash),
            ],
        ),
        (
            "ficant.research.v1.ReadNodeOutputRequest",
            &[
                ExpectedField::message("run_id", id),
                ExpectedField::message("node_id", id),
            ],
        ),
        (
            "ficant.research.v1.ObservedNodeOutput",
            &[
                ExpectedField::scalar("port_name", Type::String),
                ExpectedField::message("value_type", typed_value),
                ExpectedField::message("content_hash", hash),
                ExpectedField::scalar("payload", Type::Bytes),
            ],
        ),
        (
            "ficant.research.v1.ReadNodeOutputResponse",
            &[
                ExpectedField::message("manifest", ".ficant.research.v1.NodeOutputManifest"),
                ExpectedField::repeated_message(
                    "outputs",
                    ".ficant.research.v1.ObservedNodeOutput",
                ),
            ],
        ),
    ];

    for (message, fields) in specs {
        assert_fields(messages, message, fields);
    }
}

fn assert_query_contracts(messages: &BTreeMap<String, &DescriptorProto>) {
    let id = ".ficant.core.v1.Ulid";
    let page_request = ".ficant.core.v1.PageRequest";
    let page_response = ".ficant.core.v1.PageResponse";

    assert_fields(
        messages,
        "ficant.market.v1.MarketDefinition",
        &[
            ExpectedField::oneof_message(
                "instrument",
                ".ficant.market.v1.Instrument",
                "definition",
            ),
            ExpectedField::oneof_message("bond", ".ficant.market.v1.Bond", "definition"),
            ExpectedField::oneof_message(
                "futures_contract",
                ".ficant.market.v1.FuturesContract",
                "definition",
            ),
            ExpectedField::oneof_message("calendar", ".ficant.market.v1.Calendar", "definition"),
            ExpectedField::oneof_message("unit", ".ficant.market.v1.Unit", "definition"),
            ExpectedField::oneof_message(
                "market_rule_pack",
                ".ficant.market.v1.MarketRulePack",
                "definition",
            ),
        ],
    );
    assert_fields(
        messages,
        "ficant.market.v1.GetDefinitionVersionRequest",
        &[
            ExpectedField::message("definition_id", id),
            ExpectedField::scalar("version", Type::Uint64),
        ],
    );
    assert_fields(
        messages,
        "ficant.market.v1.GetDefinitionVersionResponse",
        &[ExpectedField::message(
            "definition",
            ".ficant.market.v1.MarketDefinition",
        )],
    );
    assert_fields(
        messages,
        "ficant.market.v1.ResolveDefinitionAsOfRequest",
        &[
            ExpectedField::message("definition_id", id),
            ExpectedField::message("as_of", ".google.protobuf.Timestamp"),
        ],
    );
    assert_fields(
        messages,
        "ficant.market.v1.ResolveDefinitionAsOfResponse",
        &[ExpectedField::message(
            "definition",
            ".ficant.market.v1.MarketDefinition",
        )],
    );
    assert_fields(
        messages,
        "ficant.market.v1.ListDefinitionVersionsRequest",
        &[
            ExpectedField::message("definition_id", id),
            ExpectedField::message("page", page_request),
        ],
    );
    assert_fields(
        messages,
        "ficant.market.v1.ListDefinitionVersionsResponse",
        &[
            ExpectedField::repeated_message("definitions", ".ficant.market.v1.MarketDefinition"),
            ExpectedField::message("page", page_response),
        ],
    );

    assert_fields(
        messages,
        "ficant.market.v1.MarketFact",
        &[
            ExpectedField::oneof_message("cashflow", ".ficant.market.v1.Cashflow", "fact"),
            ExpectedField::oneof_message("quote", ".ficant.market.v1.Quote", "fact"),
            ExpectedField::oneof_message("trade", ".ficant.market.v1.Trade", "fact"),
            ExpectedField::oneof_message("valuation", ".ficant.market.v1.Valuation", "fact"),
        ],
    );
    assert_fields(
        messages,
        "ficant.market.v1.QueryInstrumentFactsRequest",
        &[
            ExpectedField::message("instrument", ".ficant.core.v1.VersionRef"),
            ExpectedField::message("from", ".ficant.core.v1.MarketTime"),
            ExpectedField::message("to", ".ficant.core.v1.MarketTime"),
            ExpectedField::message("page", page_request),
        ],
    );
    assert_fields(
        messages,
        "ficant.market.v1.QueryInstrumentFactsResponse",
        &[
            ExpectedField::repeated_message("facts", ".ficant.market.v1.MarketFact"),
            ExpectedField::message("page", page_response),
        ],
    );
    assert_fields(
        messages,
        "ficant.market.v1.GetCurveSnapshotRequest",
        &[ExpectedField::message("curve_snapshot_id", id)],
    );
    assert_fields(
        messages,
        "ficant.market.v1.GetCurveSnapshotResponse",
        &[ExpectedField::message(
            "curve_snapshot",
            ".ficant.market.v1.CurveSnapshot",
        )],
    );

    assert_fields(
        messages,
        "ficant.research.v1.GetSignalSetRequest",
        &[ExpectedField::message("signal_set_id", id)],
    );
    assert_fields(
        messages,
        "ficant.research.v1.GetSignalSetResponse",
        &[ExpectedField::message(
            "signal_set",
            ".ficant.research.v1.SignalSet",
        )],
    );
    for (message, id_field) in [
        (
            "ficant.research.v1.ReadArtifactLineageRequest",
            "artifact_id",
        ),
        (
            "ficant.research.v1.ReadSignalSetLineageRequest",
            "signal_set_id",
        ),
    ] {
        assert_fields(
            messages,
            message,
            &[
                ExpectedField::message(id_field, id),
                ExpectedField::message("page", page_request),
            ],
        );
    }
    for message in [
        "ficant.research.v1.ReadArtifactLineageResponse",
        "ficant.research.v1.ReadSignalSetLineageResponse",
    ] {
        assert_fields(
            messages,
            message,
            &[
                ExpectedField::repeated_message("lineage", ".ficant.core.v1.LineageRef"),
                ExpectedField::message("page", page_response),
            ],
        );
    }
}

fn assert_enum(
    enums: &BTreeMap<String, &EnumDescriptorProto>,
    enum_name: &str,
    expected_values: &[(&str, i32)],
) {
    let enumeration = enums
        .get(enum_name)
        .unwrap_or_else(|| panic!("missing enum {enum_name}"));
    let actual = enumeration
        .value
        .iter()
        .map(|value| (value.name().to_owned(), value.number()))
        .collect::<BTreeMap<_, _>>();
    let expected = expected_values
        .iter()
        .map(|(name, number)| ((*name).to_owned(), *number))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(actual, expected, "{enum_name} name/value set drifted");
}

fn assert_domain_enums(enums: &BTreeMap<String, &EnumDescriptorProto>) {
    assert_enum(
        enums,
        "ficant.core.v1.ErrorCode",
        &[
            ("ERROR_CODE_UNSPECIFIED", 0),
            ("ERROR_CODE_VALIDATION_FAILED", 1),
            ("ERROR_CODE_NOT_FOUND", 2),
            ("ERROR_CODE_ALREADY_EXISTS", 3),
            ("ERROR_CODE_VERSION_CONFLICT", 4),
            ("ERROR_CODE_CONCURRENCY_CONFLICT", 5),
            ("ERROR_CODE_IMMUTABLE_VIOLATION", 6),
            ("ERROR_CODE_LINEAGE_INCOMPLETE", 7),
            ("ERROR_CODE_HASH_MISMATCH", 8),
            ("ERROR_CODE_UNAUTHENTICATED", 9),
            ("ERROR_CODE_FORBIDDEN", 10),
            ("ERROR_CODE_STORAGE_UNAVAILABLE", 11),
            ("ERROR_CODE_INTERNAL", 12),
        ],
    );
    assert_enum(
        enums,
        "ficant.core.v1.FundingTier",
        &[
            ("FUNDING_TIER_UNSPECIFIED", 0),
            ("FUNDING_TIER_DR_AVAILABLE", 1),
            ("FUNDING_TIER_R_ONLY", 2),
        ],
    );
    assert_enum(
        enums,
        "ficant.market.v1.InstrumentKind",
        &[
            ("INSTRUMENT_KIND_UNSPECIFIED", 0),
            ("INSTRUMENT_KIND_BOND", 1),
            ("INSTRUMENT_KIND_FUTURES", 2),
            ("INSTRUMENT_KIND_OTHER", 3),
        ],
    );
    assert_enum(
        enums,
        "ficant.market.v1.VerificationStatus",
        &[
            ("VERIFICATION_STATUS_UNSPECIFIED", 0),
            ("VERIFICATION_STATUS_UNVERIFIED", 1),
            ("VERIFICATION_STATUS_VERIFIED", 2),
            ("VERIFICATION_STATUS_REJECTED", 3),
        ],
    );
    assert_enum(
        enums,
        "ficant.research.v1.ArtifactKind",
        &[
            ("ARTIFACT_KIND_UNSPECIFIED", 0),
            ("ARTIFACT_KIND_GENERIC", 1),
            ("ARTIFACT_KIND_CURVE_SNAPSHOT", 2),
            ("ARTIFACT_KIND_DATA_SNAPSHOT", 3),
            ("ARTIFACT_KIND_UNIVERSE_SNAPSHOT", 4),
            ("ARTIFACT_KIND_SIGNAL_SET", 5),
        ],
    );
    assert_enum(
        enums,
        "ficant.research.v1.RunState",
        &[
            ("RUN_STATE_UNSPECIFIED", 0),
            ("RUN_STATE_CREATED", 1),
            ("RUN_STATE_RUNNING", 2),
            ("RUN_STATE_SUCCEEDED", 3),
            ("RUN_STATE_FAILED", 4),
            ("RUN_STATE_CANCELLED", 5),
        ],
    );
    assert_enum(
        enums,
        "ficant.research.v1.JournalEventType",
        &[
            ("JOURNAL_EVENT_TYPE_UNSPECIFIED", 0),
            ("JOURNAL_EVENT_TYPE_RUN_CREATED", 1),
            ("JOURNAL_EVENT_TYPE_RUN_STARTED", 2),
            ("JOURNAL_EVENT_TYPE_RUN_SUCCEEDED", 3),
            ("JOURNAL_EVENT_TYPE_RUN_FAILED", 4),
            ("JOURNAL_EVENT_TYPE_RUN_CANCELLED", 5),
            ("JOURNAL_EVENT_TYPE_ARTIFACT_PUBLISHED", 6),
            ("JOURNAL_EVENT_TYPE_SIGNAL_SET_PUBLISHED", 7),
            ("JOURNAL_EVENT_TYPE_NODE_STARTED", 8),
            ("JOURNAL_EVENT_TYPE_NODE_SUCCEEDED", 9),
            ("JOURNAL_EVENT_TYPE_NODE_FAILED", 10),
            ("JOURNAL_EVENT_TYPE_NODE_CHECKPOINTED", 11),
        ],
    );
    assert_enum(
        enums,
        "ficant.research.v1.DeterminismClass",
        &[
            ("DETERMINISM_CLASS_UNSPECIFIED", 0),
            ("DETERMINISM_CLASS_DETERMINISTIC", 1),
            ("DETERMINISM_CLASS_SEEDED", 2),
        ],
    );
    assert_enum(
        enums,
        "ficant.research.v1.FilesystemPermission",
        &[
            ("FILESYSTEM_PERMISSION_UNSPECIFIED", 0),
            ("FILESYSTEM_PERMISSION_NONE", 1),
            ("FILESYSTEM_PERMISSION_TEMPORARY_ONLY", 2),
        ],
    );
}

fn assert_exact_service(
    descriptor_set: &FileDescriptorSet,
    service_name: &str,
    expected_methods: &[ExpectedMethod],
) {
    let services = top_level_services(descriptor_set);
    let service = services
        .get(service_name)
        .unwrap_or_else(|| panic!("missing service {service_name}"));
    let actual = service
        .method
        .iter()
        .map(|method| {
            assert!(
                !method.client_streaming() && !method.server_streaming(),
                "{service_name}.{} must remain unary",
                method.name()
            );
            (
                method.name().to_owned(),
                method.input_type().to_owned(),
                method.output_type().to_owned(),
            )
        })
        .collect::<BTreeSet<_>>();
    let expected = expected_methods
        .iter()
        .map(|method| {
            (
                method.name.to_owned(),
                method.input.to_owned(),
                method.output.to_owned(),
            )
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        actual, expected,
        "{service_name} must expose the exact frozen method/input/output set"
    );
}

fn market_definition_methods() -> Vec<ExpectedMethod> {
    vec![
        ExpectedMethod::new(
            "AppendInstrument",
            ".ficant.market.v1.AppendInstrumentRequest",
            ".ficant.market.v1.AppendInstrumentResponse",
        ),
        ExpectedMethod::new(
            "AppendBond",
            ".ficant.market.v1.AppendBondRequest",
            ".ficant.market.v1.AppendBondResponse",
        ),
        ExpectedMethod::new(
            "AppendFuturesContract",
            ".ficant.market.v1.AppendFuturesContractRequest",
            ".ficant.market.v1.AppendFuturesContractResponse",
        ),
        ExpectedMethod::new(
            "AppendCalendar",
            ".ficant.market.v1.AppendCalendarRequest",
            ".ficant.market.v1.AppendCalendarResponse",
        ),
        ExpectedMethod::new(
            "AppendUnit",
            ".ficant.market.v1.AppendUnitRequest",
            ".ficant.market.v1.AppendUnitResponse",
        ),
        ExpectedMethod::new(
            "AppendMarketRulePack",
            ".ficant.market.v1.AppendMarketRulePackRequest",
            ".ficant.market.v1.AppendMarketRulePackResponse",
        ),
        ExpectedMethod::new(
            "GetDefinitionVersion",
            ".ficant.market.v1.GetDefinitionVersionRequest",
            ".ficant.market.v1.GetDefinitionVersionResponse",
        ),
        ExpectedMethod::new(
            "ResolveDefinitionAsOf",
            ".ficant.market.v1.ResolveDefinitionAsOfRequest",
            ".ficant.market.v1.ResolveDefinitionAsOfResponse",
        ),
        ExpectedMethod::new(
            "ListDefinitionVersions",
            ".ficant.market.v1.ListDefinitionVersionsRequest",
            ".ficant.market.v1.ListDefinitionVersionsResponse",
        ),
    ]
}

fn market_fact_methods() -> Vec<ExpectedMethod> {
    vec![
        ExpectedMethod::new(
            "AppendCashflow",
            ".ficant.market.v1.AppendCashflowRequest",
            ".ficant.market.v1.AppendCashflowResponse",
        ),
        ExpectedMethod::new(
            "AppendQuote",
            ".ficant.market.v1.AppendQuoteRequest",
            ".ficant.market.v1.AppendQuoteResponse",
        ),
        ExpectedMethod::new(
            "AppendTrade",
            ".ficant.market.v1.AppendTradeRequest",
            ".ficant.market.v1.AppendTradeResponse",
        ),
        ExpectedMethod::new(
            "AppendValuation",
            ".ficant.market.v1.AppendValuationRequest",
            ".ficant.market.v1.AppendValuationResponse",
        ),
        ExpectedMethod::new(
            "PublishCurveSnapshot",
            ".ficant.market.v1.PublishCurveSnapshotRequest",
            ".ficant.market.v1.PublishCurveSnapshotResponse",
        ),
        ExpectedMethod::new(
            "QueryInstrumentFacts",
            ".ficant.market.v1.QueryInstrumentFactsRequest",
            ".ficant.market.v1.QueryInstrumentFactsResponse",
        ),
        ExpectedMethod::new(
            "GetCurveSnapshot",
            ".ficant.market.v1.GetCurveSnapshotRequest",
            ".ficant.market.v1.GetCurveSnapshotResponse",
        ),
    ]
}

fn artifact_methods() -> Vec<ExpectedMethod> {
    vec![
        ExpectedMethod::new(
            "PublishArtifact",
            ".ficant.research.v1.PublishArtifactRequest",
            ".ficant.research.v1.PublishArtifactResponse",
        ),
        ExpectedMethod::new(
            "GetArtifact",
            ".ficant.research.v1.GetArtifactRequest",
            ".ficant.research.v1.GetArtifactResponse",
        ),
        ExpectedMethod::new(
            "PublishSignalSet",
            ".ficant.research.v1.PublishSignalSetRequest",
            ".ficant.research.v1.PublishSignalSetResponse",
        ),
        ExpectedMethod::new(
            "GetSignalSet",
            ".ficant.research.v1.GetSignalSetRequest",
            ".ficant.research.v1.GetSignalSetResponse",
        ),
        ExpectedMethod::new(
            "ReadArtifactLineage",
            ".ficant.research.v1.ReadArtifactLineageRequest",
            ".ficant.research.v1.ReadArtifactLineageResponse",
        ),
        ExpectedMethod::new(
            "ReadSignalSetLineage",
            ".ficant.research.v1.ReadSignalSetLineageRequest",
            ".ficant.research.v1.ReadSignalSetLineageResponse",
        ),
    ]
}

fn snapshot_methods() -> Vec<ExpectedMethod> {
    vec![
        ExpectedMethod::new(
            "PublishDataSnapshot",
            ".ficant.research.v1.PublishDataSnapshotRequest",
            ".ficant.research.v1.PublishDataSnapshotResponse",
        ),
        ExpectedMethod::new(
            "PublishUniverseSnapshot",
            ".ficant.research.v1.PublishUniverseSnapshotRequest",
            ".ficant.research.v1.PublishUniverseSnapshotResponse",
        ),
        ExpectedMethod::new(
            "GetSnapshot",
            ".ficant.research.v1.GetSnapshotRequest",
            ".ficant.research.v1.GetSnapshotResponse",
        ),
    ]
}

fn experiment_methods() -> Vec<ExpectedMethod> {
    vec![
        ExpectedMethod::new(
            "CreateRun",
            ".ficant.research.v1.CreateRunRequest",
            ".ficant.research.v1.CreateRunResponse",
        ),
        ExpectedMethod::new(
            "TransitionRun",
            ".ficant.research.v1.TransitionRunRequest",
            ".ficant.research.v1.TransitionRunResponse",
        ),
        ExpectedMethod::new(
            "GetRun",
            ".ficant.research.v1.GetRunRequest",
            ".ficant.research.v1.GetRunResponse",
        ),
        ExpectedMethod::new(
            "ReadRunJournal",
            ".ficant.research.v1.ReadRunJournalRequest",
            ".ficant.research.v1.ReadRunJournalResponse",
        ),
        ExpectedMethod::new(
            "SubmitGraphRun",
            ".ficant.research.v1.SubmitGraphRunRequest",
            ".ficant.research.v1.SubmitGraphRunResponse",
        ),
        ExpectedMethod::new(
            "GetGraphRun",
            ".ficant.research.v1.GetGraphRunRequest",
            ".ficant.research.v1.GetGraphRunResponse",
        ),
        ExpectedMethod::new(
            "ListNodeOutputManifests",
            ".ficant.research.v1.ListNodeOutputManifestsRequest",
            ".ficant.research.v1.ListNodeOutputManifestsResponse",
        ),
        ExpectedMethod::new(
            "TraceGraphOutput",
            ".ficant.research.v1.TraceGraphOutputRequest",
            ".ficant.research.v1.TraceGraphOutputResponse",
        ),
        ExpectedMethod::new(
            "ReadNodeOutput",
            ".ficant.research.v1.ReadNodeOutputRequest",
            ".ficant.research.v1.ReadNodeOutputResponse",
        ),
        ExpectedMethod::new(
            "CompareGraphRuns",
            ".ficant.research.v1.CompareGraphRunsRequest",
            ".ficant.research.v1.CompareGraphRunsResponse",
        ),
    ]
}

fn registry_methods() -> Vec<ExpectedMethod> {
    vec![
        ExpectedMethod::new(
            "RegisterSubject",
            ".ficant.core.v1.RegisterSubjectRequest",
            ".ficant.core.v1.RegisterSubjectResponse",
        ),
        ExpectedMethod::new(
            "GetSubject",
            ".ficant.core.v1.GetSubjectRequest",
            ".ficant.core.v1.GetSubjectResponse",
        ),
        ExpectedMethod::new(
            "RegisterSubjectState",
            ".ficant.core.v1.RegisterSubjectStateRequest",
            ".ficant.core.v1.RegisterSubjectStateResponse",
        ),
        ExpectedMethod::new(
            "GetSubjectState",
            ".ficant.core.v1.GetSubjectStateRequest",
            ".ficant.core.v1.GetSubjectStateResponse",
        ),
    ]
}

fn platform_methods() -> Vec<ExpectedMethod> {
    vec![
        ExpectedMethod::new(
            "GetAppRegistry",
            ".ficant.app.v1.GetAppRegistryRequest",
            ".ficant.app.v1.GetAppRegistryResponse",
        ),
        ExpectedMethod::new(
            "GetCurrentSession",
            ".ficant.app.v1.GetCurrentSessionRequest",
            ".ficant.app.v1.GetCurrentSessionResponse",
        ),
        ExpectedMethod::new(
            "RefreshSession",
            ".ficant.app.v1.RefreshSessionRequest",
            ".ficant.app.v1.RefreshSessionResponse",
        ),
        ExpectedMethod::new(
            "RevokeSession",
            ".ficant.app.v1.RevokeSessionRequest",
            ".ficant.app.v1.RevokeSessionResponse",
        ),
        ExpectedMethod::new(
            "AuthorizeAppLaunch",
            ".ficant.app.v1.AuthorizeAppLaunchRequest",
            ".ficant.app.v1.AppLaunchAuthorizationResponse",
        ),
        ExpectedMethod::new(
            "RefreshAppLaunch",
            ".ficant.app.v1.RefreshAppLaunchRequest",
            ".ficant.app.v1.AppLaunchAuthorizationResponse",
        ),
        ExpectedMethod::new(
            "RevokeAppLaunch",
            ".ficant.app.v1.RevokeAppLaunchRequest",
            ".ficant.app.v1.RevokeAppLaunchResponse",
        ),
    ]
}

fn rates_analytics_methods() -> Vec<ExpectedMethod> {
    vec![
        ExpectedMethod::new(
            "AnalyzeBond",
            ".ficant.rates.v1.AnalyzeBondRequest",
            ".ficant.rates.v1.AnalyzeBondResponse",
        ),
        ExpectedMethod::new(
            "InterpolateYieldCurve",
            ".ficant.rates.v1.InterpolateYieldCurveRequest",
            ".ficant.rates.v1.InterpolateYieldCurveResponse",
        ),
        ExpectedMethod::new(
            "AnalyzeCarryRoll",
            ".ficant.rates.v1.AnalyzeCarryRollRequest",
            ".ficant.rates.v1.AnalyzeCarryRollResponse",
        ),
        ExpectedMethod::new(
            "AnalyzeFuturesDelivery",
            ".ficant.rates.v1.AnalyzeFuturesDeliveryRequest",
            ".ficant.rates.v1.AnalyzeFuturesDeliveryResponse",
        ),
        ExpectedMethod::new(
            "AnalyzeFuturesHedge",
            ".ficant.rates.v1.AnalyzeFuturesHedgeRequest",
            ".ficant.rates.v1.AnalyzeFuturesHedgeResponse",
        ),
    ]
}

#[test]
fn service_inventory_rejects_an_unauthorized_ficant_service() {
    let mut actual = expected_service_fqns();
    actual.insert("ficant.app.v1.UnauthorizedService".to_owned());

    assert!(
        validate_service_inventory(&actual).is_err(),
        "an unauthorized additional ficant service must be rejected"
    );
}

fn assert_service_inventory(descriptor_set: &FileDescriptorSet) {
    let mut services = BTreeSet::new();
    for file in &descriptor_set.file {
        let package = file.package.as_deref().unwrap_or_default();
        for service in &file.service {
            let name = service
                .name
                .as_deref()
                .expect("every service must be named");
            if package.starts_with("ficant.") {
                services.insert(format!("{package}.{name}"));
            }
            assert!(
                !service.method.is_empty(),
                "service {package}.{name} must expose a real Phase 1 operation"
            );
        }
    }
    validate_service_inventory(&services).unwrap_or_else(|message| panic!("{message}"));
}

fn expected_service_fqns() -> BTreeSet<String> {
    BTreeSet::from([
        "ficant.core.v1.RegistryService".to_owned(),
        "ficant.market.v1.MarketDefinitionService".to_owned(),
        "ficant.market.v1.MarketFactService".to_owned(),
        "ficant.research.v1.SnapshotService".to_owned(),
        "ficant.research.v1.ExperimentService".to_owned(),
        "ficant.research.v1.ArtifactService".to_owned(),
        "ficant.app.v1.PlatformService".to_owned(),
        "ficant.rates.v1.RatesAnalyticsService".to_owned(),
    ])
}

fn validate_service_inventory(actual: &BTreeSet<String>) -> Result<(), String> {
    let expected = expected_service_fqns();
    if &expected == actual {
        Ok(())
    } else {
        Err(format!(
            "ficant service inventory drifted; missing={:?} unexpected={:?}",
            expected.difference(actual).collect::<Vec<_>>(),
            actual.difference(&expected).collect::<Vec<_>>()
        ))
    }
}

fn assert_fields(
    messages: &BTreeMap<String, &DescriptorProto>,
    message_name: &str,
    expected_fields: &[ExpectedField],
) {
    let message = messages
        .get(message_name)
        .unwrap_or_else(|| panic!("missing message {message_name}"));
    assert_eq!(
        message.field.len(),
        expected_fields.len(),
        "{message_name} must have the exact frozen field set; actual={:?} expected={:?}",
        message
            .field
            .iter()
            .map(|field| field.name())
            .collect::<Vec<_>>(),
        expected_fields
            .iter()
            .map(|field| field.name)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        message
            .field
            .iter()
            .map(|field| field.name())
            .collect::<BTreeSet<_>>(),
        expected_fields
            .iter()
            .map(|field| field.name)
            .collect::<BTreeSet<_>>(),
        "{message_name} must not add, remove, or rename fields"
    );
    let expected_oneofs = expected_fields
        .iter()
        .filter_map(|field| field.oneof)
        .collect::<BTreeSet<_>>();
    let actual_oneofs = message
        .oneof_decl
        .iter()
        .map(|oneof| oneof.name())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        actual_oneofs, expected_oneofs,
        "{message_name} must have the exact frozen oneof declarations"
    );

    for (index, expected) in expected_fields.iter().enumerate() {
        let field = message
            .field
            .iter()
            .find(|field| field.name() == expected.name)
            .unwrap_or_else(|| panic!("{message_name} missing field {}", expected.name));
        assert_eq!(
            field.number(),
            (index + 1) as i32,
            "{message_name}.{} has the wrong field number",
            expected.name
        );
        assert_eq!(
            field.r#type(),
            expected.field_type,
            "{message_name}.{} has the wrong wire type",
            expected.name
        );
        assert_eq!(
            field.type_name.as_deref(),
            expected.type_name,
            "{message_name}.{} has the wrong referenced type",
            expected.name
        );
        assert_eq!(
            field.label() == Label::Repeated,
            expected.repeated,
            "{message_name}.{} has the wrong cardinality",
            expected.name
        );
        let actual_oneof = field.oneof_index.map(|oneof_index| {
            message.oneof_decl[oneof_index as usize]
                .name
                .as_deref()
                .expect("oneof must be named")
        });
        assert_eq!(
            actual_oneof, expected.oneof,
            "{message_name}.{} has the wrong oneof branch",
            expected.name
        );
    }
}
