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
    ChangeJustification, DecimalValue, FoundationChangeRecord, PlatformRole, Subject,
    SubjectStateSnapshot, SubjectVersion,
};
use ficant_contracts::ficant::market::v1::{
    BondCouponTaxRule, BondCouponTaxTreatmentRule, BondTaxAttributes, CgbFuturesDeliveryRulePack,
    CgbFuturesProductRule, CompleteInstrumentDefinition, CouponTaxClaimScope,
    DataSourceAuthorization, FundingRulePack, FundingTierRate, GrossCouponTaxBasis, Instrument,
    InstrumentKind, InstrumentMapping, MarketDefinition, MarketFact, MarketRulePack,
    SubjectCouponTaxRate, SubjectCouponTaxTreatment, TaxRoundingMode, TaxRulePack, TaxRulePackV2,
};
use ficant_contracts::ficant::rates::v1::{
    AnalysisInputBinding, AnalysisInputRole, AnalyzeBondRequest, AnalyzeFuturesDeliveryRequest,
    AnalyzeFuturesDeliveryResult, ArtifactBinding, CurveNodeBinding,
    FuturesDeliveryCandidateResult, FuturesDeliveryMeasures, ParameterDigest, SnapshotBinding,
    TaxAdjustedBondAnalytics,
};
use ficant_contracts::ficant::research::v1::{
    DataSnapshot, ExecutionInstanceIdentity, ExperimentRun, ReproducibilityIdentity, ResearchGraph,
    RunState,
};

const DEFAULT_BUF: &str = "buf";
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
    let snapshot_binding = SnapshotBinding::default();
    let artifact_binding = ArtifactBinding::default();
    let curve_node_binding = CurveNodeBinding::default();
    let input_binding = AnalysisInputBinding::default();
    let parameter_digest = ParameterDigest::default();
    let graph = ResearchGraph::default();
    let reproducibility = ReproducibilityIdentity::default();
    let execution = ExecutionInstanceIdentity::default();
    let subject = Subject::default();
    let subject_version = SubjectVersion::default();
    let subject_state = SubjectStateSnapshot::default();
    let rule_pack = MarketRulePack::default();
    let cgb_futures_rule_pack = CgbFuturesDeliveryRulePack::default();
    let cgb_product_rule = CgbFuturesProductRule::default();
    let funding_rule_pack = FundingRulePack::default();
    let funding_tier_rate = FundingTierRate::default();
    let tax_rule_pack = TaxRulePack::default();
    let coupon_tax_rule = BondCouponTaxRule::default();
    let subject_coupon_tax_rate = SubjectCouponTaxRate::default();
    let bond_tax_attributes = BondTaxAttributes::default();
    let tax_rule_pack_v2 = TaxRulePackV2::default();
    let coupon_tax_treatment_rule = BondCouponTaxTreatmentRule::default();
    let subject_coupon_tax_treatment = SubjectCouponTaxTreatment::default();
    let delivery_request = AnalyzeFuturesDeliveryRequest::default();
    let delivery_measures = FuturesDeliveryMeasures::default();
    let delivery_candidate = FuturesDeliveryCandidateResult::default();
    let delivery_result = AnalyzeFuturesDeliveryResult::default();
    let after_tax = TaxAdjustedBondAnalytics::default();
    let change = ChangeJustification::default();
    let change_record = FoundationChangeRecord::default();
    let complete_instrument = CompleteInstrumentDefinition::default();
    let definition = MarketDefinition::default();
    let fact = MarketFact::default();
    let authorization = DataSourceAuthorization::default();
    let data_snapshot = DataSnapshot::default();
    let mapping = InstrumentMapping::default();

    assert!(instrument.instrument_id.is_none());
    assert_eq!(instrument.kind, InstrumentKind::Unspecified as i32);
    assert!(decimal.unit.is_none());
    assert_eq!(run.state, RunState::Unspecified as i32);
    assert!(registry.apps.is_empty());
    assert!(rates.context.is_none());
    assert!(snapshot_binding.snapshot_id.is_none());
    assert!(artifact_binding.artifact_id.is_none());
    assert!(curve_node_binding.curve_node_id.is_empty());
    assert_eq!(input_binding.role, AnalysisInputRole::Unspecified as i32);
    assert!(input_binding.binding.is_none());
    assert!(parameter_digest.canonical_parameters_sha256.is_none());
    assert!(graph.nodes.is_empty());
    assert!(reproducibility.node_implementations.is_empty());
    assert!(execution.reproducibility.is_none());
    assert!(subject.subject_id.is_none());
    assert!(subject_version.subject_ref.is_none());
    assert!(subject_state.snapshot_id.is_none());
    assert!(rule_pack.content.is_none());
    assert!(cgb_futures_rule_pack.products.is_empty());
    assert!(cgb_product_rule.product_code.is_none());
    assert!(funding_rule_pack.rates.is_empty());
    assert!(funding_tier_rate.annual_financing_rate.is_none());
    assert!(tax_rule_pack.coupon_rules.is_empty());
    assert!(coupon_tax_rule.tax_attributes.is_none());
    assert!(subject_coupon_tax_rate.coupon_tax_rate.is_none());
    assert_eq!(bond_tax_attributes.value_added_tax_status, 0);
    assert!(tax_rule_pack_v2.coupon_rules.is_empty());
    assert!(coupon_tax_treatment_rule.treatments.is_empty());
    assert_eq!(
        subject_coupon_tax_treatment.gross_coupon_basis,
        GrossCouponTaxBasis::Unspecified as i32
    );
    assert_eq!(
        subject_coupon_tax_treatment.rounding,
        TaxRoundingMode::Unspecified as i32
    );
    assert_eq!(
        subject_coupon_tax_treatment.claim_scope,
        CouponTaxClaimScope::Unspecified as i32
    );
    assert!(delivery_request.tax_rule_pack.is_none());
    assert!(delivery_measures.tax_adjusted_interim_coupons.is_none());
    assert!(delivery_candidate.measures.is_none());
    assert_eq!(delivery_result.subject_ctd_index, 0);
    assert_eq!(
        after_tax.claim_scope,
        CouponTaxClaimScope::Unspecified as i32
    );
    assert!(change.sources.is_empty());
    assert_eq!(change_record.active_role, PlatformRole::Unspecified as i32);
    assert!(complete_instrument.instrument.is_none());
    assert!(definition.definition.is_none());
    assert!(fact.fact.is_none());
    assert!(authorization.r#ref.is_none());
    assert!(data_snapshot.authorization_ref.is_none());
    assert!(mapping.mapping_id.is_none());
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
    assert_r5d_rates_contracts(&messages, &top_level_enums(descriptor_set));
    assert_phase1_objects(&messages);
    assert_cgb_futures_rule_pack_contract(&messages);
    assert_funding_rule_pack_contract(&messages);
    assert_tax_rule_pack_contract(&messages, &top_level_enums(descriptor_set));
    assert_position_snapshot_contract(&messages);
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
fn position_snapshot_service_has_exact_unary_rpcs() {
    assert_exact_service(
        descriptor_set(),
        "ficant.research.v1.PositionSnapshotService",
        &position_snapshot_methods(),
    );
}

#[test]
fn factor_registry_service_has_exact_unary_rpcs() {
    assert_exact_service(
        descriptor_set(),
        "ficant.research.v1.FactorRegistryService",
        &factor_registry_methods(),
    );
}

#[test]
fn r5a_price_source_contracts_are_exact() {
    let descriptor_set = descriptor_set();
    let messages = top_level_messages(descriptor_set);
    let enums = top_level_enums(descriptor_set);

    assert_enum(
        &enums,
        "ficant.market.v1.PriceSourceType",
        &[
            ("PRICE_SOURCE_TYPE_UNSPECIFIED", 0),
            ("PRICE_SOURCE_TYPE_REAL_TRADE", 1),
            ("PRICE_SOURCE_TYPE_ACTIVE_QUOTE", 2),
            ("PRICE_SOURCE_TYPE_MODEL_VALUATION", 3),
            ("PRICE_SOURCE_TYPE_CURVE_INTERPOLATION", 4),
        ],
    );
    assert_enum(
        &enums,
        "ficant.market.v1.DataSourceKind",
        &[
            ("DATA_SOURCE_KIND_UNSPECIFIED", 0),
            ("DATA_SOURCE_KIND_FILE_NDJSON", 1),
            ("DATA_SOURCE_KIND_POSTGRES", 2),
        ],
    );
    assert_fields(
        &messages,
        "ficant.market.v1.DataSourceDefinition",
        &[
            ExpectedField::message("data_source", ".ficant.core.v1.VersionRef"),
            ExpectedField::message("owner", ".ficant.core.v1.OwnerRef"),
            ExpectedField::enumeration("kind", ".ficant.market.v1.DataSourceKind"),
            ExpectedField::scalar("name", Type::String),
            ExpectedField::scalar("connection_binding", Type::String),
            ExpectedField::scalar("dataset", Type::String),
            ExpectedField::scalar("canonical_schema_id", Type::String),
            ExpectedField::message("canonical_schema_hash", ".ficant.core.v1.Sha256"),
            ExpectedField::enumeration("price_source_type", ".ficant.market.v1.PriceSourceType"),
        ],
    );
    assert_fields(
        &messages,
        "ficant.market.v1.FactSource",
        &[
            ExpectedField::scalar("source_id", Type::String),
            ExpectedField::scalar("external_id", Type::String),
            ExpectedField::scalar("source_revision", Type::Uint64),
            ExpectedField::message("data_source", ".ficant.core.v1.VersionRef"),
        ],
    );
    assert_fields(
        &messages,
        "ficant.market.v1.RegisterDataSourceRequest",
        &[
            ExpectedField::scalar("idempotency_key", Type::String),
            ExpectedField::scalar("expected_latest_version", Type::Uint64),
            ExpectedField::message("definition", ".ficant.market.v1.DataSourceDefinition"),
            ExpectedField::message("change", ".ficant.core.v1.ChangeJustification"),
        ],
    );
    assert_fields(
        &messages,
        "ficant.market.v1.RegisterDataSourceResponse",
        &[
            ExpectedField::oneof_message(
                "definition",
                ".ficant.market.v1.DataSourceDefinition",
                "result",
            ),
            ExpectedField::oneof_message("error", ".ficant.core.v1.ErrorDetail", "result"),
        ],
    );
    assert_fields(
        &messages,
        "ficant.market.v1.GetDataSourceRequest",
        &[ExpectedField::message(
            "data_source",
            ".ficant.core.v1.VersionRef",
        )],
    );
    assert_fields(
        &messages,
        "ficant.market.v1.GetDataSourceResponse",
        &[
            ExpectedField::oneof_message(
                "definition",
                ".ficant.market.v1.DataSourceDefinition",
                "result",
            ),
            ExpectedField::oneof_message("error", ".ficant.core.v1.ErrorDetail", "result"),
        ],
    );
    assert_enum(
        &enums,
        "ficant.market.v1.ImportInterface",
        &[
            ("IMPORT_INTERFACE_UNSPECIFIED", 0),
            ("IMPORT_INTERFACE_CANONICAL_QUOTE_SNAPSHOT", 1),
        ],
    );
    assert_enum(
        &enums,
        "ficant.market.v1.DataSourceAuthorizationState",
        &[
            ("DATA_SOURCE_AUTHORIZATION_STATE_UNSPECIFIED", 0),
            ("DATA_SOURCE_AUTHORIZATION_STATE_ACTIVE", 1),
            ("DATA_SOURCE_AUTHORIZATION_STATE_REVOKED", 2),
        ],
    );
    assert_fields(
        &messages,
        "ficant.market.v1.DataSourceAuthorization",
        &[
            ExpectedField::message("ref", ".ficant.core.v1.VersionRef"),
            ExpectedField::message("owner", ".ficant.core.v1.OwnerRef"),
            ExpectedField::message("source", ".ficant.core.v1.VersionRef"),
            ExpectedField::message("source_hash", ".ficant.core.v1.Sha256"),
            ExpectedField::enumeration("interface", ".ficant.market.v1.ImportInterface"),
            ExpectedField::scalar("schema_id", Type::String),
            ExpectedField::message("schema_hash", ".ficant.core.v1.Sha256"),
            ExpectedField::message("effective_from", ".ficant.core.v1.MarketTime"),
            ExpectedField::message("effective_to", ".ficant.core.v1.MarketTime"),
            ExpectedField::enumeration("state", ".ficant.market.v1.DataSourceAuthorizationState"),
            ExpectedField::message("supersedes", ".ficant.core.v1.VersionRef"),
            ExpectedField::message("content_hash", ".ficant.core.v1.Sha256"),
            ExpectedField::message("mapping_id", ".ficant.core.v1.Ulid"),
            ExpectedField::message("mapping_hash", ".ficant.core.v1.Sha256"),
        ],
    );
    assert_fields(
        &messages,
        "ficant.market.v1.PublishDataSourceAuthorizationRequest",
        &[
            ExpectedField::scalar("idempotency_key", Type::String),
            ExpectedField::scalar("expected_latest_version", Type::Uint64),
            ExpectedField::message("authorization", ".ficant.market.v1.DataSourceAuthorization"),
            ExpectedField::message("change", ".ficant.core.v1.ChangeJustification"),
            ExpectedField::message("mapping", ".ficant.market.v1.InstrumentMapping"),
        ],
    );
    for response in [
        "ficant.market.v1.PublishDataSourceAuthorizationResponse",
        "ficant.market.v1.GetDataSourceAuthorizationResponse",
    ] {
        assert_fields(
            &messages,
            response,
            &[
                ExpectedField::oneof_message(
                    "authorization",
                    ".ficant.market.v1.DataSourceAuthorization",
                    "result",
                ),
                ExpectedField::oneof_message("error", ".ficant.core.v1.ErrorDetail", "result"),
            ],
        );
    }
    assert_fields(
        &messages,
        "ficant.market.v1.GetDataSourceAuthorizationRequest",
        &[ExpectedField::message(
            "authorization_ref",
            ".ficant.core.v1.VersionRef",
        )],
    );
    assert_fields(
        &messages,
        "ficant.market.v1.ListDataSourceAuthorizationsRequest",
        &[
            ExpectedField::message("owner", ".ficant.core.v1.OwnerRef"),
            ExpectedField::message("data_source", ".ficant.core.v1.VersionRef"),
            ExpectedField::enumeration("import_interface", ".ficant.market.v1.ImportInterface"),
            ExpectedField::message("page", ".ficant.core.v1.PageRequest"),
        ],
    );
    assert_fields(
        &messages,
        "ficant.market.v1.DataSourceAuthorizations",
        &[
            ExpectedField::repeated_message(
                "authorizations",
                ".ficant.market.v1.DataSourceAuthorization",
            ),
            ExpectedField::message("page", ".ficant.core.v1.PageResponse"),
        ],
    );
    assert_fields(
        &messages,
        "ficant.market.v1.ListDataSourceAuthorizationsResponse",
        &[
            ExpectedField::oneof_message(
                "authorizations",
                ".ficant.market.v1.DataSourceAuthorizations",
                "result",
            ),
            ExpectedField::oneof_message("error", ".ficant.core.v1.ErrorDetail", "result"),
        ],
    );
    assert_fields(
        &messages,
        "ficant.research.v1.PriceSourceCount",
        &[
            ExpectedField::enumeration("source_type", ".ficant.market.v1.PriceSourceType"),
            ExpectedField::scalar("record_count", Type::Uint64),
        ],
    );
    assert_fields(
        &messages,
        "ficant.research.v1.PriceSourceSummary",
        &[
            ExpectedField::repeated_message("counts", ".ficant.research.v1.PriceSourceCount"),
            ExpectedField::scalar("mixed", Type::Bool),
        ],
    );
    assert_fields(
        &messages,
        "ficant.research.v1.CoverageDeclaration",
        &[
            ExpectedField::scalar("imported_position_count", Type::Uint64),
            ExpectedField::scalar("participating_position_count", Type::Uint64),
            ExpectedField::repeated_message(
                "imported_gross_economic_value_by_unit",
                ".ficant.core.v1.DecimalValue",
            ),
            ExpectedField::repeated_message(
                "participating_gross_economic_value_by_unit",
                ".ficant.core.v1.DecimalValue",
            ),
            ExpectedField::scalar("missing_critical_field_record_count", Type::Uint64),
            ExpectedField::message(
                "source_confidence",
                ".ficant.research.v1.PriceSourceSummary",
            ),
            ExpectedField::scalar("distinct_external_data_source_version_count", Type::Uint64),
        ],
    );
    assert_exact_service(
        descriptor_set,
        "ficant.market.v1.DataSourceRegistryService",
        &[
            ExpectedMethod::new(
                "RegisterDataSource",
                ".ficant.market.v1.RegisterDataSourceRequest",
                ".ficant.market.v1.RegisterDataSourceResponse",
            ),
            ExpectedMethod::new(
                "GetDataSource",
                ".ficant.market.v1.GetDataSourceRequest",
                ".ficant.market.v1.GetDataSourceResponse",
            ),
            ExpectedMethod::new(
                "PublishDataSourceAuthorization",
                ".ficant.market.v1.PublishDataSourceAuthorizationRequest",
                ".ficant.market.v1.PublishDataSourceAuthorizationResponse",
            ),
            ExpectedMethod::new(
                "GetDataSourceAuthorization",
                ".ficant.market.v1.GetDataSourceAuthorizationRequest",
                ".ficant.market.v1.GetDataSourceAuthorizationResponse",
            ),
            ExpectedMethod::new(
                "ListDataSourceAuthorizations",
                ".ficant.market.v1.ListDataSourceAuthorizationsRequest",
                ".ficant.market.v1.ListDataSourceAuthorizationsResponse",
            ),
        ],
    );
}

#[test]
fn r5c_data_health_contracts_are_exact() {
    let descriptor_set = descriptor_set();
    let messages = top_level_messages(descriptor_set);
    let enums = top_level_enums(descriptor_set);

    assert_enum(
        &enums,
        "ficant.research.v1.DataHealthState",
        &[
            ("DATA_HEALTH_STATE_UNSPECIFIED", 0),
            ("DATA_HEALTH_STATE_HEALTHY", 1),
            ("DATA_HEALTH_STATE_WARNING", 2),
        ],
    );
    assert_enum(
        &enums,
        "ficant.research.v1.PositionSetState",
        &[
            ("POSITION_SET_STATE_UNSPECIFIED", 0),
            ("POSITION_SET_STATE_NON_EMPTY", 1),
            ("POSITION_SET_STATE_VERIFIED_EMPTY", 2),
        ],
    );
    assert_enum(
        &enums,
        "ficant.research.v1.DataHealthIssueCode",
        &[
            ("DATA_HEALTH_ISSUE_CODE_UNSPECIFIED", 0),
            ("DATA_HEALTH_ISSUE_CODE_EMPTY_POSITIONS", 1),
            (
                "DATA_HEALTH_ISSUE_CODE_UNKNOWN_ACCOUNTING_CLASSIFICATION",
                2,
            ),
            ("DATA_HEALTH_ISSUE_CODE_STALE_POSITION_SNAPSHOT", 3),
            ("DATA_HEALTH_ISSUE_CODE_UNTYPED_PRICE_SOURCE", 4),
            ("DATA_HEALTH_ISSUE_CODE_MODEL_VALUATION_SHARE", 5),
            ("DATA_HEALTH_ISSUE_CODE_STALE_DATA_SNAPSHOT", 6),
        ],
    );
    assert_fields(
        &messages,
        "ficant.research.v1.DataHealthThresholdProfile",
        &[
            ExpectedField::message("profile_ref", ".ficant.core.v1.VersionRef"),
            ExpectedField::scalar("max_position_snapshot_age_seconds", Type::Uint64),
            ExpectedField::scalar("unknown_accounting_warning_basis_points", Type::Uint32),
            ExpectedField::scalar("max_data_snapshot_age_seconds", Type::Uint64),
            ExpectedField::scalar("model_valuation_warning_basis_points", Type::Uint32),
            ExpectedField::message("content_hash", ".ficant.core.v1.Sha256"),
            ExpectedField::message("profile_snapshot_id", ".ficant.core.v1.Ulid"),
            ExpectedField::message("owner", ".ficant.core.v1.OwnerRef"),
            ExpectedField::message("visible_at", ".ficant.core.v1.MarketTime"),
            ExpectedField::message("effective_from", ".ficant.core.v1.MarketTime"),
            ExpectedField::message("effective_to", ".ficant.core.v1.MarketTime"),
            ExpectedField::repeated_message("lineage", ".ficant.core.v1.LineageRef"),
        ],
    );
    assert_fields(
        &messages,
        "ficant.research.v1.DataHealthIssue",
        &[
            ExpectedField::enumeration("code", ".ficant.research.v1.DataHealthIssueCode"),
            ExpectedField::repeated_message("affected_position_ids", ".ficant.core.v1.Ulid"),
            ExpectedField::message("data_source_ref", ".ficant.core.v1.VersionRef"),
            ExpectedField::scalar("record_count", Type::Uint64),
            ExpectedField::scalar("ratio_basis_points", Type::Uint32),
            ExpectedField::scalar("observed_age_seconds", Type::Uint64),
        ],
    );
    assert_fields(
        &messages,
        "ficant.research.v1.GetDataHealthReportRequest",
        &[
            ExpectedField::message("subject_ref", ".ficant.core.v1.VersionRef"),
            ExpectedField::message("position_snapshot_id", ".ficant.core.v1.Ulid"),
            ExpectedField::message("data_snapshot_id", ".ficant.core.v1.Ulid"),
            ExpectedField::message("evaluated_at", ".ficant.core.v1.MarketTime"),
        ],
    );
    assert_reserved_tag(
        &messages,
        "ficant.research.v1.GetDataHealthReportRequest",
        5,
    );
    assert_fields(
        &messages,
        "ficant.research.v1.PublishDataHealthThresholdProfileRequest",
        &[
            ExpectedField::scalar("idempotency_key", Type::String),
            ExpectedField::message(
                "threshold_profile",
                ".ficant.research.v1.DataHealthThresholdProfile",
            ),
            ExpectedField::message("change", ".ficant.core.v1.ChangeJustification"),
        ],
    );
    assert_fields(
        &messages,
        "ficant.research.v1.PublishDataHealthThresholdProfileResponse",
        &[
            ExpectedField::oneof_message(
                "threshold_profile",
                ".ficant.research.v1.DataHealthThresholdProfile",
                "result",
            ),
            ExpectedField::oneof_message("error", ".ficant.core.v1.ErrorDetail", "result"),
        ],
    );
    assert_fields(
        &messages,
        "ficant.research.v1.DataHealthReport",
        &[
            ExpectedField::message("owner", ".ficant.core.v1.OwnerRef"),
            ExpectedField::message("subject_ref", ".ficant.core.v1.VersionRef"),
            ExpectedField::message("evaluated_at", ".ficant.core.v1.MarketTime"),
            ExpectedField::message("position_snapshot_id", ".ficant.core.v1.Ulid"),
            ExpectedField::message("position_snapshot_hash", ".ficant.core.v1.Sha256"),
            ExpectedField::message("data_snapshot_id", ".ficant.core.v1.Ulid"),
            ExpectedField::message("data_snapshot_manifest_hash", ".ficant.core.v1.Sha256"),
            ExpectedField::message("data_source_ref", ".ficant.core.v1.VersionRef"),
            ExpectedField::message(
                "threshold_profile",
                ".ficant.research.v1.DataHealthThresholdProfile",
            ),
            ExpectedField::enumeration("state", ".ficant.research.v1.DataHealthState"),
            ExpectedField::repeated_message("issues", ".ficant.research.v1.DataHealthIssue"),
            ExpectedField::scalar("price_evidence_evaluated", Type::Bool),
            ExpectedField::enumeration(
                "position_set_state",
                ".ficant.research.v1.PositionSetState",
            ),
            ExpectedField::message("coverage", ".ficant.research.v1.CoverageDeclaration"),
            ExpectedField::message("request_fingerprint", ".ficant.core.v1.Sha256"),
            ExpectedField::message("content_hash", ".ficant.core.v1.Sha256"),
            ExpectedField::repeated_message("lineage", ".ficant.core.v1.LineageRef"),
        ],
    );
    assert_fields(
        &messages,
        "ficant.research.v1.GetDataHealthReportResponse",
        &[
            ExpectedField::oneof_message(
                "report",
                ".ficant.research.v1.DataHealthReport",
                "result",
            ),
            ExpectedField::oneof_message("error", ".ficant.core.v1.ErrorDetail", "result"),
        ],
    );
    assert_exact_service(
        descriptor_set,
        "ficant.research.v1.DataHealthService",
        &[
            ExpectedMethod::new(
                "PublishDataHealthThresholdProfile",
                ".ficant.research.v1.PublishDataHealthThresholdProfileRequest",
                ".ficant.research.v1.PublishDataHealthThresholdProfileResponse",
            ),
            ExpectedMethod::new(
                "GetDataHealthReport",
                ".ficant.research.v1.GetDataHealthReportRequest",
                ".ficant.research.v1.GetDataHealthReportResponse",
            ),
        ],
    );
}

#[test]
fn r4d_a_bond_curve_and_portfolio_risk_contracts_are_exact() {
    let descriptor_set = descriptor_set();
    let messages = top_level_messages(descriptor_set);
    let enums = top_level_enums(descriptor_set);
    assert_fields(
        &messages,
        "ficant.market.v1.CurvePoint",
        &[
            ExpectedField::scalar("curve_node_id", Type::String),
            ExpectedField::message("curve_node_content_hash", ".ficant.core.v1.Sha256"),
            ExpectedField::message("yield_to_maturity", ".ficant.core.v1.DecimalValue"),
        ],
    );
    assert_fields(
        &messages,
        "ficant.market.v1.CurvePointSet",
        &[
            ExpectedField::scalar("curve_family_id", Type::String),
            ExpectedField::repeated_message("points", ".ficant.market.v1.CurvePoint"),
        ],
    );
    assert_fields(
        &messages,
        "ficant.research.v1.RiskAlgorithmBinding",
        &[
            ExpectedField::scalar("algorithm_id", Type::String),
            ExpectedField::scalar("algorithm_version", Type::Uint32),
            ExpectedField::scalar("convention_profile", Type::String),
        ],
    );
    assert_fields(
        &messages,
        "ficant.research.v1.FactorDv01",
        &[
            ExpectedField::scalar("factor_id", Type::String),
            ExpectedField::message("factor_definition_hash", ".ficant.core.v1.Sha256"),
            ExpectedField::message("dv01", ".ficant.core.v1.DecimalValue"),
        ],
    );
    assert_fields(
        &messages,
        "ficant.research.v1.PositionKeyRateExposure",
        &[
            ExpectedField::message("position_id", ".ficant.core.v1.Ulid"),
            ExpectedField::message("instrument", ".ficant.core.v1.VersionRef"),
            ExpectedField::repeated_message("exposures", ".ficant.research.v1.FactorDv01"),
            ExpectedField::message("content_hash", ".ficant.core.v1.Sha256"),
            ExpectedField::repeated_message("lineage", ".ficant.core.v1.LineageRef"),
        ],
    );
    assert_fields(
        &messages,
        "ficant.research.v1.PortfolioKeyRateExposure",
        &[
            ExpectedField::message("position_snapshot_id", ".ficant.core.v1.Ulid"),
            ExpectedField::message("curve_snapshot_id", ".ficant.core.v1.Ulid"),
            ExpectedField::repeated_message(
                "positions",
                ".ficant.research.v1.PositionKeyRateExposure",
            ),
            ExpectedField::repeated_message("totals", ".ficant.research.v1.FactorDv01"),
            ExpectedField::message("algorithm", ".ficant.research.v1.RiskAlgorithmBinding"),
            ExpectedField::message("content_hash", ".ficant.core.v1.Sha256"),
            ExpectedField::repeated_message("lineage", ".ficant.core.v1.LineageRef"),
            ExpectedField::message("futures_data_snapshot_id", ".ficant.core.v1.Ulid"),
            ExpectedField::message(
                "source_confidence",
                ".ficant.research.v1.PriceSourceSummary",
            ),
            ExpectedField::message("coverage", ".ficant.research.v1.CoverageDeclaration"),
        ],
    );
    assert_fields(
        &messages,
        "ficant.research.v1.CalculateKeyRateDv01Request",
        &[
            ExpectedField::message("position_snapshot_id", ".ficant.core.v1.Ulid"),
            ExpectedField::message("knowledge_at", ".ficant.core.v1.MarketTime"),
            ExpectedField::message("valuation_at", ".ficant.core.v1.MarketTime"),
            ExpectedField::message("curve_snapshot_id", ".ficant.core.v1.Ulid"),
            ExpectedField::message("dv01_unit", ".ficant.core.v1.UnitRef"),
            ExpectedField::message("futures_data_snapshot_id", ".ficant.core.v1.Ulid"),
        ],
    );
    assert_fields(
        &messages,
        "ficant.research.v1.CalculateKeyRateDv01Response",
        &[
            ExpectedField::oneof_message(
                "exposure",
                ".ficant.research.v1.PortfolioKeyRateExposure",
                "result",
            ),
            ExpectedField::oneof_message("error", ".ficant.core.v1.ErrorDetail", "result"),
        ],
    );
    assert_enum(
        &enums,
        "ficant.market.v1.BondCouponFrequency",
        &[
            ("BOND_COUPON_FREQUENCY_UNSPECIFIED", 0),
            ("BOND_COUPON_FREQUENCY_ANNUAL", 1),
            ("BOND_COUPON_FREQUENCY_SEMIANNUAL", 2),
        ],
    );
    assert_enum(
        &enums,
        "ficant.market.v1.BondDayCountConvention",
        &[
            ("BOND_DAY_COUNT_CONVENTION_UNSPECIFIED", 0),
            ("BOND_DAY_COUNT_CONVENTION_ACT_ACT_BOND_ISMA", 1),
        ],
    );
    assert_enum(
        &enums,
        "ficant.market.v1.BondBusinessDayConvention",
        &[
            ("BOND_BUSINESS_DAY_CONVENTION_UNSPECIFIED", 0),
            ("BOND_BUSINESS_DAY_CONVENTION_FOLLOWING", 1),
        ],
    );
    assert_exact_service(
        descriptor_set,
        "ficant.research.v1.PortfolioRiskService",
        &[ExpectedMethod::new(
            "CalculateKeyRateDv01",
            ".ficant.research.v1.CalculateKeyRateDv01Request",
            ".ficant.research.v1.CalculateKeyRateDv01Response",
        )],
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
    let messages = top_level_messages(descriptor_set);
    let enums = top_level_enums(descriptor_set);
    assert_r6b_artifact_contracts(&messages, &enums);
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
fn rates_analytics_service_has_exact_r5d_signatures() {
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
    let enums = top_level_enums(descriptor_set);

    assert_platform_contracts(&messages);
    assert_governance_contracts(descriptor_set, &messages, &enums);
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
    assert_position_snapshot_enums(&enums);
    assert_exact_service(
        descriptor_set,
        "ficant.research.v1.SnapshotService",
        &snapshot_methods(),
    );
    assert_r6a_snapshot_contracts(&top_level_messages(descriptor_set));
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
    let descriptor_input = std::env::var_os("FICANT_DESCRIPTOR_INPUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("interface"));
    let output = Command::new(&buf)
        .arg("build")
        .arg(&descriptor_input)
        .args(["--as-file-descriptor-set", "-o"])
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

#[test]
fn composition_level_outputs_have_coverage() {
    let descriptor_set = descriptor_set();
    let messages = top_level_messages(descriptor_set);
    let services = top_level_services(descriptor_set);
    let actual = reachable_success_arms(&messages, &services);
    let expected = expected_success_arms();
    let classified_non_composition_count = NonCompositionReason::ALL
        .iter()
        .map(|reason| {
            expected
                .values()
                .filter(|class| **class == SuccessArmClass::NonComposition(*reason))
                .count()
        })
        .sum::<usize>();
    assert_eq!(
        classified_non_composition_count, 60,
        "every non-composition success arm must select one of the three closed reasons"
    );
    let expected_keys = expected.keys().cloned().collect::<BTreeSet<_>>();
    assert_eq!(
        actual, expected_keys,
        "every reachable RPC success arm must have an explicit coverage classification"
    );

    let composition_carriers = expected
        .iter()
        .filter_map(|(arm, class)| {
            (*class == SuccessArmClass::Composition).then(|| {
                arm.rsplit_once("->")
                    .expect("success-arm inventory entries contain a payload separator")
                    .1
                    .to_owned()
            })
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        composition_carriers,
        BTreeSet::from([
            "ficant.research.v1.CapitalUse".to_owned(),
            "ficant.research.v1.DataHealthReport".to_owned(),
            "ficant.research.v1.PortfolioKeyRateExposure".to_owned(),
            "ficant.research.v1.PositionViews".to_owned(),
        ]),
        "the explicitly classified composition carrier set must remain exact"
    );

    for (carrier, tag) in [
        ("ficant.research.v1.PortfolioKeyRateExposure", 10),
        ("ficant.research.v1.PositionViews", 5),
        ("ficant.research.v1.CapitalUse", 5),
        ("ficant.research.v1.DataHealthReport", 14),
    ] {
        let message = messages
            .get(carrier)
            .unwrap_or_else(|| panic!("missing composition carrier {carrier}"));
        let coverage = message
            .field
            .iter()
            .find(|field| field.name() == "coverage")
            .unwrap_or_else(|| panic!("{carrier} must carry CoverageDeclaration"));
        assert_eq!(coverage.number(), tag, "{carrier}.coverage tag changed");
        assert_eq!(coverage.r#type(), Type::Message);
        assert_eq!(
            coverage.type_name(),
            ".ficant.research.v1.CoverageDeclaration",
            "{carrier}.coverage must use the shared declaration"
        );
        assert_ne!(coverage.label(), Label::Repeated);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SuccessArmClass {
    Composition,
    NonComposition(NonCompositionReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NonCompositionReason {
    NoNumericAggregate,
    RegistryMetadata,
    AckOrEcho,
}

impl NonCompositionReason {
    const ALL: [Self; 3] = [
        Self::NoNumericAggregate,
        Self::RegistryMetadata,
        Self::AckOrEcho,
    ];
}

fn reachable_success_arms(
    messages: &BTreeMap<String, &DescriptorProto>,
    services: &BTreeMap<String, &ServiceDescriptorProto>,
) -> BTreeSet<String> {
    let mut arms = BTreeSet::new();
    for (service_name, service) in services {
        for method in &service.method {
            let method_name = method.name();
            let response_name = method.output_type().trim_start_matches('.');
            let response = messages
                .get(response_name)
                .unwrap_or_else(|| panic!("missing RPC response message {response_name}"));
            let oneof_fields = response
                .field
                .iter()
                .filter(|field| field.oneof_index.is_some())
                .collect::<Vec<_>>();
            if oneof_fields.is_empty() {
                let arm = format!("{service_name}/{method_name}:response->{response_name}");
                assert!(arms.insert(arm.clone()), "duplicate RPC success arm {arm}");
            } else {
                let success_fields = oneof_fields
                    .into_iter()
                    .filter(|field| {
                        !matches!(
                            field.type_name(),
                            ".ficant.core.v1.ErrorDetail" | ".ficant.app.v1.SafeError"
                        )
                    })
                    .collect::<Vec<_>>();
                assert!(
                    !success_fields.is_empty(),
                    "RPC {service_name}/{method_name} must expose at least one success arm"
                );
                for field in success_fields {
                    assert_eq!(
                        field.r#type(),
                        Type::Message,
                        "RPC success arm {service_name}/{method_name}:{} must be a message",
                        field.name()
                    );
                    let arm = format!(
                        "{service_name}/{method_name}:{}->{}",
                        field.name(),
                        field.type_name().trim_start_matches('.')
                    );
                    assert!(arms.insert(arm.clone()), "duplicate RPC success arm {arm}");
                }
            }
        }
    }
    arms
}

fn expected_success_arms() -> BTreeMap<String, SuccessArmClass> {
    use NonCompositionReason::{AckOrEcho, NoNumericAggregate, RegistryMetadata};
    use SuccessArmClass::NonComposition;

    [
        ("ficant.app.v1.PlatformService/AuthorizeAppLaunch:grant->ficant.app.v1.AppLaunchGrant", NonComposition(AckOrEcho)),
        ("ficant.app.v1.PlatformService/GetAppRegistry:registry->ficant.app.v1.AppRegistry", NonComposition(RegistryMetadata)),
        ("ficant.app.v1.PlatformService/GetCurrentSession:session->ficant.app.v1.Session", NonComposition(RegistryMetadata)),
        ("ficant.app.v1.PlatformService/RefreshAppLaunch:grant->ficant.app.v1.AppLaunchGrant", NonComposition(AckOrEcho)),
        ("ficant.app.v1.PlatformService/RefreshSession:session->ficant.app.v1.Session", NonComposition(AckOrEcho)),
        ("ficant.app.v1.PlatformService/RevokeAppLaunch:revocation->ficant.app.v1.AppLaunchRevocation", NonComposition(AckOrEcho)),
        ("ficant.app.v1.PlatformService/RevokeSession:revocation->ficant.app.v1.SessionRevocation", NonComposition(AckOrEcho)),
        ("ficant.core.v1.RegistryService/GetSubject:subject->ficant.core.v1.SubjectRecord", NonComposition(RegistryMetadata)),
        ("ficant.core.v1.RegistryService/GetSubjectState:snapshot->ficant.core.v1.SubjectStateSnapshot", NonComposition(RegistryMetadata)),
        ("ficant.core.v1.RegistryService/RegisterSubject:subject->ficant.core.v1.SubjectRecord", NonComposition(AckOrEcho)),
        ("ficant.core.v1.RegistryService/RegisterSubjectState:snapshot->ficant.core.v1.SubjectStateSnapshot", NonComposition(AckOrEcho)),
        ("ficant.core.v1.FoundationChangeService/GetFoundationChange:change->ficant.core.v1.FoundationChangeRecord", NonComposition(RegistryMetadata)),
        ("ficant.core.v1.FoundationChangeService/ListFoundationChanges:changes->ficant.core.v1.FoundationChangeRecords", NonComposition(RegistryMetadata)),
        ("ficant.market.v1.DataSourceRegistryService/GetDataSource:definition->ficant.market.v1.DataSourceDefinition", NonComposition(RegistryMetadata)),
        ("ficant.market.v1.DataSourceRegistryService/GetDataSourceAuthorization:authorization->ficant.market.v1.DataSourceAuthorization", NonComposition(RegistryMetadata)),
        ("ficant.market.v1.DataSourceRegistryService/ListDataSourceAuthorizations:authorizations->ficant.market.v1.DataSourceAuthorizations", NonComposition(RegistryMetadata)),
        ("ficant.market.v1.DataSourceRegistryService/PublishDataSourceAuthorization:authorization->ficant.market.v1.DataSourceAuthorization", NonComposition(AckOrEcho)),
        ("ficant.market.v1.DataSourceRegistryService/RegisterDataSource:definition->ficant.market.v1.DataSourceDefinition", NonComposition(AckOrEcho)),
        ("ficant.market.v1.MarketDefinitionService/AppendDefinition:definition->ficant.market.v1.MarketDefinition", NonComposition(AckOrEcho)),
        ("ficant.market.v1.MarketDefinitionService/GetDefinitionVersion:definition->ficant.market.v1.MarketDefinition", NonComposition(RegistryMetadata)),
        ("ficant.market.v1.MarketDefinitionService/ListDefinitionVersions:versions->ficant.market.v1.DefinitionVersions", NonComposition(RegistryMetadata)),
        ("ficant.market.v1.MarketDefinitionService/ResolveDefinitionAsOf:definition->ficant.market.v1.MarketDefinition", NonComposition(RegistryMetadata)),
        ("ficant.market.v1.MarketFactService/AppendMarketFact:fact->ficant.market.v1.MarketFact", NonComposition(AckOrEcho)),
        ("ficant.market.v1.MarketFactService/CorrectMarketFact:fact->ficant.market.v1.MarketFact", NonComposition(AckOrEcho)),
        ("ficant.market.v1.MarketFactService/GetCurveSnapshot:curve->ficant.market.v1.CurveSnapshotPayload", NonComposition(NoNumericAggregate)),
        ("ficant.market.v1.MarketFactService/PublishCurveSnapshot:curve_snapshot->ficant.market.v1.CurveSnapshot", NonComposition(AckOrEcho)),
        ("ficant.market.v1.MarketFactService/QueryInstrumentFacts:instrument_facts->ficant.market.v1.InstrumentFacts", NonComposition(NoNumericAggregate)),
        ("ficant.rates.v1.RatesAnalyticsService/AnalyzeBond:analysis->ficant.rates.v1.AnalyzeBondResult", NonComposition(NoNumericAggregate)),
        ("ficant.rates.v1.RatesAnalyticsService/AnalyzeCarryRoll:analysis->ficant.rates.v1.AnalyzeCarryRollResult", NonComposition(NoNumericAggregate)),
        ("ficant.rates.v1.RatesAnalyticsService/AnalyzeFuturesDelivery:analysis->ficant.rates.v1.AnalyzeFuturesDeliveryResult", NonComposition(NoNumericAggregate)),
        ("ficant.rates.v1.RatesAnalyticsService/AnalyzeFuturesHedge:analysis->ficant.rates.v1.AnalyzeFuturesHedgeResult", NonComposition(NoNumericAggregate)),
        ("ficant.rates.v1.RatesAnalyticsService/InterpolateYieldCurve:point->ficant.rates.v1.InterpolateYieldCurveResult", NonComposition(NoNumericAggregate)),
        ("ficant.research.v1.ArtifactService/GetArtifact:artifact->ficant.research.v1.Artifact", NonComposition(RegistryMetadata)),
        ("ficant.research.v1.ArtifactService/GetSignalSet:signal_set->ficant.research.v1.SignalSet", NonComposition(RegistryMetadata)),
        ("ficant.research.v1.ArtifactService/ReadArtifactLineage:lineage_page->ficant.research.v1.LineagePage", NonComposition(RegistryMetadata)),
        ("ficant.research.v1.ArtifactService/ReadSignalSetLineage:lineage_page->ficant.research.v1.LineagePage", NonComposition(RegistryMetadata)),
        ("ficant.research.v1.DataHealthService/GetDataHealthReport:report->ficant.research.v1.DataHealthReport", SuccessArmClass::Composition),
        ("ficant.research.v1.DataHealthService/PublishDataHealthThresholdProfile:threshold_profile->ficant.research.v1.DataHealthThresholdProfile", NonComposition(AckOrEcho)),
        ("ficant.research.v1.ExperimentService/CompareGraphRuns:response->ficant.research.v1.CompareGraphRunsResponse", NonComposition(NoNumericAggregate)),
        ("ficant.research.v1.ExperimentService/CreateRun:response->ficant.research.v1.CreateRunResponse", NonComposition(AckOrEcho)),
        ("ficant.research.v1.ExperimentService/GetGraphRun:response->ficant.research.v1.GetGraphRunResponse", NonComposition(RegistryMetadata)),
        ("ficant.research.v1.ExperimentService/GetRun:response->ficant.research.v1.GetRunResponse", NonComposition(RegistryMetadata)),
        ("ficant.research.v1.ExperimentService/ListNodeOutputManifests:response->ficant.research.v1.ListNodeOutputManifestsResponse", NonComposition(RegistryMetadata)),
        ("ficant.research.v1.ExperimentService/ReadNodeOutput:response->ficant.research.v1.ReadNodeOutputResponse", NonComposition(RegistryMetadata)),
        ("ficant.research.v1.ExperimentService/ReadRunJournal:response->ficant.research.v1.ReadRunJournalResponse", NonComposition(RegistryMetadata)),
        ("ficant.research.v1.ExperimentService/SubmitGraphRun:response->ficant.research.v1.SubmitGraphRunResponse", NonComposition(AckOrEcho)),
        ("ficant.research.v1.ExperimentService/TraceGraphOutput:response->ficant.research.v1.TraceGraphOutputResponse", NonComposition(RegistryMetadata)),
        ("ficant.research.v1.ExperimentService/TransitionRun:response->ficant.research.v1.TransitionRunResponse", NonComposition(AckOrEcho)),
        ("ficant.research.v1.FactorRegistryService/BindFactorTarget:binding->ficant.research.v1.FactorTargetBinding", NonComposition(AckOrEcho)),
        ("ficant.research.v1.FactorRegistryService/GetFactorDefinition:definition->ficant.research.v1.FactorDefinition", NonComposition(RegistryMetadata)),
        ("ficant.research.v1.FactorRegistryService/GetFactorTargets:bindings->ficant.research.v1.FactorTargetBindings", NonComposition(RegistryMetadata)),
        ("ficant.research.v1.FactorRegistryService/GetTargetFactors:definitions->ficant.research.v1.FactorDefinitions", NonComposition(RegistryMetadata)),
        ("ficant.research.v1.FactorRegistryService/RegisterCurveNodeDefinition:definition->ficant.research.v1.CurveNodeDefinition", NonComposition(AckOrEcho)),
        ("ficant.research.v1.FactorRegistryService/RegisterFactorDefinition:definition->ficant.research.v1.FactorDefinition", NonComposition(AckOrEcho)),
        ("ficant.research.v1.PortfolioRiskService/CalculateKeyRateDv01:exposure->ficant.research.v1.PortfolioKeyRateExposure", SuccessArmClass::Composition),
        ("ficant.research.v1.PositionSnapshotService/CalculateCapitalUse:capital_use->ficant.research.v1.CapitalUse", SuccessArmClass::Composition),
        ("ficant.research.v1.PositionSnapshotService/GetPositionSnapshot:snapshot->ficant.research.v1.PositionSnapshot", NonComposition(NoNumericAggregate)),
        ("ficant.research.v1.PositionSnapshotService/GetPositionViews:views->ficant.research.v1.PositionViews", SuccessArmClass::Composition),
        ("ficant.research.v1.PositionSnapshotService/PublishPositionSnapshot:snapshot->ficant.research.v1.PositionSnapshot", NonComposition(AckOrEcho)),
        ("ficant.research.v1.PositionSnapshotService/ResolvePositionSnapshot:snapshot->ficant.research.v1.PositionSnapshot", NonComposition(NoNumericAggregate)),
        ("ficant.research.v1.SnapshotService/GetSnapshot:data_snapshot->ficant.research.v1.DataSnapshot", NonComposition(NoNumericAggregate)),
        ("ficant.research.v1.SnapshotService/GetSnapshot:universe_snapshot->ficant.research.v1.UniverseSnapshot", NonComposition(NoNumericAggregate)),
        ("ficant.research.v1.SnapshotService/ImportCanonicalQuoteSnapshot:data_snapshot->ficant.research.v1.DataSnapshot", NonComposition(AckOrEcho)),
        ("ficant.research.v1.SnapshotService/PublishUniverseSnapshot:universe_snapshot->ficant.research.v1.UniverseSnapshot", NonComposition(AckOrEcho)),
    ]
    .into_iter()
    .map(|(arm, class)| (arm.to_owned(), class))
    .collect()
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
            ExpectedField::message("owner", ".ficant.core.v1.OwnerRef"),
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
            ExpectedField::message("owner", ".ficant.core.v1.OwnerRef"),
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
            ExpectedField::message("change", ".ficant.core.v1.ChangeJustification"),
            ExpectedField::scalar("idempotency_key", Type::String),
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
        &[
            ExpectedField::message("snapshot", ".ficant.core.v1.SubjectStateSnapshot"),
            ExpectedField::message("change", ".ficant.core.v1.ChangeJustification"),
            ExpectedField::scalar("idempotency_key", Type::String),
        ],
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
}

fn assert_r5d_rates_contracts(
    messages: &BTreeMap<String, &DescriptorProto>,
    enums: &BTreeMap<String, &EnumDescriptorProto>,
) {
    let owner = ".ficant.core.v1.OwnerRef";
    let version = ".ficant.core.v1.VersionRef";
    let id = ".ficant.core.v1.Ulid";
    let hash = ".ficant.core.v1.Sha256";
    let decimal = ".ficant.core.v1.DecimalValue";
    let time = ".ficant.core.v1.MarketTime";
    let object = ".ficant.rates.v1.ObjectBinding";
    let snapshot = ".ficant.rates.v1.SnapshotBinding";
    let artifact = ".ficant.rates.v1.ArtifactBinding";
    let curve_node = ".ficant.rates.v1.CurveNodeBinding";
    let context_type = ".ficant.rates.v1.AnalysisContext";

    assert_enum(
        enums,
        "ficant.rates.v1.AnalysisInputRole",
        &[
            ("ANALYSIS_INPUT_ROLE_UNSPECIFIED", 0),
            ("ANALYSIS_INPUT_ROLE_SUBJECT", 1),
            ("ANALYSIS_INPUT_ROLE_UNIT", 2),
            ("ANALYSIS_INPUT_ROLE_BOND", 3),
            ("ANALYSIS_INPUT_ROLE_CALENDAR", 4),
            ("ANALYSIS_INPUT_ROLE_CURVE_SNAPSHOT", 5),
            ("ANALYSIS_INPUT_ROLE_DATA_SNAPSHOT", 6),
            ("ANALYSIS_INPUT_ROLE_DATA_SOURCE", 7),
            ("ANALYSIS_INPUT_ROLE_TAX_RULE_PACK", 8),
            ("ANALYSIS_INPUT_ROLE_FUNDING_RULE_PACK", 9),
            ("ANALYSIS_INPUT_ROLE_DELIVERY_RULE_PACK", 10),
            ("ANALYSIS_INPUT_ROLE_FUTURES_CONTRACT", 11),
            ("ANALYSIS_INPUT_ROLE_TARGET_RISK_ARTIFACT", 12),
            ("ANALYSIS_INPUT_ROLE_DELIVERY_ARTIFACT", 13),
            ("ANALYSIS_INPUT_ROLE_CTD_ANALYTICS_ARTIFACT", 14),
            ("ANALYSIS_INPUT_ROLE_CURVE_RULE_PACK", 15),
            ("ANALYSIS_INPUT_ROLE_CURVE_NODE_DEFINITION", 16),
        ],
    );

    for removed in [
        "ficant.rates.v1.BondTerms",
        "ficant.rates.v1.CalendarBinding",
        "ficant.rates.v1.YieldCurveNode",
        "ficant.rates.v1.YieldCurveBinding",
        "ficant.rates.v1.FuturesDeliverableCandidate",
    ] {
        assert!(
            !messages.contains_key(removed),
            "R5D must remove duplicate inline contract {removed}"
        );
    }
    for removed in [
        "ficant.rates.v1.CouponFrequency",
        "ficant.rates.v1.YieldCurveInterpolation",
        "ficant.rates.v1.CgbFuturesProduct",
    ] {
        assert!(
            !enums.contains_key(removed),
            "R5D must remove unused inline enum {removed}"
        );
    }

    assert_fields(
        messages,
        "ficant.rates.v1.SnapshotBinding",
        &[
            ExpectedField::message("snapshot_id", id),
            ExpectedField::message("content_hash", hash),
        ],
    );
    assert_fields(
        messages,
        "ficant.rates.v1.ArtifactBinding",
        &[
            ExpectedField::message("artifact_id", id),
            ExpectedField::message("content_hash", hash),
        ],
    );
    assert_fields(
        messages,
        "ficant.rates.v1.CurveNodeBinding",
        &[
            ExpectedField::scalar("curve_node_id", Type::String),
            ExpectedField::message("content_hash", hash),
        ],
    );
    let curve_node_binding = messages
        .get("ficant.rates.v1.CurveNodeBinding")
        .expect("CurveNodeBinding must exist");
    assert_exact_tagged_fields(
        curve_node_binding,
        &[
            ("curve_node_id", 1, Type::String, None, false),
            ("content_hash", 2, Type::Message, Some(hash), false),
        ],
    );
    assert_fields(
        messages,
        "ficant.rates.v1.AnalysisInputBinding",
        &[
            ExpectedField::enumeration("role", ".ficant.rates.v1.AnalysisInputRole"),
            ExpectedField::message("owner", owner),
            ExpectedField::oneof_message("object", object, "binding"),
            ExpectedField::oneof_message("snapshot", snapshot, "binding"),
            ExpectedField::oneof_message("artifact", artifact, "binding"),
            ExpectedField::message("observed_at", time),
            ExpectedField::message("visible_at", time),
            ExpectedField::message("effective_from", time),
            ExpectedField::message("effective_to", time),
            ExpectedField::oneof_message("curve_node", curve_node, "binding"),
        ],
    );
    let input_binding = messages
        .get("ficant.rates.v1.AnalysisInputBinding")
        .expect("AnalysisInputBinding must exist");
    assert_exact_tagged_fields(
        input_binding,
        &[
            (
                "role",
                1,
                Type::Enum,
                Some(".ficant.rates.v1.AnalysisInputRole"),
                false,
            ),
            ("owner", 2, Type::Message, Some(owner), false),
            ("object", 3, Type::Message, Some(object), false),
            ("snapshot", 4, Type::Message, Some(snapshot), false),
            ("artifact", 5, Type::Message, Some(artifact), false),
            ("observed_at", 6, Type::Message, Some(time), false),
            ("visible_at", 7, Type::Message, Some(time), false),
            ("effective_from", 8, Type::Message, Some(time), false),
            ("effective_to", 9, Type::Message, Some(time), false),
            ("curve_node", 10, Type::Message, Some(curve_node), false),
        ],
    );
    for field in ["object", "snapshot", "artifact", "curve_node"] {
        assert_field_oneof(input_binding, field, "binding");
    }
    assert_fields(
        messages,
        "ficant.rates.v1.ParameterDigest",
        &[
            ExpectedField::message("algorithm", ".ficant.rates.v1.AlgorithmBinding"),
            ExpectedField::message("canonical_parameters_sha256", hash),
        ],
    );

    let context = messages
        .get("ficant.rates.v1.AnalysisContext")
        .expect("AnalysisContext must exist");
    assert_exact_tagged_fields(
        context,
        &[
            ("owner", 1, Type::Message, Some(owner), false),
            (
                "algorithm",
                4,
                Type::Message,
                Some(".ficant.rates.v1.AlgorithmBinding"),
                false,
            ),
            (
                "units",
                5,
                Type::Message,
                Some(".ficant.rates.v1.AnalysisUnits"),
                false,
            ),
            ("subject_ref", 6, Type::Message, Some(version), false),
            ("knowledge_at", 9, Type::Message, Some(time), false),
        ],
    );
    assert_reserved_tags(messages, "ficant.rates.v1.AnalysisContext", &[2, 3, 7, 8]);
    assert_reserved_names(
        messages,
        "ficant.rates.v1.AnalysisContext",
        &[
            "rule_pack",
            "data_snapshot",
            "funding_rule_pack",
            "tax_rule_pack",
        ],
    );

    let metadata = messages
        .get("ficant.rates.v1.ResultMetadata")
        .expect("ResultMetadata must exist");
    assert_exact_tagged_fields(
        metadata,
        &[
            ("schema_id", 1, Type::String, None, false),
            ("engine_id", 2, Type::String, None, false),
            ("engine_version", 3, Type::String, None, false),
            (
                "algorithm",
                4,
                Type::Message,
                Some(".ficant.rates.v1.AlgorithmBinding"),
                false,
            ),
            ("subject_ref", 5, Type::Message, Some(version), false),
            (
                "consumed_inputs",
                8,
                Type::Message,
                Some(".ficant.rates.v1.AnalysisInputBinding"),
                true,
            ),
            (
                "parameter_digest",
                9,
                Type::Message,
                Some(".ficant.rates.v1.ParameterDigest"),
                false,
            ),
            ("request_fingerprint", 10, Type::Message, Some(hash), false),
        ],
    );
    assert_reserved_tags(messages, "ficant.rates.v1.ResultMetadata", &[6, 7]);
    assert_reserved_names(
        messages,
        "ficant.rates.v1.ResultMetadata",
        &["funding_rule_pack", "tax_rule_pack"],
    );

    let bond = messages
        .get("ficant.rates.v1.AnalyzeBondRequest")
        .expect("AnalyzeBondRequest must exist");
    assert_exact_tagged_fields(
        bond,
        &[
            ("context", 1, Type::Message, Some(context_type), false),
            ("bond", 2, Type::Message, Some(object), false),
            ("valuation_at", 3, Type::Message, Some(time), false),
            ("settlement_date", 4, Type::String, None, false),
            (
                "calendar_requirement",
                5,
                Type::Enum,
                Some(".ficant.rates.v1.CalendarRequirement"),
                false,
            ),
            ("calendar", 6, Type::Message, Some(object), false),
            ("yield_to_maturity", 8, Type::Message, Some(decimal), false),
            ("clean_price", 9, Type::Message, Some(decimal), false),
            ("data_snapshot", 11, Type::Message, Some(snapshot), false),
            ("tax_rule_pack", 12, Type::Message, Some(object), false),
        ],
    );
    assert_field_oneof(bond, "yield_to_maturity", "input");
    assert_field_oneof(bond, "clean_price", "input");
    assert_reserved_tags(messages, "ficant.rates.v1.AnalyzeBondRequest", &[7, 10]);
    assert_reserved_names(messages, "ficant.rates.v1.AnalyzeBondRequest", &["terms"]);

    assert_fields(
        messages,
        "ficant.rates.v1.InterpolateYieldCurveRequest",
        &[
            ExpectedField::message("context", context_type),
            ExpectedField::message("curve", snapshot),
            ExpectedField::scalar("query_date", Type::String),
        ],
    );

    let carry = messages
        .get("ficant.rates.v1.AnalyzeCarryRollRequest")
        .expect("AnalyzeCarryRollRequest must exist");
    assert_exact_tagged_fields(
        carry,
        &[
            ("context", 1, Type::Message, Some(context_type), false),
            ("bond", 2, Type::Message, Some(object), false),
            ("valuation_at", 3, Type::Message, Some(time), false),
            ("initial_settlement", 4, Type::String, None, false),
            ("horizon_settlement", 5, Type::String, None, false),
            (
                "calendar_requirement",
                6,
                Type::Enum,
                Some(".ficant.rates.v1.CalendarRequirement"),
                false,
            ),
            ("curve", 9, Type::Message, Some(snapshot), false),
        ],
    );
    assert_reserved_tags(messages, "ficant.rates.v1.AnalyzeCarryRollRequest", &[7, 8]);
    assert_reserved_names(
        messages,
        "ficant.rates.v1.AnalyzeCarryRollRequest",
        &["calendar", "terms"],
    );

    let delivery = messages
        .get("ficant.rates.v1.AnalyzeFuturesDeliveryRequest")
        .expect("AnalyzeFuturesDeliveryRequest must exist");
    assert_exact_tagged_fields(
        delivery,
        &[
            ("context", 1, Type::Message, Some(context_type), false),
            ("futures_contract", 2, Type::Message, Some(object), false),
            ("valuation_at", 3, Type::Message, Some(time), false),
            ("purchase_date", 4, Type::String, None, false),
            ("data_snapshot", 11, Type::Message, Some(snapshot), false),
            ("funding_rule_pack", 12, Type::Message, Some(object), false),
            ("tax_rule_pack", 13, Type::Message, Some(object), false),
        ],
    );
    assert_reserved_tags(
        messages,
        "ficant.rates.v1.AnalyzeFuturesDeliveryRequest",
        &[5, 6, 7, 8, 9, 10],
    );
    assert_reserved_names(
        messages,
        "ficant.rates.v1.AnalyzeFuturesDeliveryRequest",
        &[
            "delivery_month_first",
            "delivery_date",
            "product",
            "futures_clean_price",
            "candidates",
        ],
    );

    let hedge = messages
        .get("ficant.rates.v1.AnalyzeFuturesHedgeRequest")
        .expect("AnalyzeFuturesHedgeRequest must exist");
    assert_exact_tagged_fields(
        hedge,
        &[
            ("context", 1, Type::Message, Some(context_type), false),
            (
                "target_risk_artifact",
                2,
                Type::Message,
                Some(artifact),
                false,
            ),
            ("delivery_artifact", 3, Type::Message, Some(artifact), false),
            (
                "ctd_analytics_artifact",
                4,
                Type::Message,
                Some(artifact),
                false,
            ),
            ("futures_contract", 5, Type::Message, Some(object), false),
            ("valuation_at", 7, Type::Message, Some(time), false),
        ],
    );
    assert_reserved_tags(
        messages,
        "ficant.rates.v1.AnalyzeFuturesHedgeRequest",
        &[6, 8, 9, 10, 11],
    );
    assert_reserved_names(
        messages,
        "ficant.rates.v1.AnalyzeFuturesHedgeRequest",
        &[
            "ctd_bond",
            "product",
            "target_dv01",
            "ctd_dv01_per_100",
            "conversion_factor",
        ],
    );

    assert_fields(
        messages,
        "ficant.rates.v1.TaxAdjustedBondAnalytics",
        &[
            ExpectedField::repeated_message("cashflows", ".ficant.rates.v1.DerivedCashflow"),
            ExpectedField::message("yield_to_maturity", decimal),
            ExpectedField::enumeration("claim_scope", ".ficant.market.v1.CouponTaxClaimScope"),
        ],
    );
    assert_fields(
        messages,
        "ficant.rates.v1.AnalyzeBondResult",
        &[
            ExpectedField::repeated_message("cashflows", ".ficant.rates.v1.DerivedCashflow"),
            ExpectedField::message("measures", ".ficant.rates.v1.BondAnalyticsMeasures"),
            ExpectedField::message("metadata", ".ficant.rates.v1.ResultMetadata"),
            ExpectedField::message("after_tax", ".ficant.rates.v1.TaxAdjustedBondAnalytics"),
        ],
    );

    let delivery_measures = messages
        .get("ficant.rates.v1.FuturesDeliveryMeasures")
        .expect("FuturesDeliveryMeasures must exist");
    assert_exact_field(
        delivery_measures,
        "funding_adjusted_irr",
        15,
        Type::Message,
        Some(decimal),
        false,
        false,
    );
    assert_exact_field(
        delivery_measures,
        "tax_adjusted_interim_coupons",
        16,
        Type::Message,
        Some(decimal),
        false,
        false,
    );
    assert_exact_field(
        delivery_measures,
        "subject_tax_adjusted_irr",
        17,
        Type::Message,
        Some(decimal),
        false,
        false,
    );
    assert_eq!(
        delivery_measures.field.len(),
        17,
        "FuturesDeliveryMeasures must retain market/funding values and add tax values"
    );
    assert_fields(
        messages,
        "ficant.rates.v1.FuturesDeliveryCandidateResult",
        &[
            ExpectedField::message("bond", object),
            ExpectedField::message("measures", ".ficant.rates.v1.FuturesDeliveryMeasures"),
            ExpectedField::enumeration("claim_scope", ".ficant.market.v1.CouponTaxClaimScope"),
        ],
    );
    let delivery_result = messages
        .get("ficant.rates.v1.AnalyzeFuturesDeliveryResult")
        .expect("AnalyzeFuturesDeliveryResult must exist");
    assert_exact_field(
        delivery_result,
        "subject_ctd_index",
        4,
        Type::Uint32,
        None,
        false,
        false,
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

    let specs: [(&str, &[ExpectedField]); 16] = [
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
            "ficant.market.v1.FuturesContract",
            &[
                ExpectedField::message("instrument", version),
                ExpectedField::message("last_trade_time", time),
                ExpectedField::message("expiry_time", time),
                ExpectedField::message("settlement_time", time),
                ExpectedField::message("multiplier", decimal),
                ExpectedField::message("rule_pack", version),
                ExpectedField::scalar("product_code", Type::String),
                ExpectedField::message("price_unit", ".ficant.core.v1.UnitRef"),
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
                ExpectedField::enumeration("cashflow_type", ".ficant.market.v1.CashflowType"),
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
                ExpectedField::message("visible_at", time),
                ExpectedField::scalar("curve_family_id", Type::String),
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
                ExpectedField::message("content", ".google.protobuf.Any"),
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
                ExpectedField::message("authorization_ref", version),
                ExpectedField::message("actor_id", id),
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
                ExpectedField::message("actor_id", id),
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
    assert_bond_contract(messages);
}

fn assert_bond_contract(messages: &BTreeMap<String, &DescriptorProto>) {
    let bond = messages
        .get("ficant.market.v1.Bond")
        .expect("Bond must exist");
    assert_exact_field(
        bond,
        "instrument",
        1,
        Type::Message,
        Some(".ficant.core.v1.VersionRef"),
        false,
        false,
    );
    assert_exact_field(bond, "maturity_date", 3, Type::String, None, false, false);
    assert_exact_field(
        bond,
        "face_value",
        4,
        Type::Message,
        Some(".ficant.core.v1.DecimalValue"),
        false,
        false,
    );
    assert_exact_field(
        bond,
        "first_issue_date",
        5,
        Type::String,
        None,
        false,
        false,
    );
    assert_exact_field(
        bond,
        "current_issue_date",
        6,
        Type::String,
        None,
        false,
        false,
    );
    assert_exact_field(
        bond,
        "cumulative_issued_amount",
        7,
        Type::Message,
        Some(".ficant.core.v1.DecimalValue"),
        false,
        false,
    );
    assert_exact_field(
        bond,
        "tax_attributes",
        8,
        Type::Message,
        Some(".ficant.market.v1.BondTaxAttributes"),
        false,
        false,
    );
    assert_exact_field(
        bond,
        "coupon_rate",
        9,
        Type::Message,
        Some(".ficant.core.v1.DecimalValue"),
        false,
        false,
    );
    assert_exact_field(
        bond,
        "coupon_frequency",
        10,
        Type::Enum,
        Some(".ficant.market.v1.BondCouponFrequency"),
        false,
        false,
    );
    assert_exact_field(
        bond,
        "day_count",
        11,
        Type::Enum,
        Some(".ficant.market.v1.BondDayCountConvention"),
        false,
        false,
    );
    assert_exact_field(
        bond,
        "business_day",
        12,
        Type::Enum,
        Some(".ficant.market.v1.BondBusinessDayConvention"),
        false,
        false,
    );
    assert_eq!(bond.field.len(), 11, "Bond field drift");
    assert_reserved_tag(messages, "ficant.market.v1.Bond", 2);
    assert_fields(
        messages,
        "ficant.market.v1.BondTaxAttributes",
        &[
            ExpectedField::enumeration(
                "value_added_tax_status",
                ".ficant.market.v1.ValueAddedTaxStatus",
            ),
            ExpectedField::enumeration("income_tax_status", ".ficant.market.v1.IncomeTaxStatus"),
        ],
    );
}

fn assert_cgb_futures_rule_pack_contract(messages: &BTreeMap<String, &DescriptorProto>) {
    let pack = messages
        .get("ficant.market.v1.CgbFuturesDeliveryRulePack")
        .expect("CgbFuturesDeliveryRulePack must exist");
    assert_exact_field(
        pack,
        "products",
        1,
        Type::Message,
        Some(".ficant.market.v1.CgbFuturesProductRule"),
        true,
        false,
    );
    assert_exact_field(pack, "delivery_months", 2, Type::Uint32, None, true, false);
    assert_exact_field(
        pack,
        "nominal_coupon",
        3,
        Type::Message,
        Some(".ficant.core.v1.DecimalValue"),
        false,
        false,
    );
    assert_exact_field(
        pack,
        "face_quote_basis",
        4,
        Type::Message,
        Some(".ficant.core.v1.DecimalValue"),
        false,
        false,
    );
    for (name, number) in [
        ("accrued_interest_day_count", 5),
        ("conversion_factor_rounding_places", 6),
        ("accrued_interest_rounding_places", 7),
        ("annual_day_basis", 8),
    ] {
        assert_exact_field(pack, name, number, Type::Uint32, None, false, true);
    }
    assert_eq!(
        pack.field.len(),
        8,
        "CgbFuturesDeliveryRulePack field drift"
    );

    let product = messages
        .get("ficant.market.v1.CgbFuturesProductRule")
        .expect("CgbFuturesProductRule must exist");
    for (name, number, field_type) in [
        ("product_code", 1, Type::String),
        ("original_term_max_months", 2, Type::Uint32),
        ("residual_min_months", 3, Type::Uint32),
    ] {
        assert_exact_field(product, name, number, field_type, None, false, true);
    }
    assert_exact_field(
        product,
        "residual_max_months",
        4,
        Type::Uint32,
        None,
        false,
        false,
    );
    assert_exact_field(
        product,
        "residual_max_months_unbounded",
        5,
        Type::Bool,
        None,
        false,
        false,
    );
    assert_exact_field(
        product,
        "contract_size_in_quote_units",
        6,
        Type::Uint32,
        None,
        false,
        true,
    );
    assert_eq!(product.field.len(), 6, "CgbFuturesProductRule field drift");
    let residual_oneof = product
        .oneof_decl
        .iter()
        .position(|oneof| oneof.name() == "residual_upper_bound")
        .expect("residual upper bound must be a real oneof") as i32;
    for name in ["residual_max_months", "residual_max_months_unbounded"] {
        let field = product
            .field
            .iter()
            .find(|field| field.name() == name)
            .expect("residual oneof field must exist");
        assert_eq!(field.oneof_index, Some(residual_oneof));
    }
}

fn assert_funding_rule_pack_contract(messages: &BTreeMap<String, &DescriptorProto>) {
    let pack = messages
        .get("ficant.market.v1.FundingRulePack")
        .expect("FundingRulePack must exist");
    assert_exact_field(
        pack,
        "rates",
        1,
        Type::Message,
        Some(".ficant.market.v1.FundingTierRate"),
        true,
        false,
    );
    assert_eq!(pack.field.len(), 1, "FundingRulePack field drift");

    let rate = messages
        .get("ficant.market.v1.FundingTierRate")
        .expect("FundingTierRate must exist");
    assert_exact_field(
        rate,
        "funding_tier",
        1,
        Type::Enum,
        Some(".ficant.core.v1.FundingTier"),
        false,
        false,
    );
    assert_exact_field(
        rate,
        "annual_financing_rate",
        2,
        Type::Message,
        Some(".ficant.core.v1.DecimalValue"),
        false,
        false,
    );
    assert_eq!(rate.field.len(), 2, "FundingTierRate field drift");
}

fn assert_tax_rule_pack_contract(
    messages: &BTreeMap<String, &DescriptorProto>,
    enums: &BTreeMap<String, &EnumDescriptorProto>,
) {
    let pack = messages
        .get("ficant.market.v1.TaxRulePack")
        .expect("TaxRulePack must exist");
    assert_exact_field(
        pack,
        "coupon_rules",
        1,
        Type::Message,
        Some(".ficant.market.v1.BondCouponTaxRule"),
        true,
        false,
    );
    assert_eq!(pack.field.len(), 1, "TaxRulePack field drift");
    assert_fields(
        messages,
        "ficant.market.v1.BondCouponTaxRule",
        &[
            ExpectedField::scalar("first_issue_from", Type::String),
            ExpectedField::scalar("first_issue_to", Type::String),
            ExpectedField::message("tax_attributes", ".ficant.market.v1.BondTaxAttributes"),
            ExpectedField::repeated_message("rates", ".ficant.market.v1.SubjectCouponTaxRate"),
        ],
    );
    assert_fields(
        messages,
        "ficant.market.v1.SubjectCouponTaxRate",
        &[
            ExpectedField::scalar("value_added_tax_profile", Type::String),
            ExpectedField::scalar("income_tax_profile", Type::String),
            ExpectedField::message("coupon_tax_rate", ".ficant.core.v1.DecimalValue"),
        ],
    );

    let pack_v2 = messages
        .get("ficant.market.v1.TaxRulePackV2")
        .expect("TaxRulePackV2 must exist");
    assert_exact_field(
        pack_v2,
        "coupon_rules",
        1,
        Type::Message,
        Some(".ficant.market.v1.BondCouponTaxTreatmentRule"),
        true,
        false,
    );
    assert_eq!(pack_v2.field.len(), 1, "TaxRulePackV2 field drift");
    assert_fields(
        messages,
        "ficant.market.v1.BondCouponTaxTreatmentRule",
        &[
            ExpectedField::scalar("first_issue_from", Type::String),
            ExpectedField::scalar("first_issue_to", Type::String),
            ExpectedField::message("tax_attributes", ".ficant.market.v1.BondTaxAttributes"),
            ExpectedField::repeated_message(
                "treatments",
                ".ficant.market.v1.SubjectCouponTaxTreatment",
            ),
        ],
    );
    assert_fields(
        messages,
        "ficant.market.v1.SubjectCouponTaxTreatment",
        &[
            ExpectedField::scalar("value_added_tax_profile", Type::String),
            ExpectedField::scalar("income_tax_profile", Type::String),
            ExpectedField::message("value_added_tax_rate", ".ficant.core.v1.DecimalValue"),
            ExpectedField::message("income_tax_rate", ".ficant.core.v1.DecimalValue"),
            ExpectedField::enumeration(
                "gross_coupon_basis",
                ".ficant.market.v1.GrossCouponTaxBasis",
            ),
            ExpectedField::enumeration("rounding", ".ficant.market.v1.TaxRoundingMode"),
            ExpectedField::enumeration("claim_scope", ".ficant.market.v1.CouponTaxClaimScope"),
        ],
    );
    assert_enum(
        enums,
        "ficant.market.v1.GrossCouponTaxBasis",
        &[
            ("GROSS_COUPON_TAX_BASIS_UNSPECIFIED", 0),
            ("GROSS_COUPON_TAX_BASIS_VAT_INCLUDED", 1),
        ],
    );
    assert_enum(
        enums,
        "ficant.market.v1.TaxRoundingMode",
        &[
            ("TAX_ROUNDING_MODE_UNSPECIFIED", 0),
            ("TAX_ROUNDING_MODE_TIES_TO_EVEN", 1),
        ],
    );
    assert_enum(
        enums,
        "ficant.market.v1.CouponTaxClaimScope",
        &[
            ("COUPON_TAX_CLAIM_SCOPE_UNSPECIFIED", 0),
            (
                "COUPON_TAX_CLAIM_SCOPE_COUPON_OUTPUT_VAT_BEFORE_INPUT_CREDIT",
                1,
            ),
        ],
    );
}

fn assert_reserved_tag(
    messages: &BTreeMap<String, &DescriptorProto>,
    message_name: &str,
    tag: i32,
) {
    let message = messages
        .get(message_name)
        .unwrap_or_else(|| panic!("missing message {message_name}"));
    assert!(
        message
            .reserved_range
            .iter()
            .any(|range| range.start() <= tag && tag < range.end()),
        "{message_name} must reserve removed tag {tag}"
    );
}

fn assert_reserved_tags(
    messages: &BTreeMap<String, &DescriptorProto>,
    message_name: &str,
    tags: &[i32],
) {
    let message = messages
        .get(message_name)
        .unwrap_or_else(|| panic!("missing message {message_name}"));
    let actual = message
        .reserved_range
        .iter()
        .flat_map(|range| range.start()..range.end())
        .collect::<BTreeSet<_>>();
    let expected = tags.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(
        actual, expected,
        "{message_name} must reserve the exact removed field tags"
    );
}

fn assert_reserved_names(
    messages: &BTreeMap<String, &DescriptorProto>,
    message_name: &str,
    names: &[&str],
) {
    let message = messages
        .get(message_name)
        .unwrap_or_else(|| panic!("missing message {message_name}"));
    let actual = message
        .reserved_name
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let expected = names.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(
        actual, expected,
        "{message_name} must reserve the exact removed field names"
    );
}

fn assert_exact_tagged_fields(
    message: &DescriptorProto,
    expected: &[(&str, i32, Type, Option<&str>, bool)],
) {
    assert_eq!(
        message.field.len(),
        expected.len(),
        "{} must have the exact frozen field count",
        message.name()
    );
    for (name, number, field_type, type_name, repeated) in expected {
        assert_exact_field(
            message,
            name,
            *number,
            *field_type,
            *type_name,
            *repeated,
            false,
        );
    }
}

fn assert_field_oneof(message: &DescriptorProto, field_name: &str, oneof_name: &str) {
    let field = message
        .field
        .iter()
        .find(|field| field.name() == field_name)
        .unwrap_or_else(|| panic!("{}.{} must exist", message.name(), field_name));
    let oneof_index = field
        .oneof_index
        .unwrap_or_else(|| panic!("{}.{} must belong to a oneof", message.name(), field_name));
    assert_eq!(
        message.oneof_decl[oneof_index as usize].name(),
        oneof_name,
        "{}.{} oneof drift",
        message.name(),
        field_name
    );
}

fn assert_exact_field(
    message: &DescriptorProto,
    name: &str,
    number: i32,
    field_type: Type,
    type_name: Option<&str>,
    repeated: bool,
    proto3_optional: bool,
) {
    let field = message
        .field
        .iter()
        .find(|field| field.name() == name)
        .unwrap_or_else(|| panic!("{}.{} must exist", message.name(), name));
    assert_eq!(
        field.number(),
        number,
        "{}.{} tag drift",
        message.name(),
        name
    );
    assert_eq!(
        field.r#type(),
        field_type,
        "{}.{} type drift",
        message.name(),
        name
    );
    assert_eq!(
        field.type_name.as_deref(),
        type_name,
        "{}.{} type target drift",
        message.name(),
        name
    );
    assert_eq!(
        field.label() == Label::Repeated,
        repeated,
        "{}.{} cardinality drift",
        message.name(),
        name
    );
    assert_eq!(
        field.proto3_optional(),
        proto3_optional,
        "{}.{} presence drift",
        message.name(),
        name
    );
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
            ExpectedField::message("actor_id", ".ficant.core.v1.Ulid"),
            ExpectedField::enumeration("active_role", ".ficant.core.v1.PlatformRole"),
            ExpectedField::message("tenant_id", ".ficant.core.v1.Ulid"),
            ExpectedField::repeated_message("allowed_owner_ids", ".ficant.core.v1.Ulid"),
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

fn assert_governance_contracts(
    descriptor_set: &FileDescriptorSet,
    messages: &BTreeMap<String, &DescriptorProto>,
    enums: &BTreeMap<String, &EnumDescriptorProto>,
) {
    assert_enum(
        enums,
        "ficant.core.v1.PlatformRole",
        &[
            ("PLATFORM_ROLE_UNSPECIFIED", 0),
            ("PLATFORM_ROLE_PLATFORM_ADMIN", 1),
            ("PLATFORM_ROLE_RESEARCHER", 2),
        ],
    );
    assert_fields(
        messages,
        "ficant.core.v1.SourceDocumentRef",
        &[
            ExpectedField::scalar("uri", Type::String),
            ExpectedField::message("sha256", ".ficant.core.v1.Sha256"),
        ],
    );
    assert_fields(
        messages,
        "ficant.core.v1.ChangeJustification",
        &[
            ExpectedField::scalar("reason", Type::String),
            ExpectedField::repeated_message("sources", ".ficant.core.v1.SourceDocumentRef"),
        ],
    );
    assert_fields(
        messages,
        "ficant.core.v1.FoundationChangeRecord",
        &[
            ExpectedField::message("record_id", ".ficant.core.v1.Ulid"),
            ExpectedField::message("actor_id", ".ficant.core.v1.Ulid"),
            ExpectedField::enumeration("active_role", ".ficant.core.v1.PlatformRole"),
            ExpectedField::scalar("operation", Type::String),
            ExpectedField::scalar("resource_ref", Type::String),
            ExpectedField::message("before_hash", ".ficant.core.v1.Sha256"),
            ExpectedField::message("after_hash", ".ficant.core.v1.Sha256"),
            ExpectedField::message("change", ".ficant.core.v1.ChangeJustification"),
            ExpectedField::message("request_fingerprint", ".ficant.core.v1.Sha256"),
            ExpectedField::message("occurred_at", ".google.protobuf.Timestamp"),
            ExpectedField::message("authorization_ref", ".ficant.core.v1.VersionRef"),
        ],
    );
    assert_fields(
        messages,
        "ficant.core.v1.GetFoundationChangeRequest",
        &[ExpectedField::message("record_id", ".ficant.core.v1.Ulid")],
    );
    assert_fields(
        messages,
        "ficant.core.v1.GetFoundationChangeResponse",
        &[
            ExpectedField::oneof_message(
                "change",
                ".ficant.core.v1.FoundationChangeRecord",
                "result",
            ),
            ExpectedField::oneof_message("error", ".ficant.core.v1.ErrorDetail", "result"),
        ],
    );
    assert_fields(
        messages,
        "ficant.core.v1.ListFoundationChangesRequest",
        &[
            ExpectedField::scalar("resource_ref", Type::String),
            ExpectedField::message("actor_id", ".ficant.core.v1.Ulid"),
            ExpectedField::message("occurred_from", ".google.protobuf.Timestamp"),
            ExpectedField::message("occurred_to", ".google.protobuf.Timestamp"),
            ExpectedField::message("page", ".ficant.core.v1.PageRequest"),
        ],
    );
    assert_fields(
        messages,
        "ficant.core.v1.FoundationChangeRecords",
        &[
            ExpectedField::repeated_message("changes", ".ficant.core.v1.FoundationChangeRecord"),
            ExpectedField::message("page", ".ficant.core.v1.PageResponse"),
        ],
    );
    assert_fields(
        messages,
        "ficant.core.v1.ListFoundationChangesResponse",
        &[
            ExpectedField::oneof_message(
                "changes",
                ".ficant.core.v1.FoundationChangeRecords",
                "result",
            ),
            ExpectedField::oneof_message("error", ".ficant.core.v1.ErrorDetail", "result"),
        ],
    );
    assert_exact_service(
        descriptor_set,
        "ficant.core.v1.FoundationChangeService",
        &[
            ExpectedMethod::new(
                "GetFoundationChange",
                ".ficant.core.v1.GetFoundationChangeRequest",
                ".ficant.core.v1.GetFoundationChangeResponse",
            ),
            ExpectedMethod::new(
                "ListFoundationChanges",
                ".ficant.core.v1.ListFoundationChangesRequest",
                ".ficant.core.v1.ListFoundationChangesResponse",
            ),
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

    for removed in [
        "ficant.market.v1.AppendInstrumentRequest",
        "ficant.market.v1.AppendInstrumentResponse",
        "ficant.market.v1.AppendBondRequest",
        "ficant.market.v1.AppendBondResponse",
        "ficant.market.v1.AppendFuturesContractRequest",
        "ficant.market.v1.AppendFuturesContractResponse",
        "ficant.market.v1.AppendCalendarRequest",
        "ficant.market.v1.AppendCalendarResponse",
        "ficant.market.v1.AppendUnitRequest",
        "ficant.market.v1.AppendUnitResponse",
        "ficant.market.v1.AppendMarketRulePackRequest",
        "ficant.market.v1.AppendMarketRulePackResponse",
        "ficant.market.v1.AppendCashflowRequest",
        "ficant.market.v1.AppendCashflowResponse",
        "ficant.market.v1.AppendQuoteRequest",
        "ficant.market.v1.AppendQuoteResponse",
        "ficant.market.v1.AppendTradeRequest",
        "ficant.market.v1.AppendTradeResponse",
        "ficant.market.v1.AppendValuationRequest",
        "ficant.market.v1.AppendValuationResponse",
        "ficant.research.v1.PublishDataSnapshotRequest",
        "ficant.research.v1.PublishDataSnapshotResponse",
    ] {
        assert!(
            !messages.contains_key(removed),
            "removed pre-production write contract {removed} must not survive as a compatibility shim"
        );
    }

    assert_fields(
        messages,
        "ficant.market.v1.CompleteInstrumentDefinition",
        &[
            ExpectedField::message("instrument", ".ficant.market.v1.Instrument"),
            ExpectedField::oneof_message("bond", ".ficant.market.v1.Bond", "subtype"),
            ExpectedField::oneof_message(
                "futures_contract",
                ".ficant.market.v1.FuturesContract",
                "subtype",
            ),
        ],
    );
    assert_reserved_tags(messages, "ficant.market.v1.MarketDefinition", &[2, 3]);
    assert_reserved_names(
        messages,
        "ficant.market.v1.MarketDefinition",
        &["bond", "futures_contract"],
    );
    let market_definition = messages
        .get("ficant.market.v1.MarketDefinition")
        .expect("MarketDefinition exists");
    assert_exact_tagged_fields(
        market_definition,
        &[
            (
                "instrument",
                1,
                Type::Message,
                Some(".ficant.market.v1.CompleteInstrumentDefinition"),
                false,
            ),
            (
                "calendar",
                4,
                Type::Message,
                Some(".ficant.market.v1.Calendar"),
                false,
            ),
            (
                "unit",
                5,
                Type::Message,
                Some(".ficant.market.v1.Unit"),
                false,
            ),
            (
                "market_rule_pack",
                6,
                Type::Message,
                Some(".ficant.market.v1.MarketRulePack"),
                false,
            ),
        ],
    );
    for field in ["instrument", "calendar", "unit", "market_rule_pack"] {
        assert_field_oneof(market_definition, field, "definition");
    }
    assert_fields(
        messages,
        "ficant.market.v1.AppendDefinitionRequest",
        &[
            ExpectedField::scalar("idempotency_key", Type::String),
            ExpectedField::scalar("expected_latest_version", Type::Uint64),
            ExpectedField::message("definition", ".ficant.market.v1.MarketDefinition"),
            ExpectedField::message("change", ".ficant.core.v1.ChangeJustification"),
        ],
    );
    assert_fields(
        messages,
        "ficant.market.v1.AppendDefinitionResponse",
        &[
            ExpectedField::oneof_message(
                "definition",
                ".ficant.market.v1.MarketDefinition",
                "result",
            ),
            ExpectedField::oneof_message("error", ".ficant.core.v1.ErrorDetail", "result"),
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
        &[
            ExpectedField::oneof_message(
                "definition",
                ".ficant.market.v1.MarketDefinition",
                "result",
            ),
            ExpectedField::oneof_message("error", ".ficant.core.v1.ErrorDetail", "result"),
        ],
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
        &[
            ExpectedField::oneof_message(
                "definition",
                ".ficant.market.v1.MarketDefinition",
                "result",
            ),
            ExpectedField::oneof_message("error", ".ficant.core.v1.ErrorDetail", "result"),
        ],
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
        "ficant.market.v1.DefinitionVersions",
        &[
            ExpectedField::repeated_message("definitions", ".ficant.market.v1.MarketDefinition"),
            ExpectedField::message("page", page_response),
        ],
    );
    let list_definition_response = messages
        .get("ficant.market.v1.ListDefinitionVersionsResponse")
        .expect("ListDefinitionVersionsResponse exists");
    assert_reserved_tags(
        messages,
        "ficant.market.v1.ListDefinitionVersionsResponse",
        &[1, 2],
    );
    assert_reserved_names(
        messages,
        "ficant.market.v1.ListDefinitionVersionsResponse",
        &["definitions", "page"],
    );
    assert_exact_tagged_fields(
        list_definition_response,
        &[
            (
                "versions",
                3,
                Type::Message,
                Some(".ficant.market.v1.DefinitionVersions"),
                false,
            ),
            (
                "error",
                4,
                Type::Message,
                Some(".ficant.core.v1.ErrorDetail"),
                false,
            ),
        ],
    );
    assert_field_oneof(list_definition_response, "versions", "result");
    assert_field_oneof(list_definition_response, "error", "result");

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
        "ficant.market.v1.AppendMarketFactRequest",
        &[
            ExpectedField::scalar("idempotency_key", Type::String),
            ExpectedField::message("fact", ".ficant.market.v1.MarketFact"),
            ExpectedField::message("change", ".ficant.core.v1.ChangeJustification"),
        ],
    );
    for response in [
        "ficant.market.v1.AppendMarketFactResponse",
        "ficant.market.v1.CorrectMarketFactResponse",
    ] {
        assert_fields(
            messages,
            response,
            &[
                ExpectedField::oneof_message("fact", ".ficant.market.v1.MarketFact", "result"),
                ExpectedField::oneof_message("error", ".ficant.core.v1.ErrorDetail", "result"),
            ],
        );
    }
    assert_fields(
        messages,
        "ficant.market.v1.CorrectMarketFactRequest",
        &[
            ExpectedField::scalar("idempotency_key", Type::String),
            ExpectedField::message("original_fact_id", id),
            ExpectedField::message("fact", ".ficant.market.v1.MarketFact"),
            ExpectedField::message("change", ".ficant.core.v1.ChangeJustification"),
        ],
    );
    assert_fields(
        messages,
        "ficant.market.v1.CurveSnapshotInput",
        &[
            ExpectedField::message("curve_snapshot_id", ".ficant.core.v1.Ulid"),
            ExpectedField::message("owner", ".ficant.core.v1.OwnerRef"),
            ExpectedField::message("as_of", ".ficant.core.v1.MarketTime"),
            ExpectedField::message("currency", ".ficant.core.v1.UnitRef"),
            ExpectedField::scalar("curve_kind", Type::String),
            ExpectedField::message("calendar", ".ficant.core.v1.VersionRef"),
            ExpectedField::message("rule_pack", ".ficant.core.v1.VersionRef"),
            ExpectedField::scalar("point_schema", Type::String),
            ExpectedField::repeated_message("lineage", ".ficant.core.v1.LineageRef"),
            ExpectedField::message("visible_at", ".ficant.core.v1.MarketTime"),
            ExpectedField::scalar("curve_family_id", Type::String),
        ],
    );
    let publish_curve_request = messages
        .get("ficant.market.v1.PublishCurveSnapshotRequest")
        .expect("PublishCurveSnapshotRequest exists");
    assert_reserved_tag(messages, "ficant.market.v1.PublishCurveSnapshotRequest", 2);
    assert_reserved_names(
        messages,
        "ficant.market.v1.PublishCurveSnapshotRequest",
        &["curve_snapshot"],
    );
    assert_exact_tagged_fields(
        publish_curve_request,
        &[
            ("idempotency_key", 1, Type::String, None, false),
            (
                "points",
                3,
                Type::Message,
                Some(".ficant.market.v1.CurvePointSet"),
                false,
            ),
            (
                "change",
                4,
                Type::Message,
                Some(".ficant.core.v1.ChangeJustification"),
                false,
            ),
            (
                "curve",
                5,
                Type::Message,
                Some(".ficant.market.v1.CurveSnapshotInput"),
                false,
            ),
        ],
    );
    assert_fields(
        messages,
        "ficant.market.v1.PublishCurveSnapshotResponse",
        &[
            ExpectedField::oneof_message(
                "curve_snapshot",
                ".ficant.market.v1.CurveSnapshot",
                "result",
            ),
            ExpectedField::oneof_message("error", ".ficant.core.v1.ErrorDetail", "result"),
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
            ExpectedField::message("knowledge_at", ".ficant.core.v1.MarketTime"),
        ],
    );
    assert_fields(
        messages,
        "ficant.market.v1.InstrumentFacts",
        &[
            ExpectedField::repeated_message("facts", ".ficant.market.v1.MarketFact"),
            ExpectedField::message("page", page_response),
        ],
    );
    let query_facts_response = messages
        .get("ficant.market.v1.QueryInstrumentFactsResponse")
        .expect("QueryInstrumentFactsResponse exists");
    assert_reserved_tags(
        messages,
        "ficant.market.v1.QueryInstrumentFactsResponse",
        &[1, 2],
    );
    assert_reserved_names(
        messages,
        "ficant.market.v1.QueryInstrumentFactsResponse",
        &["facts", "page"],
    );
    assert_exact_tagged_fields(
        query_facts_response,
        &[
            (
                "instrument_facts",
                3,
                Type::Message,
                Some(".ficant.market.v1.InstrumentFacts"),
                false,
            ),
            (
                "error",
                4,
                Type::Message,
                Some(".ficant.core.v1.ErrorDetail"),
                false,
            ),
        ],
    );
    assert_field_oneof(query_facts_response, "instrument_facts", "result");
    assert_field_oneof(query_facts_response, "error", "result");
    assert_fields(
        messages,
        "ficant.market.v1.GetCurveSnapshotRequest",
        &[
            ExpectedField::message("curve_snapshot_id", id),
            ExpectedField::message("knowledge_at", ".ficant.core.v1.MarketTime"),
        ],
    );
    assert_fields(
        messages,
        "ficant.market.v1.CurveSnapshotPayload",
        &[
            ExpectedField::message("curve_snapshot", ".ficant.market.v1.CurveSnapshot"),
            ExpectedField::message("points", ".ficant.market.v1.CurvePointSet"),
        ],
    );
    let get_curve_response = messages
        .get("ficant.market.v1.GetCurveSnapshotResponse")
        .expect("GetCurveSnapshotResponse exists");
    assert_reserved_tag(messages, "ficant.market.v1.GetCurveSnapshotResponse", 1);
    assert_reserved_names(
        messages,
        "ficant.market.v1.GetCurveSnapshotResponse",
        &["curve_snapshot"],
    );
    assert_exact_tagged_fields(
        get_curve_response,
        &[
            (
                "curve",
                2,
                Type::Message,
                Some(".ficant.market.v1.CurveSnapshotPayload"),
                false,
            ),
            (
                "error",
                3,
                Type::Message,
                Some(".ficant.core.v1.ErrorDetail"),
                false,
            ),
        ],
    );
    assert_field_oneof(get_curve_response, "curve", "result");
    assert_field_oneof(get_curve_response, "error", "result");

    assert_r6b_artifact_messages(messages, id, page_request, page_response);
}

fn assert_r6b_artifact_contracts(
    messages: &BTreeMap<String, &DescriptorProto>,
    enums: &BTreeMap<String, &EnumDescriptorProto>,
) {
    for removed in [
        "ficant.research.v1.PublishArtifactRequest",
        "ficant.research.v1.PublishArtifactResponse",
        "ficant.research.v1.PublishSignalSetRequest",
        "ficant.research.v1.PublishSignalSetResponse",
    ] {
        assert!(
            !messages.contains_key(removed),
            "R6B removes the dishonest public publish message {removed}"
        );
    }
    let enumeration = enums
        .get("ficant.research.v1.ArtifactKind")
        .expect("ArtifactKind exists");
    let reserved_ranges = enumeration
        .reserved_range
        .iter()
        .map(|range| (range.start(), range.end()))
        .collect::<Vec<_>>();
    assert_eq!(
        reserved_ranges,
        vec![(2, 4)],
        "ArtifactKind must reserve the exact orphan numeric range 2..=4"
    );
    let reserved_names = enumeration
        .reserved_name
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        reserved_names,
        BTreeSet::from([
            "ARTIFACT_KIND_CURVE_SNAPSHOT",
            "ARTIFACT_KIND_DATA_SNAPSHOT",
            "ARTIFACT_KIND_UNIVERSE_SNAPSHOT",
        ]),
        "ArtifactKind must reserve all removed orphan names"
    );
}

fn assert_r6b_artifact_messages(
    messages: &BTreeMap<String, &DescriptorProto>,
    id: &'static str,
    page_request: &'static str,
    page_response: &'static str,
) {
    assert_fields(
        messages,
        "ficant.research.v1.GetArtifactRequest",
        &[ExpectedField::message("artifact_id", id)],
    );
    for (message, success_field, success_type) in [
        (
            "ficant.research.v1.GetArtifactResponse",
            "artifact",
            ".ficant.research.v1.Artifact",
        ),
        (
            "ficant.research.v1.GetSignalSetResponse",
            "signal_set",
            ".ficant.research.v1.SignalSet",
        ),
    ] {
        let response = messages
            .get(message)
            .unwrap_or_else(|| panic!("missing {message}"));
        assert_exact_tagged_fields(
            response,
            &[
                (success_field, 1, Type::Message, Some(success_type), false),
                (
                    "error",
                    2,
                    Type::Message,
                    Some(".ficant.core.v1.ErrorDetail"),
                    false,
                ),
            ],
        );
        assert_field_oneof(response, success_field, "result");
        assert_field_oneof(response, "error", "result");
    }
    assert_fields(
        messages,
        "ficant.research.v1.GetSignalSetRequest",
        &[ExpectedField::message("signal_set_id", id)],
    );
    assert_fields(
        messages,
        "ficant.research.v1.LineagePage",
        &[
            ExpectedField::repeated_message("lineage", ".ficant.core.v1.LineageRef"),
            ExpectedField::message("page", page_response),
        ],
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
        let response = messages
            .get(message)
            .unwrap_or_else(|| panic!("missing {message}"));
        assert_exact_tagged_fields(
            response,
            &[
                (
                    "lineage_page",
                    1,
                    Type::Message,
                    Some(".ficant.research.v1.LineagePage"),
                    false,
                ),
                (
                    "error",
                    2,
                    Type::Message,
                    Some(".ficant.core.v1.ErrorDetail"),
                    false,
                ),
            ],
        );
        assert_field_oneof(response, "lineage_page", "result");
        assert_field_oneof(response, "error", "result");
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
        "ficant.market.v1.CashflowType",
        &[
            ("CASHFLOW_TYPE_UNSPECIFIED", 0),
            ("CASHFLOW_TYPE_COUPON", 1),
            ("CASHFLOW_TYPE_PRINCIPAL", 2),
            ("CASHFLOW_TYPE_FEE", 3),
            ("CASHFLOW_TYPE_OTHER", 4),
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
        "ficant.market.v1.ValueAddedTaxStatus",
        &[
            ("VALUE_ADDED_TAX_STATUS_UNSPECIFIED", 0),
            ("VALUE_ADDED_TAX_STATUS_EXEMPT", 1),
            ("VALUE_ADDED_TAX_STATUS_TAXABLE", 2),
        ],
    );
    assert_enum(
        enums,
        "ficant.market.v1.IncomeTaxStatus",
        &[
            ("INCOME_TAX_STATUS_UNSPECIFIED", 0),
            ("INCOME_TAX_STATUS_EXEMPT", 1),
            ("INCOME_TAX_STATUS_TAXABLE", 2),
        ],
    );
    assert_enum(
        enums,
        "ficant.research.v1.ArtifactKind",
        &[
            ("ARTIFACT_KIND_UNSPECIFIED", 0),
            ("ARTIFACT_KIND_GENERIC", 1),
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

fn factor_registry_methods() -> Vec<ExpectedMethod> {
    vec![
        ExpectedMethod::new(
            "RegisterFactorDefinition",
            ".ficant.research.v1.RegisterFactorDefinitionRequest",
            ".ficant.research.v1.RegisterFactorDefinitionResponse",
        ),
        ExpectedMethod::new(
            "RegisterCurveNodeDefinition",
            ".ficant.research.v1.RegisterCurveNodeDefinitionRequest",
            ".ficant.research.v1.RegisterCurveNodeDefinitionResponse",
        ),
        ExpectedMethod::new(
            "BindFactorTarget",
            ".ficant.research.v1.BindFactorTargetRequest",
            ".ficant.research.v1.BindFactorTargetResponse",
        ),
        ExpectedMethod::new(
            "GetFactorDefinition",
            ".ficant.research.v1.GetFactorDefinitionRequest",
            ".ficant.research.v1.GetFactorDefinitionResponse",
        ),
        ExpectedMethod::new(
            "GetFactorTargets",
            ".ficant.research.v1.GetFactorTargetsRequest",
            ".ficant.research.v1.GetFactorTargetsResponse",
        ),
        ExpectedMethod::new(
            "GetTargetFactors",
            ".ficant.research.v1.GetTargetFactorsRequest",
            ".ficant.research.v1.GetTargetFactorsResponse",
        ),
    ]
}

fn market_definition_methods() -> Vec<ExpectedMethod> {
    vec![
        ExpectedMethod::new(
            "AppendDefinition",
            ".ficant.market.v1.AppendDefinitionRequest",
            ".ficant.market.v1.AppendDefinitionResponse",
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
            "AppendMarketFact",
            ".ficant.market.v1.AppendMarketFactRequest",
            ".ficant.market.v1.AppendMarketFactResponse",
        ),
        ExpectedMethod::new(
            "CorrectMarketFact",
            ".ficant.market.v1.CorrectMarketFactRequest",
            ".ficant.market.v1.CorrectMarketFactResponse",
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
            "GetArtifact",
            ".ficant.research.v1.GetArtifactRequest",
            ".ficant.research.v1.GetArtifactResponse",
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
            "ImportCanonicalQuoteSnapshot",
            ".ficant.research.v1.ImportCanonicalQuoteSnapshotRequest",
            ".ficant.research.v1.ImportCanonicalQuoteSnapshotResponse",
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

fn assert_r6a_snapshot_contracts(messages: &BTreeMap<String, &DescriptorProto>) {
    assert_fields(
        messages,
        "ficant.research.v1.DataSnapshot",
        &[
            ExpectedField::message("data_snapshot_id", ".ficant.core.v1.Ulid"),
            ExpectedField::message("owner", ".ficant.core.v1.OwnerRef"),
            ExpectedField::message("visible_at", ".ficant.core.v1.MarketTime"),
            ExpectedField::message("as_of", ".ficant.core.v1.MarketTime"),
            ExpectedField::message("schema_hash", ".ficant.core.v1.Sha256"),
            ExpectedField::message("manifest_hash", ".ficant.core.v1.Sha256"),
            ExpectedField::message("blob_content_hash", ".ficant.core.v1.Sha256"),
            ExpectedField::repeated_message("lineage", ".ficant.core.v1.LineageRef"),
            ExpectedField::message("authorization_ref", ".ficant.core.v1.VersionRef"),
            ExpectedField::message("actor_id", ".ficant.core.v1.Ulid"),
        ],
    );
    assert_fields(
        messages,
        "ficant.research.v1.UniverseSnapshot",
        &[
            ExpectedField::message("universe_snapshot_id", ".ficant.core.v1.Ulid"),
            ExpectedField::message("owner", ".ficant.core.v1.OwnerRef"),
            ExpectedField::repeated_message("instrument_versions", ".ficant.core.v1.VersionRef"),
            ExpectedField::message("filter_digest", ".ficant.core.v1.Sha256"),
            ExpectedField::message("content_hash", ".ficant.core.v1.Sha256"),
            ExpectedField::repeated_message("lineage", ".ficant.core.v1.LineageRef"),
            ExpectedField::message("actor_id", ".ficant.core.v1.Ulid"),
        ],
    );
    assert_fields(
        messages,
        "ficant.market.v1.InstrumentMappingEntry",
        &[
            ExpectedField::scalar("source_instrument_key", Type::String),
            ExpectedField::message("effective_from", ".ficant.core.v1.MarketTime"),
            ExpectedField::message("effective_to", ".ficant.core.v1.MarketTime"),
            ExpectedField::message("instrument", ".ficant.core.v1.VersionRef"),
        ],
    );
    assert_fields(
        messages,
        "ficant.market.v1.InstrumentMapping",
        &[
            ExpectedField::message("mapping_id", ".ficant.core.v1.Ulid"),
            ExpectedField::message("owner", ".ficant.core.v1.OwnerRef"),
            ExpectedField::message("source", ".ficant.core.v1.VersionRef"),
            ExpectedField::repeated_message("entries", ".ficant.market.v1.InstrumentMappingEntry"),
            ExpectedField::message("content_hash", ".ficant.core.v1.Sha256"),
        ],
    );
    assert_fields(
        messages,
        "ficant.research.v1.ImportCanonicalQuoteSnapshotRequest",
        &[
            ExpectedField::scalar("idempotency_key", Type::String),
            ExpectedField::message("target_snapshot_id", ".ficant.core.v1.Ulid"),
            ExpectedField::message("authorization_ref", ".ficant.core.v1.VersionRef"),
            ExpectedField::message("mapping", ".ficant.market.v1.InstrumentMapping"),
            ExpectedField::message("calendar", ".ficant.market.v1.Calendar"),
            ExpectedField::message("unit", ".ficant.market.v1.Unit"),
            ExpectedField::message("as_of", ".ficant.core.v1.MarketTime"),
            ExpectedField::message("visible_at", ".ficant.core.v1.MarketTime"),
            ExpectedField::scalar("import_reason", Type::String),
        ],
    );
    assert_fields(
        messages,
        "ficant.research.v1.ImportCanonicalQuoteSnapshotResponse",
        &[
            ExpectedField::oneof_message(
                "data_snapshot",
                ".ficant.research.v1.DataSnapshot",
                "result",
            ),
            ExpectedField::oneof_message("error", ".ficant.core.v1.ErrorDetail", "result"),
        ],
    );
    let publish_universe_request = messages
        .get("ficant.research.v1.PublishUniverseSnapshotRequest")
        .expect("PublishUniverseSnapshotRequest exists");
    assert_reserved_tag(
        messages,
        "ficant.research.v1.PublishUniverseSnapshotRequest",
        2,
    );
    assert_reserved_names(
        messages,
        "ficant.research.v1.PublishUniverseSnapshotRequest",
        &["universe_snapshot"],
    );
    assert_exact_tagged_fields(
        publish_universe_request,
        &[
            ("idempotency_key", 1, Type::String, None, false),
            (
                "universe_snapshot_id",
                3,
                Type::Message,
                Some(".ficant.core.v1.Ulid"),
                false,
            ),
            (
                "owner",
                4,
                Type::Message,
                Some(".ficant.core.v1.OwnerRef"),
                false,
            ),
            (
                "instrument_versions",
                5,
                Type::Message,
                Some(".ficant.core.v1.VersionRef"),
                true,
            ),
            (
                "filter_digest",
                6,
                Type::Message,
                Some(".ficant.core.v1.Sha256"),
                false,
            ),
            (
                "lineage",
                7,
                Type::Message,
                Some(".ficant.core.v1.LineageRef"),
                true,
            ),
            (
                "change",
                8,
                Type::Message,
                Some(".ficant.core.v1.ChangeJustification"),
                false,
            ),
        ],
    );
    assert_fields(
        messages,
        "ficant.research.v1.PublishUniverseSnapshotResponse",
        &[
            ExpectedField::oneof_message(
                "universe_snapshot",
                ".ficant.research.v1.UniverseSnapshot",
                "result",
            ),
            ExpectedField::oneof_message("error", ".ficant.core.v1.ErrorDetail", "result"),
        ],
    );
    assert_fields(
        messages,
        "ficant.research.v1.GetSnapshotRequest",
        &[ExpectedField::message(
            "snapshot_id",
            ".ficant.core.v1.Ulid",
        )],
    );
    assert_fields(
        messages,
        "ficant.research.v1.GetSnapshotResponse",
        &[
            ExpectedField::oneof_message(
                "data_snapshot",
                ".ficant.research.v1.DataSnapshot",
                "result",
            ),
            ExpectedField::oneof_message(
                "universe_snapshot",
                ".ficant.research.v1.UniverseSnapshot",
                "result",
            ),
            ExpectedField::oneof_message("error", ".ficant.core.v1.ErrorDetail", "result"),
        ],
    );
}

fn position_snapshot_methods() -> Vec<ExpectedMethod> {
    vec![
        ExpectedMethod::new(
            "PublishPositionSnapshot",
            ".ficant.research.v1.PublishPositionSnapshotRequest",
            ".ficant.research.v1.PublishPositionSnapshotResponse",
        ),
        ExpectedMethod::new(
            "GetPositionSnapshot",
            ".ficant.research.v1.GetPositionSnapshotRequest",
            ".ficant.research.v1.GetPositionSnapshotResponse",
        ),
        ExpectedMethod::new(
            "ResolvePositionSnapshot",
            ".ficant.research.v1.ResolvePositionSnapshotRequest",
            ".ficant.research.v1.ResolvePositionSnapshotResponse",
        ),
        ExpectedMethod::new(
            "GetPositionViews",
            ".ficant.research.v1.GetPositionViewsRequest",
            ".ficant.research.v1.GetPositionViewsResponse",
        ),
        ExpectedMethod::new(
            "CalculateCapitalUse",
            ".ficant.research.v1.CalculateCapitalUseRequest",
            ".ficant.research.v1.CalculateCapitalUseResponse",
        ),
    ]
}

fn assert_position_snapshot_contract(messages: &BTreeMap<String, &DescriptorProto>) {
    assert_fields(
        messages,
        "ficant.research.v1.AccountingClassification",
        &[
            ExpectedField::enumeration(
                "state",
                ".ficant.research.v1.AccountingClassificationState",
            ),
            ExpectedField::enumeration("book", ".ficant.research.v1.AccountingBook"),
        ],
    );
    assert_fields(
        messages,
        "ficant.research.v1.Position",
        &[
            ExpectedField::message("position_id", ".ficant.core.v1.Ulid"),
            ExpectedField::message("instrument_ref", ".ficant.core.v1.VersionRef"),
            ExpectedField::message("quantity", ".ficant.core.v1.DecimalValue"),
            ExpectedField::message("economic_value", ".ficant.core.v1.DecimalValue"),
            ExpectedField::message("economic_pnl", ".ficant.core.v1.DecimalValue"),
            ExpectedField::message("accounting_pnl", ".ficant.core.v1.DecimalValue"),
            ExpectedField::message("capital_requirement", ".ficant.core.v1.DecimalValue"),
            ExpectedField::message(
                "accounting_classification",
                ".ficant.research.v1.AccountingClassification",
            ),
            ExpectedField::enumeration("holding_form", ".ficant.research.v1.PositionHoldingForm"),
        ],
    );
    assert_fields(
        messages,
        "ficant.research.v1.PositionSnapshot",
        &[
            ExpectedField::message("snapshot_id", ".ficant.core.v1.Ulid"),
            ExpectedField::message("owner", ".ficant.core.v1.OwnerRef"),
            ExpectedField::message("subject_ref", ".ficant.core.v1.VersionRef"),
            ExpectedField::message("observed_at", ".ficant.core.v1.MarketTime"),
            ExpectedField::message("visible_at", ".ficant.core.v1.MarketTime"),
            ExpectedField::message("content_hash", ".ficant.core.v1.Sha256"),
            ExpectedField::repeated_message("lineage", ".ficant.core.v1.LineageRef"),
            ExpectedField::repeated_message("positions", ".ficant.research.v1.Position"),
        ],
    );
    let responses = [
        ("PublishPositionSnapshotResponse", "snapshot"),
        ("GetPositionSnapshotResponse", "snapshot"),
        ("ResolvePositionSnapshotResponse", "snapshot"),
    ];
    for (name, field) in responses {
        assert_fields(
            messages,
            &format!("ficant.research.v1.{name}"),
            &[
                ExpectedField::oneof_message(
                    field,
                    ".ficant.research.v1.PositionSnapshot",
                    "result",
                ),
                ExpectedField::oneof_message("error", ".ficant.core.v1.ErrorDetail", "result"),
            ],
        );
    }
    assert_fields(
        messages,
        "ficant.research.v1.PublishPositionSnapshotRequest",
        &[
            ExpectedField::scalar("idempotency_key", Type::String),
            ExpectedField::message("snapshot", ".ficant.research.v1.PositionSnapshot"),
            ExpectedField::message("change", ".ficant.core.v1.ChangeJustification"),
        ],
    );
    assert_fields(
        messages,
        "ficant.research.v1.GetPositionSnapshotRequest",
        &[
            ExpectedField::message("snapshot_id", ".ficant.core.v1.Ulid"),
            ExpectedField::message("knowledge_at", ".ficant.core.v1.MarketTime"),
        ],
    );
    assert_fields(
        messages,
        "ficant.research.v1.ResolvePositionSnapshotRequest",
        &[
            ExpectedField::message("subject_ref", ".ficant.core.v1.VersionRef"),
            ExpectedField::message("observed_at", ".ficant.core.v1.MarketTime"),
            ExpectedField::message("knowledge_at", ".ficant.core.v1.MarketTime"),
        ],
    );
    assert_fields(
        messages,
        "ficant.research.v1.PositionView",
        &[
            ExpectedField::message("position_id", ".ficant.core.v1.Ulid"),
            ExpectedField::message("economic_value", ".ficant.core.v1.DecimalValue"),
            ExpectedField::message("economic_pnl", ".ficant.core.v1.DecimalValue"),
            ExpectedField::message("accounting_pnl", ".ficant.core.v1.DecimalValue"),
            ExpectedField::scalar("included_in_position_exposure", Type::Bool),
            ExpectedField::scalar("included_in_available_liquidity", Type::Bool),
            ExpectedField::scalar("collateral_fact", Type::Bool),
        ],
    );
    assert_fields(
        messages,
        "ficant.research.v1.PositionViews",
        &[
            ExpectedField::message("snapshot_id", ".ficant.core.v1.Ulid"),
            ExpectedField::message("content_hash", ".ficant.core.v1.Sha256"),
            ExpectedField::repeated_message("lineage", ".ficant.core.v1.LineageRef"),
            ExpectedField::repeated_message("positions", ".ficant.research.v1.PositionView"),
            ExpectedField::message("coverage", ".ficant.research.v1.CoverageDeclaration"),
        ],
    );
    assert_fields(
        messages,
        "ficant.research.v1.CapitalUse",
        &[
            ExpectedField::message("snapshot_id", ".ficant.core.v1.Ulid"),
            ExpectedField::message("content_hash", ".ficant.core.v1.Sha256"),
            ExpectedField::repeated_message("lineage", ".ficant.core.v1.LineageRef"),
            ExpectedField::message("total_capital_requirement", ".ficant.core.v1.DecimalValue"),
            ExpectedField::message("coverage", ".ficant.research.v1.CoverageDeclaration"),
        ],
    );
    for (request, response, success, success_type) in [
        (
            "GetPositionViewsRequest",
            "GetPositionViewsResponse",
            "views",
            ".ficant.research.v1.PositionViews",
        ),
        (
            "CalculateCapitalUseRequest",
            "CalculateCapitalUseResponse",
            "capital_use",
            ".ficant.research.v1.CapitalUse",
        ),
    ] {
        assert_fields(
            messages,
            &format!("ficant.research.v1.{request}"),
            &[
                ExpectedField::message("snapshot_id", ".ficant.core.v1.Ulid"),
                ExpectedField::message("knowledge_at", ".ficant.core.v1.MarketTime"),
            ],
        );
        assert_fields(
            messages,
            &format!("ficant.research.v1.{response}"),
            &[
                ExpectedField::oneof_message(success, success_type, "result"),
                ExpectedField::oneof_message("error", ".ficant.core.v1.ErrorDetail", "result"),
            ],
        );
    }
}

fn assert_position_snapshot_enums(enums: &BTreeMap<String, &EnumDescriptorProto>) {
    assert_enum(
        enums,
        "ficant.research.v1.AccountingClassificationState",
        &[
            ("ACCOUNTING_CLASSIFICATION_STATE_UNSPECIFIED", 0),
            ("ACCOUNTING_CLASSIFICATION_STATE_CLASSIFIED", 1),
            ("ACCOUNTING_CLASSIFICATION_STATE_NOT_APPLICABLE", 2),
            ("ACCOUNTING_CLASSIFICATION_STATE_UNKNOWN", 3),
        ],
    );
    assert_enum(
        enums,
        "ficant.research.v1.AccountingBook",
        &[
            ("ACCOUNTING_BOOK_UNSPECIFIED", 0),
            ("ACCOUNTING_BOOK_AC", 1),
            ("ACCOUNTING_BOOK_FVOCI", 2),
            ("ACCOUNTING_BOOK_FVTPL", 3),
        ],
    );
    assert_enum(
        enums,
        "ficant.research.v1.PositionHoldingForm",
        &[
            ("POSITION_HOLDING_FORM_UNSPECIFIED", 0),
            ("POSITION_HOLDING_FORM_OWNED", 1),
            ("POSITION_HOLDING_FORM_REPO_SOLD", 2),
            ("POSITION_HOLDING_FORM_REVERSE_REPO_COLLATERAL", 3),
        ],
    );
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
        "ficant.core.v1.FoundationChangeService".to_owned(),
        "ficant.market.v1.MarketDefinitionService".to_owned(),
        "ficant.market.v1.MarketFactService".to_owned(),
        "ficant.market.v1.DataSourceRegistryService".to_owned(),
        "ficant.research.v1.SnapshotService".to_owned(),
        "ficant.research.v1.PositionSnapshotService".to_owned(),
        "ficant.research.v1.FactorRegistryService".to_owned(),
        "ficant.research.v1.PortfolioRiskService".to_owned(),
        "ficant.research.v1.ExperimentService".to_owned(),
        "ficant.research.v1.ArtifactService".to_owned(),
        "ficant.research.v1.DataHealthService".to_owned(),
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
