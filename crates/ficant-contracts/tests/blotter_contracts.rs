//! L0 contract verification for `ficant.blotter.v1` and BP-1 wire compatibility.
//!
//! Reads the frozen Buf descriptor image `ficant_descriptor.bin` (verified against
//! live `buf build interface --exclude-source-info`) and asserts every service, RPC,
//! streaming flag, enum, message, field tag, type, presence, explicit oneof (`F-31`),
//! and reserved field range (`ImportOpeningBaseRequest` 4, 5).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Command;

use ficant_contracts::ficant::blotter::v1 as blotter;
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::market::v1 as market;
use ficant_contracts::ficant::rates::v1 as rates;
use prost::Message;
use prost_types::field_descriptor_proto::{Label, Type};
use prost_types::{DescriptorProto, EnumDescriptorProto, FileDescriptorProto, FileDescriptorSet};
use tonic::server::NamedService;

const MODEL_PROTO: &str = "ficant/blotter/v1/model.proto";
const SPI_PROTO: &str = "ficant/blotter/v1/spi.proto";
const SERVICE_PROTO: &str = "ficant/blotter/v1/service.proto";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir")
        .parent()
        .expect("repo root")
        .to_path_buf()
}

fn descriptor_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("ficant_descriptor.bin")
}

fn load_descriptor_set() -> FileDescriptorSet {
    let bytes = std::fs::read(descriptor_path()).expect("read ficant_descriptor.bin");
    FileDescriptorSet::decode(bytes.as_slice()).expect("decode FileDescriptorSet")
}

fn blotter_files(fds: &FileDescriptorSet) -> BTreeMap<&str, &FileDescriptorProto> {
    fds.file
        .iter()
        .filter(|f| f.package.as_deref() == Some("ficant.blotter.v1"))
        .map(|f| (f.name.as_deref().expect("file name"), f))
        .collect()
}

#[derive(Clone, Copy)]
enum FieldSpecType {
    String,
    Uint64,
    Uint32,
    Bool,
    Message(&'static str),
    Enum(&'static str),
}

#[derive(Clone, Copy)]
struct FieldSpec {
    name: &'static str,
    tag: i32,
    ty: FieldSpecType,
    repeated: bool,
    proto3_optional: bool,
    oneof: Option<&'static str>,
}

const fn f_str(name: &'static str, tag: i32) -> FieldSpec {
    FieldSpec {
        name,
        tag,
        ty: FieldSpecType::String,
        repeated: false,
        proto3_optional: false,
        oneof: None,
    }
}

const fn f_opt_str(name: &'static str, tag: i32) -> FieldSpec {
    FieldSpec {
        name,
        tag,
        ty: FieldSpecType::String,
        repeated: false,
        proto3_optional: true,
        oneof: None,
    }
}

const fn f_rep_str(name: &'static str, tag: i32) -> FieldSpec {
    FieldSpec {
        name,
        tag,
        ty: FieldSpecType::String,
        repeated: true,
        proto3_optional: false,
        oneof: None,
    }
}

const fn f_u64(name: &'static str, tag: i32) -> FieldSpec {
    FieldSpec {
        name,
        tag,
        ty: FieldSpecType::Uint64,
        repeated: false,
        proto3_optional: false,
        oneof: None,
    }
}

const fn f_opt_u64(name: &'static str, tag: i32) -> FieldSpec {
    FieldSpec {
        name,
        tag,
        ty: FieldSpecType::Uint64,
        repeated: false,
        proto3_optional: true,
        oneof: None,
    }
}

const fn f_u32(name: &'static str, tag: i32) -> FieldSpec {
    FieldSpec {
        name,
        tag,
        ty: FieldSpecType::Uint32,
        repeated: false,
        proto3_optional: false,
        oneof: None,
    }
}

const fn f_bool(name: &'static str, tag: i32) -> FieldSpec {
    FieldSpec {
        name,
        tag,
        ty: FieldSpecType::Bool,
        repeated: false,
        proto3_optional: false,
        oneof: None,
    }
}

const fn f_msg(name: &'static str, tag: i32, type_name: &'static str) -> FieldSpec {
    FieldSpec {
        name,
        tag,
        ty: FieldSpecType::Message(type_name),
        repeated: false,
        proto3_optional: false,
        oneof: None,
    }
}

const fn f_rep_msg(name: &'static str, tag: i32, type_name: &'static str) -> FieldSpec {
    FieldSpec {
        name,
        tag,
        ty: FieldSpecType::Message(type_name),
        repeated: true,
        proto3_optional: false,
        oneof: None,
    }
}

const fn f_oneof_msg(
    name: &'static str,
    tag: i32,
    type_name: &'static str,
    oneof: &'static str,
) -> FieldSpec {
    FieldSpec {
        name,
        tag,
        ty: FieldSpecType::Message(type_name),
        repeated: false,
        proto3_optional: false,
        oneof: Some(oneof),
    }
}

const fn f_enum(name: &'static str, tag: i32, type_name: &'static str) -> FieldSpec {
    FieldSpec {
        name,
        tag,
        ty: FieldSpecType::Enum(type_name),
        repeated: false,
        proto3_optional: false,
        oneof: None,
    }
}

const D: &str = ".ficant.core.v1.DecimalValue";
const U: &str = ".ficant.core.v1.Ulid";
const H: &str = ".ficant.core.v1.Sha256";
const Z: &str = ".ficant.core.v1.MarketTime";
const E: &str = ".ficant.core.v1.ErrorDetail";
const BOND: &str = ".ficant.market.v1.Bond";
const CALENDAR: &str = ".ficant.market.v1.Calendar";
const RULE_PACK: &str = ".ficant.market.v1.MarketRulePack";
const PRICE_SRC: &str = ".ficant.market.v1.PriceSourceType";
const CAL_REQ: &str = ".ficant.rates.v1.CalendarRequirement";
const UNITS: &str = ".ficant.rates.v1.AnalysisUnits";
const TAX_BASIS: &str = ".ficant.rates.v1.BondTaxBasis";
const HEDGE_MEAS: &str = ".ficant.rates.v1.FuturesHedgeMeasures";

const E_BUCKET: &str = ".ficant.blotter.v1.SettlementBucket";
const E_SIDE: &str = ".ficant.blotter.v1.TradeSide";
const E_EARMARK_STATE: &str = ".ficant.blotter.v1.EarmarkState";
const E_PENDING_STATE: &str = ".ficant.blotter.v1.PendingMatchState";
const E_PENDING_REASON: &str = ".ficant.blotter.v1.PendingMatchReason";
const E_RESOLVE_ACTION: &str = ".ficant.blotter.v1.PendingMatchResolutionAction";
const E_FEED_ACTION: &str = ".ficant.blotter.v1.TradeFeedRecordAction";
const E_ELIGIBILITY: &str = ".ficant.blotter.v1.RateCoverageEligibility";
const E_COVERAGE: &str = ".ficant.blotter.v1.CoverageStatus";
const E_UNCOVERED: &str = ".ficant.blotter.v1.UncoveredReason";
const E_CAL_RES: &str = ".ficant.blotter.v1.CalendarResolutionMode";
const E_FUNDING_PREC: &str = ".ficant.blotter.v1.FundingPrecision";
const E_HEDGE_SCOPE: &str = ".ficant.blotter.v1.PreviewHedgeScope";
const E_DRAFT_TARGET: &str = ".ficant.blotter.v1.DraftTargetState";
const E_DRAFT_STATUS: &str = ".ficant.blotter.v1.DraftEarmarkStatus";
const E_FEED_HEALTH: &str = ".ficant.blotter.v1.FeedHealthState";
const E_OUTCOME_KIND: &str = ".ficant.blotter.v1.TradeFillOutcomeKind";

const M_QUOTE_TERMS: &str = ".ficant.blotter.v1.QuoteTerms";
const M_CATALOG_ENTRY: &str = ".ficant.blotter.v1.InstrumentCatalogEntry";
const M_OPENING_BAL: &str = ".ficant.blotter.v1.OpeningInstrumentBalance";
const M_BASE_EVIDENCE: &str = ".ficant.blotter.v1.BaseEvidence";
const M_OPENING_BASE: &str = ".ficant.blotter.v1.OpeningPositionBase";
const M_BASELINE_DELTA: &str = ".ficant.blotter.v1.BaselineDelta";
const M_BUCKET_VIEW: &str = ".ficant.blotter.v1.BucketBalanceView";
const M_POS_VIEW: &str = ".ficant.blotter.v1.InstrumentPositionView";
const M_EARMARK: &str = ".ficant.blotter.v1.EarmarkRecord";
const M_ALLOC_VIEW: &str = ".ficant.blotter.v1.ReconcileAllocationView";
const M_PENDING: &str = ".ficant.blotter.v1.PendingMatchRecord";
const M_ANCHOR: &str = ".ficant.blotter.v1.SourceTextAnchor";
const M_RULE_LINEAGE: &str = ".ficant.blotter.v1.RuleLineage";
const M_DRAFT: &str = ".ficant.blotter.v1.DraftEarmarkRecord";
const M_RISK_SUM: &str = ".ficant.blotter.v1.RiskAndCoverageSummary";
const M_FUNDING_SUM: &str = ".ficant.blotter.v1.FundingGapSummary";
const M_SEQ_RANGE: &str = ".ficant.blotter.v1.SeqRange";
const M_FEED_STATUS: &str = ".ficant.blotter.v1.FeedStatusView";
const M_SNAPSHOT: &str = ".ficant.blotter.v1.AccountBlotterSnapshot";
const M_ACCT_CHANGE: &str = ".ficant.blotter.v1.AccountChange";
const M_ENVELOPE: &str = ".ficant.blotter.v1.BlotterEventEnvelope";
const M_FALLBACK: &str = ".ficant.blotter.v1.SnapshotFallbackRequired";
const M_STREAM_HB: &str = ".ficant.blotter.v1.StreamHeartbeat";
const M_POLL_BATCH: &str = ".ficant.blotter.v1.PollEventsBatch";
const M_FILL: &str = ".ficant.blotter.v1.TradeFillRecord";
const M_QUOTE_TICK: &str = ".ficant.blotter.v1.MarketQuoteTick";
const M_NLP_PROPOSAL: &str = ".ficant.blotter.v1.NlpEarmarkProposal";
const M_FILL_OUTCOME: &str = ".ficant.blotter.v1.TradeFillIngestOutcome";
const M_PULL_FEED_RES: &str = ".ficant.blotter.v1.PullTradeFeedBatchResult";
const M_QUOTE_CURSOR: &str = ".ficant.blotter.v1.QuoteCursor";
const M_PULL_QUOTES_RES: &str = ".ficant.blotter.v1.PullMarketQuotesResult";
const M_FEED_HB: &str = ".ficant.blotter.v1.FeedHeartbeat";
const M_QUOTE_HB: &str = ".ficant.blotter.v1.QuoteHeartbeat";

fn assert_message_spec(
    messages: &BTreeMap<&str, (&str, &DescriptorProto)>,
    seen: &mut BTreeSet<&'static str>,
    file: &'static str,
    name: &'static str,
    fields: &[FieldSpec],
    expected_reserved: &[i32],
) {
    assert!(seen.insert(name), "duplicate spec assertion for {name}");
    let (actual_file, desc) = messages
        .get(name)
        .unwrap_or_else(|| panic!("missing message {name}"));
    assert_eq!(*actual_file, file, "message {name} in wrong file");
    assert_eq!(
        desc.field.len(),
        fields.len(),
        "message {name}: expected {} fields, got {}",
        fields.len(),
        desc.field.len()
    );

    for (actual, expected) in desc.field.iter().zip(fields.iter()) {
        assert_eq!(
            actual.name.as_deref(),
            Some(expected.name),
            "{name} tag {}: field name mismatch",
            expected.tag
        );
        assert_eq!(
            actual.number,
            Some(expected.tag),
            "{name}.{}: tag number mismatch",
            expected.name
        );

        let expected_label = if expected.repeated {
            Label::Repeated
        } else {
            Label::Optional
        };
        assert_eq!(
            actual.label(),
            expected_label,
            "{name}.{}: label mismatch",
            expected.name
        );

        match expected.ty {
            FieldSpecType::String => {
                assert_eq!(actual.r#type(), Type::String, "{name}.{}", expected.name);
                assert!(actual.type_name.is_none());
            }
            FieldSpecType::Uint64 => {
                assert_eq!(actual.r#type(), Type::Uint64, "{name}.{}", expected.name);
                assert!(actual.type_name.is_none());
            }
            FieldSpecType::Uint32 => {
                assert_eq!(actual.r#type(), Type::Uint32, "{name}.{}", expected.name);
                assert!(actual.type_name.is_none());
            }
            FieldSpecType::Bool => {
                assert_eq!(actual.r#type(), Type::Bool, "{name}.{}", expected.name);
                assert!(actual.type_name.is_none());
            }
            FieldSpecType::Message(tname) => {
                assert_eq!(actual.r#type(), Type::Message, "{name}.{}", expected.name);
                assert_eq!(
                    actual.type_name.as_deref(),
                    Some(tname),
                    "{name}.{}",
                    expected.name
                );
            }
            FieldSpecType::Enum(tname) => {
                assert_eq!(actual.r#type(), Type::Enum, "{name}.{}", expected.name);
                assert_eq!(
                    actual.type_name.as_deref(),
                    Some(tname),
                    "{name}.{}",
                    expected.name
                );
            }
        }

        let actual_proto3_opt = actual.proto3_optional.unwrap_or(false);
        assert_eq!(
            actual_proto3_opt, expected.proto3_optional,
            "{name}.{}: proto3_optional mismatch",
            expected.name
        );

        match expected.oneof {
            Some(expected_oneof) => {
                assert!(!actual_proto3_opt, "{name}.{} is real oneof", expected.name);
                let idx = actual
                    .oneof_index
                    .unwrap_or_else(|| panic!("{name}.{} missing oneof_index", expected.name));
                let oneof_name = desc.oneof_decl[usize::try_from(idx).unwrap()]
                    .name
                    .as_deref();
                assert_eq!(
                    oneof_name,
                    Some(expected_oneof),
                    "{name}.{}: oneof name mismatch",
                    expected.name
                );
            }
            None => {
                if !actual_proto3_opt {
                    assert!(
                        actual.oneof_index.is_none(),
                        "{name}.{}: unexpected oneof_index",
                        expected.name
                    );
                }
            }
        }
    }

    let mut actual_reserved = BTreeSet::new();
    for range in &desc.reserved_range {
        let start = range.start.unwrap_or(0);
        let end = range.end.unwrap_or(0);
        for num in start..end {
            actual_reserved.insert(num);
        }
    }
    let expected_reserved_set: BTreeSet<i32> = expected_reserved.iter().copied().collect();
    assert_eq!(
        actual_reserved, expected_reserved_set,
        "{name}: reserved field numbers mismatch"
    );
}

fn run_buf_build_to_file(out_path: &std::path::Path) -> std::process::Output {
    let args = [
        "build",
        "interface",
        "--exclude-source-info",
        "--output",
        out_path.to_str().expect("utf-8 output path"),
    ];
    let try_run = |program: &str| {
        Command::new(program)
            .current_dir(repo_root())
            .args(args)
            .output()
    };
    match try_run("buf") {
        Ok(out) => out,
        Err(err) if cfg!(windows) && err.kind() == std::io::ErrorKind::NotFound => {
            try_run("buf.cmd").expect("run buf.cmd build interface")
        }
        Err(err) => panic!("run buf build interface: {err}"),
    }
}

#[test]
fn frozen_descriptor_matches_live_buf_build_byte_for_byte() {
    let frozen = std::fs::read(descriptor_path()).expect("read frozen ficant_descriptor.bin");
    let temp_path = std::env::temp_dir().join(format!(
        "ficant_blotter_descriptor_live_{}.bin",
        std::process::id()
    ));
    let output = run_buf_build_to_file(&temp_path);
    assert!(
        output.status.success(),
        "buf build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let live = std::fs::read(&temp_path).expect("read live descriptor file");
    let _ = std::fs::remove_file(&temp_path);
    assert_eq!(
        live, frozen,
        "ficant_descriptor.bin drifted from live `buf build interface --exclude-source-info`"
    );
}

#[test]
fn blotter_services_and_25_rpcs_match_frozen_spec() {
    let fds = load_descriptor_set();
    let files = blotter_files(&fds);
    assert_eq!(files.len(), 3);
    assert!(files.contains_key(MODEL_PROTO));
    assert!(files.contains_key(SPI_PROTO));
    assert!(files.contains_key(SERVICE_PROTO));
    for file in files.values() {
        assert_eq!(file.syntax.as_deref(), Some("proto3"));
    }

    let mut actual_services: BTreeMap<&str, (&str, Vec<(&str, &str, &str, bool, bool)>)> =
        BTreeMap::new();
    for (&fname, file) in &files {
        for svc in &file.service {
            let methods = svc
                .method
                .iter()
                .map(|m| {
                    (
                        m.name.as_deref().unwrap(),
                        m.input_type.as_deref().unwrap(),
                        m.output_type.as_deref().unwrap(),
                        m.client_streaming.unwrap_or(false),
                        m.server_streaming.unwrap_or(false),
                    )
                })
                .collect();
            actual_services.insert(svc.name.as_deref().unwrap(), (fname, methods));
        }
    }

    assert_eq!(actual_services.len(), 5, "expected 5 blotter services");
    let total_rpcs: usize = actual_services.values().map(|(_, m)| m.len()).sum();
    assert_eq!(total_rpcs, 25, "expected 25 blotter RPCs");

    let expected: &[(&str, &str, &[(&str, bool)])] = &[
        (
            "DeskBlotterService",
            SERVICE_PROTO,
            &[
                ("ImportOpeningBase", false),
                ("IngestMarketQuotes", false),
                ("CreateHold", false),
                ("ExtendHold", false),
                ("ConfirmHold", false),
                ("ReleaseHold", false),
                ("CreateCommitted", false),
                ("CancelCommitted", false),
                ("SubmitDraftEarmark", false),
                ("ConfirmDraftEarmark", false),
                ("IngestTradeFeed", false),
                ("ResolvePendingMatch", false),
                ("SweepLifecycle", false),
                ("SetFeedControl", false),
                ("PreviewCoveredHedge", false),
                ("GetAccountSnapshot", false),
                ("PollEvents", false),
                ("SubscribeEvents", true),
            ],
        ),
        (
            "PositionBaseProviderService",
            SPI_PROTO,
            &[("FetchOpeningPositionBase", false)],
        ),
        (
            "TradeFeedReconcilerService",
            SPI_PROTO,
            &[
                ("PullTradeFeedBatch", false),
                ("StreamTradeFeed", true),
                ("FetchTradeRevision", false),
            ],
        ),
        (
            "MarketQuoteStreamService",
            SPI_PROTO,
            &[("PullMarketQuotes", false), ("StreamMarketQuotes", true)],
        ),
        (
            "IntranetNlpIntentParserService",
            SPI_PROTO,
            &[("ParseNlpIntent", false)],
        ),
    ];

    for (svc_name, expected_file, expected_rpcs) in expected {
        let (actual_file, actual_rpcs) = actual_services
            .get(svc_name)
            .unwrap_or_else(|| panic!("missing service {svc_name}"));
        assert_eq!(
            actual_file, expected_file,
            "service {svc_name} file mismatch"
        );
        assert_eq!(
            actual_rpcs.len(),
            expected_rpcs.len(),
            "{svc_name} RPC count"
        );

        for (actual, (rpc_name, server_streaming)) in actual_rpcs.iter().zip(expected_rpcs.iter()) {
            let exp_in = format!(".ficant.blotter.v1.{rpc_name}Request");
            let exp_out = format!(".ficant.blotter.v1.{rpc_name}Response");
            assert_eq!(actual.0, *rpc_name);
            assert_eq!(actual.1, exp_in);
            assert_eq!(actual.2, exp_out);
            assert!(!actual.3, "{svc_name}.{rpc_name} must not client-stream");
            assert_eq!(
                actual.4, *server_streaming,
                "{svc_name}.{rpc_name} server_streaming mismatch"
            );
        }
    }
}

#[test]
fn blotter_all_18_enums_match_frozen_spec() {
    let fds = load_descriptor_set();
    let files = blotter_files(&fds);

    let enums: BTreeMap<&str, (&str, &EnumDescriptorProto)> = files
        .iter()
        .flat_map(|(&fname, file)| {
            file.enum_type
                .iter()
                .map(move |e| (e.name.as_deref().unwrap(), (fname, e)))
        })
        .collect();

    assert_eq!(enums.len(), 18, "expected 18 blotter enums");

    let expected: &[(&str, &str, &str, &[&str])] = &[
        (
            MODEL_PROTO,
            "SettlementBucket",
            "SETTLEMENT_BUCKET",
            &["T_PLUS_0", "T_PLUS_1"],
        ),
        (MODEL_PROTO, "TradeSide", "TRADE_SIDE", &["BUY", "SELL"]),
        (
            MODEL_PROTO,
            "EarmarkState",
            "EARMARK_STATE",
            &[
                "HOLD",
                "COMMITTED",
                "EXPIRED",
                "RELEASED",
                "CANCELLED",
                "RECONCILED",
            ],
        ),
        (
            MODEL_PROTO,
            "PendingMatchState",
            "PENDING_MATCH_STATE",
            &[
                "OPEN",
                "RESOLVED_MATCHED",
                "RESOLVED_ORPHAN_ACCEPTED",
                "RELEASED",
                "REVERSED_BY_FEED",
                "RESOLVED_BASELINE_DELTA",
            ],
        ),
        (
            MODEL_PROTO,
            "PendingMatchReason",
            "PENDING_MATCH_REASON",
            &[
                "AMBIGUOUS_CANDIDATES",
                "NOTIONAL_MISMATCH_OR_SPLIT",
                "PRICE_OUT_OF_TOLERANCE",
                "ORPHAN_NO_COMMITMENT",
                "BURST_REPEAT",
                "EXISTING_PENDING_ON_KEY",
                "AMENDMENT_MISMATCH",
                "BASELINE_REVISION_MANUAL",
                "INCOMPARABLE_BASIS",
                "REVISION_HOLD",
                "GAP_HOLD",
            ],
        ),
        (
            MODEL_PROTO,
            "PendingMatchResolutionAction",
            "PENDING_MATCH_RESOLUTION_ACTION",
            &[
                "MATCH_FULL",
                "SPLIT_OFFSET",
                "ACCEPT_ORPHAN",
                "RELEASE_REJECT",
                "APPLY_BASELINE_DELTA",
            ],
        ),
        (
            MODEL_PROTO,
            "TradeFeedRecordAction",
            "TRADE_FEED_RECORD_ACTION",
            &["NEW_FILL", "AMEND", "CANCEL_REVERSAL"],
        ),
        (
            MODEL_PROTO,
            "RateCoverageEligibility",
            "RATE_COVERAGE_ELIGIBILITY",
            &[
                "SOVEREIGN_OR_POLICY_FIXED",
                "CREDIT_OR_SPREAD_PRODUCT",
                "FLOATING_OR_INDEXED",
                "EMBEDDED_OPTION_OR_SINKING",
            ],
        ),
        (
            MODEL_PROTO,
            "CoverageStatus",
            "COVERAGE_STATUS",
            &["COVERED_PRE_TAX", "UNCOVERED", "INACTIVE_ZERO_POSITION"],
        ),
        (
            MODEL_PROTO,
            "UncoveredReason",
            "UNCOVERED_REASON",
            &[
                "INSTRUMENT_SPEC_MISSING",
                "UNSUPPORTED_COUPON_STRUCTURE",
                "EMBEDDED_OPTION_OR_CALLABLE",
                "UNSUPPORTED_CONVENTION",
                "MISSING_MARKET_QUOTE",
                "CALENDAR_COVERAGE_MISSING",
                "KERNEL_PRICING_FAILED",
                "ELIGIBILITY_UNVERIFIED",
                "CREDIT_OR_NON_RATES_ISSUER",
                "STALE_QUOTE",
            ],
        ),
        (
            MODEL_PROTO,
            "CalendarResolutionMode",
            "CALENDAR_RESOLUTION_MODE",
            &["EXACT", "PROVISIONAL_WEEKEND_BEYOND_COVERAGE"],
        ),
        (
            MODEL_PROTO,
            "FundingPrecision",
            "FUNDING_PRECISION",
            &[
                "EXACT_SETTLEMENT_AMOUNT",
                "EXACT_DIRTY_PRICE",
                "CLEAN_PRICE_ESTIMATE",
                "UNPRICED_YIELD_ONLY",
            ],
        ),
        (
            MODEL_PROTO,
            "PreviewHedgeScope",
            "PREVIEW_HEDGE_SCOPE",
            &["WHOLE_ACCOUNT", "COVERED_SUBSET_ONLY"],
        ),
        (
            MODEL_PROTO,
            "DraftTargetState",
            "DRAFT_TARGET_STATE",
            &["HOLD", "COMMITTED"],
        ),
        (
            MODEL_PROTO,
            "DraftEarmarkStatus",
            "DRAFT_EARMARK_STATUS",
            &["PENDING_CONFIRMATION", "CONFIRMED", "DISCARDED"],
        ),
        (
            MODEL_PROTO,
            "FeedHealthState",
            "FEED_HEALTH_STATE",
            &["HEALTHY", "STALE", "GAP_DETECTED", "PAUSED_BY_OPERATOR"],
        ),
        (
            MODEL_PROTO,
            "PrincipalRole",
            "PRINCIPAL_ROLE",
            &["TRADER", "NLP_PARSER", "FEED_ADAPTER", "OPERATOR"],
        ),
        (
            SPI_PROTO,
            "TradeFillOutcomeKind",
            "TRADE_FILL_OUTCOME_KIND",
            &[
                "PER_ITEM_DEDUPLICATED",
                "PRE_WATERMARK_IGNORED",
                "AUTO_MATCHED",
                "PENDING_MATCH",
                "BUFFERED_GAP",
                "BUFFERED_REVISION",
                "REVERSED",
                "BASELINE_REVIEW",
                "SUPERSEDED_IN_BATCH",
            ],
        ),
    ];

    assert_eq!(expected.len(), 18);
    for (exp_file, enum_name, prefix, values) in expected {
        let (actual_file, desc) = enums
            .get(enum_name)
            .unwrap_or_else(|| panic!("missing enum {enum_name}"));
        assert_eq!(actual_file, exp_file, "enum {enum_name} file mismatch");

        let mut expected_pairs = vec![(format!("{prefix}_UNSPECIFIED"), 0_i32)];
        for (idx, suffix) in values.iter().enumerate() {
            expected_pairs.push((
                format!("{prefix}_{suffix}"),
                i32::try_from(idx + 1).unwrap(),
            ));
        }

        let actual_pairs: Vec<(String, i32)> = desc
            .value
            .iter()
            .map(|v| (v.name.clone().unwrap(), v.number.unwrap()))
            .collect();
        assert_eq!(actual_pairs, expected_pairs, "enum {enum_name} mismatch");
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn blotter_all_100_messages_match_frozen_spec() {
    let fds = load_descriptor_set();
    let files = blotter_files(&fds);

    let messages: BTreeMap<&str, (&str, &DescriptorProto)> = files
        .iter()
        .flat_map(|(&fname, file)| {
            file.message_type
                .iter()
                .map(move |m| (m.name.as_deref().unwrap(), (fname, m)))
        })
        .collect();

    assert_eq!(messages.len(), 100, "expected 100 blotter messages");
    let mut seen = BTreeSet::new();

    // --- model.proto (24 messages) ---
    assert_message_spec(
        &messages,
        &mut seen,
        MODEL_PROTO,
        "QuoteTerms",
        &[
            f_oneof_msg("clean_price_per_100", 1, D, "price_or_yield"),
            f_oneof_msg("yield_to_maturity", 2, D, "price_or_yield"),
            f_msg("settlement_amount", 3, D),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        MODEL_PROTO,
        "InstrumentCatalogEntry",
        &[
            f_str("instrument_code", 1),
            f_msg("bond", 2, BOND),
            f_enum("eligibility_class", 3, E_ELIGIBILITY),
            f_enum("calendar_requirement", 4, CAL_REQ),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        MODEL_PROTO,
        "OpeningInstrumentBalance",
        &[
            f_str("instrument_code", 1),
            f_msg("opening_economic_notional", 2, D),
            f_msg("opening_available_notional", 3, D),
            f_msg("opening_net_t0_notional", 4, D),
            f_msg("opening_net_t1_notional", 5, D),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        MODEL_PROTO,
        "BaseEvidence",
        &[
            f_str("source_id", 1),
            f_str("external_trade_id", 2),
            f_u64("original_feed_seq", 3),
            f_msg("original_executed_at", 4, Z),
            f_msg("old_notional", 5, D),
            f_msg("old_signed_cash", 6, D),
            f_enum("settlement_bucket", 7, E_BUCKET),
            f_str("evidence_ref", 8),
            f_msg("evidence_hash", 9, H),
            f_str("instrument_code", 10),
            f_enum("side", 11, E_SIDE),
            f_u64("original_revision", 12),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        MODEL_PROTO,
        "OpeningPositionBase",
        &[
            f_str("account_id", 1),
            f_msg("base_id", 2, U),
            f_str("trading_date", 3),
            f_str("next_settlement_date", 4),
            f_msg("effective_from", 5, Z),
            f_msg("watermark_time", 6, Z),
            f_opt_u64("watermark_source_seq", 7),
            f_str("source_id", 8),
            f_msg("opening_cash_t0", 9, D),
            f_msg("opening_net_cash_delta_t1", 10, D),
            f_rep_msg("balances", 11, M_OPENING_BAL),
            f_rep_msg("instruments", 12, M_CATALOG_ENTRY),
            f_msg("calendar", 13, CALENDAR),
            f_enum("default_calendar_requirement", 14, CAL_REQ),
            f_msg("units", 15, UNITS),
            f_rep_msg("baseline_evidence", 16, M_BASE_EVIDENCE),
            f_u64("source_start_after_feed_seq", 17),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        MODEL_PROTO,
        "BaselineDelta",
        &[
            f_msg("delta_notional", 1, D),
            f_msg("delta_cash", 2, D),
            f_enum("settlement_bucket", 3, E_BUCKET),
            f_msg("evidence", 4, M_BASE_EVIDENCE),
            f_str("reason", 5),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        MODEL_PROTO,
        "BucketBalanceView",
        &[
            f_enum("settlement_bucket", 1, E_BUCKET),
            f_str("settlement_date", 2),
            f_msg("available_notional", 3, D),
            f_msg("hold_buy_notional", 4, D),
            f_msg("hold_sell_notional", 5, D),
            f_msg("committed_buy_unreconciled_notional", 6, D),
            f_msg("committed_sell_unreconciled_notional", 7, D),
            f_msg("reconciled_buy_notional", 8, D),
            f_msg("reconciled_sell_notional", 9, D),
            f_msg("pending_match_buy_unresolved_notional", 10, D),
            f_msg("pending_match_sell_unresolved_notional", 11, D),
            f_bool("sell_ambiguity_locked", 12),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        MODEL_PROTO,
        "InstrumentPositionView",
        &[
            f_str("account_id", 1),
            f_str("instrument_code", 2),
            f_u64("position_version", 3),
            f_msg("economic_net_notional", 4, D),
            f_msg("available_t0_notional", 5, D),
            f_msg("available_t1_notional", 6, D),
            f_msg("bucket_t0", 7, M_BUCKET_VIEW),
            f_msg("bucket_t1", 8, M_BUCKET_VIEW),
            f_enum("coverage_status", 9, E_COVERAGE),
            f_enum("tax_basis", 10, TAX_BASIS),
            f_msg("pre_tax_dv01", 11, D),
            f_msg("valuation_clean_price_per_100", 12, D),
            f_msg("valuation_yield_to_maturity", 13, D),
            f_enum("calendar_resolution", 14, E_CAL_RES),
            f_enum("uncovered_reason", 15, E_UNCOVERED),
            f_str("uncovered_detail", 16),
            f_str("valuation_settlement_date", 17),
            f_str("active_quote_source_id", 18),
            f_opt_u64("active_quote_seq", 19),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        MODEL_PROTO,
        "EarmarkRecord",
        &[
            f_msg("earmark_id", 1, U),
            f_u64("earmark_revision", 2),
            f_str("account_id", 3),
            f_str("instrument_code", 4),
            f_enum("side", 5, E_SIDE),
            f_enum("settlement_bucket", 6, E_BUCKET),
            f_str("settlement_date", 7),
            f_msg("notional", 8, D),
            f_msg("unreconciled_notional", 9, D),
            f_msg("quote_terms", 10, M_QUOTE_TERMS),
            f_msg("locked_settlement_cash", 11, D),
            f_msg("unreconciled_settlement_cash", 12, D),
            f_enum("funding_precision", 13, E_FUNDING_PREC),
            f_enum("state", 14, E_EARMARK_STATE),
            f_str("created_by", 15),
            f_msg("created_at", 16, Z),
            f_msg("expires_at", 17, Z),
            f_msg("committed_at", 18, Z),
            f_bool("overdue_alert", 19),
            f_bool("pre_close_alert", 20),
            f_bool("reopened_by_reversal", 21),
            f_str("note", 22),
            f_u64("created_seq", 23),
            f_u64("updated_seq", 24),
            f_opt_str("confirmed_by_trader_id", 25),
            f_opt_str("cancel_actor_id", 26),
            f_opt_str("cancel_reason", 27),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        MODEL_PROTO,
        "ReconcileAllocationView",
        &[
            f_msg("allocation_id", 1, U),
            f_str("source_id", 2),
            f_str("external_trade_id", 3),
            f_u64("source_revision", 4),
            f_msg("earmark_id", 5, U),
            f_msg("pending_match_id", 6, U),
            f_msg("allocated_notional", 7, D),
            f_msg("allocated_fill_cash", 8, D),
            f_opt_u64("reversed_by_revision", 9),
            f_msg("released_earmark_cash", 10, D),
            f_msg("baseline_delta", 11, M_BASELINE_DELTA),
            f_str("actor_id", 12),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        MODEL_PROTO,
        "PendingMatchRecord",
        &[
            f_msg("pending_match_id", 1, U),
            f_u64("pending_revision", 2),
            f_str("account_id", 3),
            f_str("instrument_code", 4),
            f_enum("side", 5, E_SIDE),
            f_enum("settlement_bucket", 6, E_BUCKET),
            f_str("settlement_date", 7),
            f_str("source_id", 8),
            f_str("external_trade_id", 9),
            f_u64("source_revision", 10),
            f_msg("original_notional", 11, D),
            f_msg("unresolved_notional", 12, D),
            f_msg("prior_baseline_notional", 13, D),
            f_msg("quote_terms", 14, M_QUOTE_TERMS),
            f_msg("executed_at", 15, Z),
            f_enum("state", 16, E_PENDING_STATE),
            f_enum("reason", 17, E_PENDING_REASON),
            f_rep_msg("candidate_earmark_ids", 18, U),
            f_u64("created_seq", 19),
            f_u64("updated_seq", 20),
            f_msg("unresolved_signed_cash", 21, D),
            f_msg("evidence", 22, M_BASE_EVIDENCE),
            f_msg("applied_delta", 23, M_BASELINE_DELTA),
            f_opt_str("resolution_actor_id", 24),
            f_opt_str("resolution_reason", 25),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        MODEL_PROTO,
        "SourceTextAnchor",
        &[
            f_str("field_name", 1),
            f_u32("byte_start", 2),
            f_u32("byte_end", 3),
            f_str("span_text", 4),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        MODEL_PROTO,
        "RuleLineage",
        &[
            f_msg("pack_id", 1, U),
            f_u64("version", 2),
            f_str("source", 3),
            f_msg("content_hash", 4, H),
            f_msg("parsed_constraints_hash", 5, H),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        MODEL_PROTO,
        "DraftEarmarkRecord",
        &[
            f_msg("draft_id", 1, U),
            f_u64("draft_revision", 2),
            f_str("account_id", 3),
            f_str("instrument_code", 4),
            f_enum("side", 5, E_SIDE),
            f_enum("settlement_bucket", 6, E_BUCKET),
            f_msg("notional", 7, D),
            f_msg("quote_terms", 8, M_QUOTE_TERMS),
            f_enum("target_state", 9, E_DRAFT_TARGET),
            f_enum("status", 10, E_DRAFT_STATUS),
            f_str("raw_source_text", 11),
            f_rep_msg("anchors", 12, M_ANCHOR),
            f_msg("confidence_score", 13, D),
            f_str("parser_source_id", 14),
            f_enum("preview_coverage_status", 15, E_COVERAGE),
            f_msg("preview_delta_pre_tax_dv01", 16, D),
            f_enum("preview_uncovered_reason", 17, E_UNCOVERED),
            f_msg("preview_delta_uncovered_notional", 18, D),
            f_msg("preview_delta_cash_t0", 19, D),
            f_msg("preview_delta_cash_t1", 20, D),
            f_u64("preview_at_seq", 21),
            f_u64("preview_quote_seq", 22),
            f_msg("confirmation_token", 23, H),
            f_msg("rule_lineage", 24, M_RULE_LINEAGE),
            f_str("preview_quote_source_id", 25),
            f_msg("context_hash", 26, H),
            f_opt_str("confirmed_by_trader_id", 27),
            f_str("valuation_settlement_date", 28),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        MODEL_PROTO,
        "RiskAndCoverageSummary",
        &[
            f_u32("covered_instrument_count", 1),
            f_u32("uncovered_instrument_count", 2),
            f_enum("covered_tax_basis", 3, TAX_BASIS),
            f_msg("covered_net_pre_tax_dv01", 4, D),
            f_msg("covered_gross_notional", 5, D),
            f_msg("uncovered_gross_notional", 6, D),
            f_msg("uncovered_net_notional", 7, D),
            f_bool("whole_account_hedge_allowed", 8),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        MODEL_PROTO,
        "FundingGapSummary",
        &[
            f_msg("opening_cash_t0", 1, D),
            f_msg("opening_net_cash_delta_t1", 2, D),
            f_msg("net_settlement_flow_t0", 3, D),
            f_msg("net_settlement_flow_t1", 4, D),
            f_msg("projected_cash_t0", 5, D),
            f_msg("projected_cash_t1", 6, D),
            f_msg("funding_gap_t0", 7, D),
            f_msg("funding_gap_t1", 8, D),
            f_u32("clean_price_estimated_funding_count", 9),
            f_u32("unpriced_funding_earmark_count", 10),
            f_bool("funding_is_complete_exact", 11),
            f_msg("pending_match_cash_t0", 12, D),
            f_msg("pending_match_cash_t1", 13, D),
            f_u32("pending_match_count", 14),
            f_u32("pending_unknown_cash_count", 15),
            f_bool("feed_incomplete", 16),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        MODEL_PROTO,
        "SeqRange",
        &[f_u64("first", 1), f_u64("last", 2)],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        MODEL_PROTO,
        "FeedStatusView",
        &[
            f_str("source_id", 1),
            f_u64("feed_status_revision", 2),
            f_enum("health_state", 3, E_FEED_HEALTH),
            f_bool("earmarks_paused", 4),
            f_bool("paused_by_gap", 5),
            f_bool("paused_by_staleness", 6),
            f_bool("paused_by_operator", 7),
            f_msg("last_heartbeat_at", 8, Z),
            f_u64("contiguous_feed_seq", 9),
            f_u64("max_seen_feed_seq", 10),
            f_rep_msg("missing_feed_ranges", 11, M_SEQ_RANGE),
            f_u32("open_pending_match_count", 12),
            f_str("status_reason", 13),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        MODEL_PROTO,
        "AccountBlotterSnapshot",
        &[
            f_u64("snapshot_seq", 1),
            f_str("account_id", 2),
            f_u64("account_version", 3),
            f_msg("opening_base", 4, M_OPENING_BASE),
            f_msg("risk_summary", 5, M_RISK_SUM),
            f_msg("funding_summary", 6, M_FUNDING_SUM),
            f_rep_msg("feed_statuses", 7, M_FEED_STATUS),
            f_rep_msg("positions", 8, M_POS_VIEW),
            f_rep_msg("active_earmarks", 9, M_EARMARK),
            f_rep_msg("open_pending_matches", 10, M_PENDING),
            f_rep_msg("pending_drafts", 11, M_DRAFT),
            f_rep_str("allowed_hedge_product_codes", 12),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        MODEL_PROTO,
        "AccountChange",
        &[
            f_str("account_id", 1),
            f_msg("replace_snapshot", 2, M_SNAPSHOT),
            f_rep_msg("changed_allocations", 3, M_ALLOC_VIEW),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        MODEL_PROTO,
        "BlotterEventEnvelope",
        &[
            f_u64("seq", 1),
            f_msg("occurred_at", 2, Z),
            f_rep_msg("changes", 3, M_ACCT_CHANGE),
            f_str("event_type", 4),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        MODEL_PROTO,
        "SnapshotFallbackRequired",
        &[
            f_u64("requested_after_seq", 1),
            f_u64("oldest_retained_seq", 2),
            f_u64("current_head_seq", 3),
            f_str("reason", 4),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        MODEL_PROTO,
        "StreamHeartbeat",
        &[
            f_u64("head_seq", 1),
            f_msg("server_time", 2, Z),
            f_u64("scanned_through_seq", 3),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        MODEL_PROTO,
        "PollEventsBatch",
        &[
            f_rep_msg("events", 1, M_ENVELOPE),
            f_u64("scanned_through_seq", 2),
            f_u64("current_head_seq", 3),
            f_bool("has_more", 4),
        ],
        &[],
    );

    // --- spi.proto (9 data + 7 req + 7 resp = 23 messages) ---
    assert_message_spec(
        &messages,
        &mut seen,
        SPI_PROTO,
        "TradeFillRecord",
        &[
            f_str("source_id", 1),
            f_str("external_trade_id", 2),
            f_u64("source_revision", 3),
            f_u64("feed_seq", 4),
            f_enum("action", 5, E_FEED_ACTION),
            f_str("account_id", 6),
            f_str("instrument_code", 7),
            f_enum("side", 8, E_SIDE),
            f_str("settlement_date", 9),
            f_msg("notional", 10, D),
            f_msg("quote_terms", 11, M_QUOTE_TERMS),
            f_msg("executed_at", 12, Z),
            f_u64("batch_first_feed_seq", 13),
            f_u64("batch_last_feed_seq", 14),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SPI_PROTO,
        "MarketQuoteTick",
        &[
            f_str("source_id", 1),
            f_u64("quote_seq", 2),
            f_str("instrument_code", 3),
            f_enum("source_type", 4, PRICE_SRC),
            f_msg("bid_clean_price", 5, D),
            f_msg("ask_clean_price", 6, D),
            f_msg("mid_clean_price", 7, D),
            f_msg("mid_yield_to_maturity", 8, D),
            f_msg("observed_at", 9, Z),
            f_msg("received_at", 10, Z),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SPI_PROTO,
        "NlpEarmarkProposal",
        &[
            f_str("parser_source_id", 1),
            f_str("model_version", 2),
            f_str("raw_source_text", 3),
            f_rep_msg("anchors", 4, M_ANCHOR),
            f_msg("confidence_score", 5, D),
            f_str("account_id", 6),
            f_str("instrument_code", 7),
            f_enum("side", 8, E_SIDE),
            f_enum("settlement_bucket", 9, E_BUCKET),
            f_msg("notional", 10, D),
            f_msg("quote_terms", 11, M_QUOTE_TERMS),
            f_enum("suggested_target_state", 12, E_DRAFT_TARGET),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SPI_PROTO,
        "TradeFillIngestOutcome",
        &[
            f_str("source_id", 1),
            f_str("external_trade_id", 2),
            f_u64("source_revision", 3),
            f_u64("feed_seq", 4),
            f_enum("kind", 5, E_OUTCOME_KIND),
            f_msg("pending_match_id", 6, U),
            f_rep_msg("allocation_ids", 7, U),
            f_opt_u64("effective_revision", 8),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SPI_PROTO,
        "PullTradeFeedBatchResult",
        &[
            f_rep_msg("fills", 1, M_FILL),
            f_u64("scanned_through_feed_seq", 2),
            f_bool("has_more", 3),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SPI_PROTO,
        "QuoteCursor",
        &[
            f_str("source_id", 1),
            f_str("instrument_code", 2),
            f_u64("after_quote_seq", 3),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SPI_PROTO,
        "PullMarketQuotesResult",
        &[
            f_rep_msg("ticks", 1, M_QUOTE_TICK),
            f_msg("next_cursor", 2, M_QUOTE_CURSOR),
            f_bool("has_more", 3),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SPI_PROTO,
        "FeedHeartbeat",
        &[
            f_str("source_id", 1),
            f_u64("contiguous_feed_seq", 2),
            f_msg("server_time", 3, Z),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SPI_PROTO,
        "QuoteHeartbeat",
        &[
            f_msg("cursor", 1, M_QUOTE_CURSOR),
            f_msg("server_time", 2, Z),
        ],
        &[],
    );

    assert_message_spec(
        &messages,
        &mut seen,
        SPI_PROTO,
        "FetchOpeningPositionBaseRequest",
        &[
            f_str("request_id", 1),
            f_str("account_id", 2),
            f_str("trading_date", 3),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SPI_PROTO,
        "FetchOpeningPositionBaseResponse",
        &[
            f_oneof_msg("value", 1, M_OPENING_BASE, "result"),
            f_oneof_msg("error", 2, E, "result"),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SPI_PROTO,
        "PullTradeFeedBatchRequest",
        &[
            f_str("source_id", 1),
            f_u64("after_feed_seq", 2),
            f_u32("max_records", 3),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SPI_PROTO,
        "PullTradeFeedBatchResponse",
        &[
            f_oneof_msg("value", 1, M_PULL_FEED_RES, "result"),
            f_oneof_msg("error", 2, E, "result"),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SPI_PROTO,
        "StreamTradeFeedRequest",
        &[f_str("source_id", 1), f_u64("after_feed_seq", 2)],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SPI_PROTO,
        "StreamTradeFeedResponse",
        &[
            f_oneof_msg("fill", 1, M_FILL, "payload"),
            f_oneof_msg("heartbeat", 2, M_FEED_HB, "payload"),
            f_oneof_msg("error", 3, E, "payload"),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SPI_PROTO,
        "FetchTradeRevisionRequest",
        &[
            f_str("source_id", 1),
            f_str("external_trade_id", 2),
            f_u64("source_revision", 3),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SPI_PROTO,
        "FetchTradeRevisionResponse",
        &[
            f_oneof_msg("value", 1, M_FILL, "result"),
            f_oneof_msg("error", 2, E, "result"),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SPI_PROTO,
        "PullMarketQuotesRequest",
        &[f_msg("cursor", 1, M_QUOTE_CURSOR), f_u32("max_ticks", 2)],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SPI_PROTO,
        "PullMarketQuotesResponse",
        &[
            f_oneof_msg("value", 1, M_PULL_QUOTES_RES, "result"),
            f_oneof_msg("error", 2, E, "result"),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SPI_PROTO,
        "StreamMarketQuotesRequest",
        &[f_msg("cursor", 1, M_QUOTE_CURSOR)],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SPI_PROTO,
        "StreamMarketQuotesResponse",
        &[
            f_oneof_msg("tick", 1, M_QUOTE_TICK, "payload"),
            f_oneof_msg("heartbeat", 2, M_QUOTE_HB, "payload"),
            f_oneof_msg("error", 3, E, "payload"),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SPI_PROTO,
        "ParseNlpIntentRequest",
        &[
            f_str("request_id", 1),
            f_str("account_id", 2),
            f_str("raw_source_text", 3),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SPI_PROTO,
        "ParseNlpIntentResponse",
        &[
            f_oneof_msg("value", 1, M_NLP_PROPOSAL, "result"),
            f_oneof_msg("error", 2, E, "result"),
        ],
        &[],
    );

    // --- service.proto (18 req + 17 result + 18 resp = 53 messages) ---
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "ImportOpeningBaseRequest",
        &[
            f_str("idempotency_key", 1),
            f_msg("opening_base", 2, M_OPENING_BASE),
            f_u64("expected_account_version", 3),
        ],
        &[4, 5],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "ImportOpeningBaseResult",
        &[
            f_u64("committed_seq", 1),
            f_bool("replayed", 2),
            f_msg("snapshot", 3, M_SNAPSHOT),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "ImportOpeningBaseResponse",
        &[
            f_oneof_msg(
                "value",
                1,
                ".ficant.blotter.v1.ImportOpeningBaseResult",
                "result",
            ),
            f_oneof_msg("error", 2, E, "result"),
        ],
        &[],
    );

    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "IngestMarketQuotesRequest",
        &[
            f_str("idempotency_key", 1),
            f_str("account_id", 2),
            f_rep_msg("ticks", 3, M_QUOTE_TICK),
            f_str("source_id", 4),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "IngestMarketQuotesResult",
        &[
            f_u64("committed_seq", 1),
            f_bool("replayed", 2),
            f_u32("applied_tick_count", 3),
            f_u32("skipped_stale_tick_count", 4),
            f_msg("risk_summary", 5, M_RISK_SUM),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "IngestMarketQuotesResponse",
        &[
            f_oneof_msg(
                "value",
                1,
                ".ficant.blotter.v1.IngestMarketQuotesResult",
                "result",
            ),
            f_oneof_msg("error", 2, E, "result"),
        ],
        &[],
    );

    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "CreateHoldRequest",
        &[
            f_str("idempotency_key", 1),
            f_str("account_id", 2),
            f_u64("expected_position_version", 3),
            f_str("instrument_code", 4),
            f_enum("side", 5, E_SIDE),
            f_enum("settlement_bucket", 6, E_BUCKET),
            f_msg("notional", 7, D),
            f_msg("quote_terms", 8, M_QUOTE_TERMS),
            f_u32("ttl_seconds", 9),
            f_str("note", 10),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "CreateHoldResult",
        &[
            f_u64("committed_seq", 1),
            f_bool("replayed", 2),
            f_msg("earmark", 3, M_EARMARK),
            f_msg("position", 4, M_POS_VIEW),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "CreateHoldResponse",
        &[
            f_oneof_msg("value", 1, ".ficant.blotter.v1.CreateHoldResult", "result"),
            f_oneof_msg("error", 2, E, "result"),
        ],
        &[],
    );

    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "ExtendHoldRequest",
        &[
            f_str("idempotency_key", 1),
            f_str("account_id", 2),
            f_msg("earmark_id", 3, U),
            f_u64("expected_earmark_revision", 4),
            f_u32("ttl_seconds", 5),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "ExtendHoldResult",
        &[
            f_u64("committed_seq", 1),
            f_bool("replayed", 2),
            f_msg("earmark", 3, M_EARMARK),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "ExtendHoldResponse",
        &[
            f_oneof_msg("value", 1, ".ficant.blotter.v1.ExtendHoldResult", "result"),
            f_oneof_msg("error", 2, E, "result"),
        ],
        &[],
    );

    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "ConfirmHoldRequest",
        &[
            f_str("idempotency_key", 1),
            f_str("account_id", 2),
            f_msg("earmark_id", 3, U),
            f_u64("expected_earmark_revision", 4),
            f_u64("expected_position_version", 5),
            f_msg("quote_terms", 6, M_QUOTE_TERMS),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "ConfirmHoldResult",
        &[
            f_u64("committed_seq", 1),
            f_bool("replayed", 2),
            f_msg("earmark", 3, M_EARMARK),
            f_msg("position", 4, M_POS_VIEW),
            f_msg("risk_summary", 5, M_RISK_SUM),
            f_msg("funding_summary", 6, M_FUNDING_SUM),
            f_msg("reconfirmation_error", 7, E),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "ConfirmHoldResponse",
        &[
            f_oneof_msg("value", 1, ".ficant.blotter.v1.ConfirmHoldResult", "result"),
            f_oneof_msg("error", 2, E, "result"),
        ],
        &[],
    );

    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "ReleaseHoldRequest",
        &[
            f_str("idempotency_key", 1),
            f_str("account_id", 2),
            f_msg("earmark_id", 3, U),
            f_u64("expected_earmark_revision", 4),
            f_str("reason", 5),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "ReleaseHoldResult",
        &[
            f_u64("committed_seq", 1),
            f_bool("replayed", 2),
            f_msg("earmark", 3, M_EARMARK),
            f_msg("position", 4, M_POS_VIEW),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "ReleaseHoldResponse",
        &[
            f_oneof_msg("value", 1, ".ficant.blotter.v1.ReleaseHoldResult", "result"),
            f_oneof_msg("error", 2, E, "result"),
        ],
        &[],
    );

    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "CreateCommittedRequest",
        &[
            f_str("idempotency_key", 1),
            f_str("account_id", 2),
            f_u64("expected_position_version", 3),
            f_str("instrument_code", 4),
            f_enum("side", 5, E_SIDE),
            f_enum("settlement_bucket", 6, E_BUCKET),
            f_msg("notional", 7, D),
            f_msg("quote_terms", 8, M_QUOTE_TERMS),
            f_str("note", 9),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "CreateCommittedResult",
        &[
            f_u64("committed_seq", 1),
            f_bool("replayed", 2),
            f_msg("earmark", 3, M_EARMARK),
            f_msg("position", 4, M_POS_VIEW),
            f_msg("risk_summary", 5, M_RISK_SUM),
            f_msg("funding_summary", 6, M_FUNDING_SUM),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "CreateCommittedResponse",
        &[
            f_oneof_msg(
                "value",
                1,
                ".ficant.blotter.v1.CreateCommittedResult",
                "result",
            ),
            f_oneof_msg("error", 2, E, "result"),
        ],
        &[],
    );

    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "CancelCommittedRequest",
        &[
            f_str("idempotency_key", 1),
            f_str("account_id", 2),
            f_msg("earmark_id", 3, U),
            f_u64("expected_earmark_revision", 4),
            f_str("reason", 5),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "CancelCommittedResult",
        &[
            f_u64("committed_seq", 1),
            f_bool("replayed", 2),
            f_msg("earmark", 3, M_EARMARK),
            f_msg("position", 4, M_POS_VIEW),
            f_msg("risk_summary", 5, M_RISK_SUM),
            f_msg("funding_summary", 6, M_FUNDING_SUM),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "CancelCommittedResponse",
        &[
            f_oneof_msg(
                "value",
                1,
                ".ficant.blotter.v1.CancelCommittedResult",
                "result",
            ),
            f_oneof_msg("error", 2, E, "result"),
        ],
        &[],
    );

    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "SubmitDraftEarmarkRequest",
        &[
            f_str("idempotency_key", 1),
            f_str("account_id", 2),
            f_msg("proposal", 3, M_NLP_PROPOSAL),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "SubmitDraftEarmarkResult",
        &[
            f_u64("committed_seq", 1),
            f_bool("replayed", 2),
            f_msg("draft", 3, M_DRAFT),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "SubmitDraftEarmarkResponse",
        &[
            f_oneof_msg(
                "value",
                1,
                ".ficant.blotter.v1.SubmitDraftEarmarkResult",
                "result",
            ),
            f_oneof_msg("error", 2, E, "result"),
        ],
        &[],
    );

    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "ConfirmDraftEarmarkRequest",
        &[
            f_str("idempotency_key", 1),
            f_str("account_id", 2),
            f_msg("draft_id", 3, U),
            f_u64("expected_draft_revision", 4),
            f_u64("expected_position_version", 5),
            f_msg("confirmation_token", 6, H),
            f_bool("acknowledge_uncovered_dv01", 7),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "ConfirmDraftEarmarkResult",
        &[
            f_u64("committed_seq", 1),
            f_bool("replayed", 2),
            f_msg("draft", 3, M_DRAFT),
            f_msg("earmark", 4, M_EARMARK),
            f_msg("position", 5, M_POS_VIEW),
            f_msg("risk_summary", 6, M_RISK_SUM),
            f_msg("funding_summary", 7, M_FUNDING_SUM),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "ConfirmDraftEarmarkResponse",
        &[
            f_oneof_msg(
                "value",
                1,
                ".ficant.blotter.v1.ConfirmDraftEarmarkResult",
                "result",
            ),
            f_oneof_msg("error", 2, E, "result"),
        ],
        &[],
    );

    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "IngestTradeFeedRequest",
        &[
            f_str("idempotency_key", 1),
            f_str("source_id", 2),
            f_rep_msg("fills", 3, M_FILL),
            f_msg("heartbeat_at", 4, Z),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "IngestTradeFeedResult",
        &[
            f_u64("committed_seq", 1),
            f_bool("replayed", 2),
            f_rep_msg("item_outcomes", 3, M_FILL_OUTCOME),
            f_msg("feed_status", 4, M_FEED_STATUS),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "IngestTradeFeedResponse",
        &[
            f_oneof_msg(
                "value",
                1,
                ".ficant.blotter.v1.IngestTradeFeedResult",
                "result",
            ),
            f_oneof_msg("error", 2, E, "result"),
        ],
        &[],
    );

    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "ResolvePendingMatchRequest",
        &[
            f_str("idempotency_key", 1),
            f_str("account_id", 2),
            f_msg("pending_match_id", 3, U),
            f_u64("expected_pending_revision", 4),
            f_u64("expected_position_version", 5),
            f_enum("action", 6, E_RESOLVE_ACTION),
            f_opt_u64("expected_earmark_revision", 7),
            f_msg("target_earmark_id", 8, U),
            f_msg("offset_notional", 9, D),
            f_str("reason", 10),
            f_msg("baseline_delta", 11, M_BASELINE_DELTA),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "ResolvePendingMatchResult",
        &[
            f_u64("committed_seq", 1),
            f_bool("replayed", 2),
            f_msg("pending_match", 3, M_PENDING),
            f_msg("updated_earmark", 4, M_EARMARK),
            f_msg("position", 5, M_POS_VIEW),
            f_msg("risk_summary", 6, M_RISK_SUM),
            f_msg("funding_summary", 7, M_FUNDING_SUM),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "ResolvePendingMatchResponse",
        &[
            f_oneof_msg(
                "value",
                1,
                ".ficant.blotter.v1.ResolvePendingMatchResult",
                "result",
            ),
            f_oneof_msg("error", 2, E, "result"),
        ],
        &[],
    );

    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "SweepLifecycleRequest",
        &[
            f_str("idempotency_key", 1),
            f_opt_str("account_id", 2),
            f_opt_str("source_id", 3),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "SweepLifecycleResult",
        &[
            f_u64("head_seq", 1),
            f_bool("replayed", 2),
            f_u32("expired_hold_count", 3),
            f_u32("newly_alerted_committed_count", 4),
            f_u32("newly_stale_feed_count", 5),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "SweepLifecycleResponse",
        &[
            f_oneof_msg(
                "value",
                1,
                ".ficant.blotter.v1.SweepLifecycleResult",
                "result",
            ),
            f_oneof_msg("error", 2, E, "result"),
        ],
        &[],
    );

    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "SetFeedControlRequest",
        &[
            f_str("idempotency_key", 1),
            f_str("source_id", 2),
            f_u64("expected_feed_status_revision", 3),
            f_bool("paused_by_operator", 4),
            f_bool("force_clear_gap_pause", 5),
            f_str("reason", 6),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "SetFeedControlResult",
        &[
            f_u64("committed_seq", 1),
            f_bool("replayed", 2),
            f_msg("feed_status", 3, M_FEED_STATUS),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "SetFeedControlResponse",
        &[
            f_oneof_msg(
                "value",
                1,
                ".ficant.blotter.v1.SetFeedControlResult",
                "result",
            ),
            f_oneof_msg("error", 2, E, "result"),
        ],
        &[],
    );

    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "PreviewCoveredHedgeRequest",
        &[
            f_str("account_id", 1),
            f_enum("scope", 2, E_HEDGE_SCOPE),
            f_str("product_code", 3),
            f_str("contract_code", 4),
            f_msg("ctd_dv01_per_100", 5, D),
            f_msg("conversion_factor", 6, D),
            f_msg("delivery_rule_pack", 7, RULE_PACK),
            f_msg("valuation_at", 8, Z),
            f_opt_u64("expected_snapshot_seq", 9),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "PreviewCoveredHedgeResult",
        &[
            f_u64("evaluated_at_seq", 1),
            f_enum("scope", 2, E_HEDGE_SCOPE),
            f_str("product_code", 3),
            f_str("contract_code", 4),
            f_bool("zero_net_exposure", 5),
            f_enum("covered_tax_basis", 6, TAX_BASIS),
            f_u32("covered_instrument_count", 7),
            f_u32("uncovered_instrument_count", 8),
            f_msg("covered_net_pre_tax_dv01", 9, D),
            f_msg("excluded_uncovered_gross_notional", 10, D),
            f_msg("measures", 11, HEDGE_MEAS),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "PreviewCoveredHedgeResponse",
        &[
            f_oneof_msg(
                "value",
                1,
                ".ficant.blotter.v1.PreviewCoveredHedgeResult",
                "result",
            ),
            f_oneof_msg("error", 2, E, "result"),
        ],
        &[],
    );

    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "GetAccountSnapshotRequest",
        &[f_str("account_id", 1)],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "GetAccountSnapshotResult",
        &[f_msg("snapshot", 1, M_SNAPSHOT)],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "GetAccountSnapshotResponse",
        &[
            f_oneof_msg(
                "value",
                1,
                ".ficant.blotter.v1.GetAccountSnapshotResult",
                "result",
            ),
            f_oneof_msg("error", 2, E, "result"),
        ],
        &[],
    );

    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "PollEventsRequest",
        &[
            f_u64("after_seq", 1),
            f_u32("max_events", 2),
            f_opt_str("account_id", 3),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "PollEventsResult",
        &[
            f_oneof_msg("batch", 1, M_POLL_BATCH, "outcome"),
            f_oneof_msg("snapshot_fallback", 2, M_FALLBACK, "outcome"),
        ],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "PollEventsResponse",
        &[
            f_oneof_msg("value", 1, ".ficant.blotter.v1.PollEventsResult", "result"),
            f_oneof_msg("error", 2, E, "result"),
        ],
        &[],
    );

    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "SubscribeEventsRequest",
        &[f_u64("after_seq", 1), f_opt_str("account_id", 2)],
        &[],
    );
    assert_message_spec(
        &messages,
        &mut seen,
        SERVICE_PROTO,
        "SubscribeEventsResponse",
        &[
            f_oneof_msg("event", 1, M_ENVELOPE, "payload"),
            f_oneof_msg("heartbeat", 2, M_STREAM_HB, "payload"),
            f_oneof_msg("snapshot_fallback", 3, M_FALLBACK, "payload"),
            f_oneof_msg("error", 4, E, "payload"),
        ],
        &[],
    );

    assert_eq!(
        seen.len(),
        messages.len(),
        "every blotter message must be verified"
    );
}

#[test]
fn generated_rust_types_and_oneofs_round_trip() {
    let dec = |coeff: &str, scale: u32| core::DecimalValue {
        coefficient: coeff.to_string(),
        scale,
        unit: Some(core::UnitRef {
            unit_id: Some(core::Ulid {
                value: "01ARZ3NDEKTSV4RRFFQ69G5FAA".to_string(),
            }),
            version: 1,
        }),
    };

    let quote = blotter::QuoteTerms {
        price_or_yield: Some(blotter::quote_terms::PriceOrYield::CleanPricePer100(dec(
            "10025", 2,
        ))),
        settlement_amount: Some(dec("100250000", 2)),
    };
    assert_eq!(
        blotter::QuoteTerms::decode(quote.encode_to_vec().as_slice()).unwrap(),
        quote
    );

    let poll_res = blotter::PollEventsResponse {
        result: Some(blotter::poll_events_response::Result::Value(
            blotter::PollEventsResult {
                outcome: Some(blotter::poll_events_result::Outcome::SnapshotFallback(
                    blotter::SnapshotFallbackRequired {
                        requested_after_seq: 1,
                        oldest_retained_seq: 10,
                        current_head_seq: 42,
                        reason: "trimmed".to_string(),
                    },
                )),
            },
        )),
    };
    assert_eq!(
        blotter::PollEventsResponse::decode(poll_res.encode_to_vec().as_slice()).unwrap(),
        poll_res
    );

    let stream_msg = blotter::SubscribeEventsResponse {
        payload: Some(blotter::subscribe_events_response::Payload::Event(
            blotter::BlotterEventEnvelope {
                seq: 11,
                occurred_at: None,
                changes: vec![],
                event_type: "INGEST_TRADE_FEED".to_string(),
            },
        )),
    };
    assert_eq!(
        blotter::SubscribeEventsResponse::decode(stream_msg.encode_to_vec().as_slice()).unwrap(),
        stream_msg
    );

    let confirm_expired = blotter::ConfirmHoldResult {
        committed_seq: 12,
        replayed: false,
        earmark: None,
        position: None,
        risk_summary: None,
        funding_summary: None,
        reconfirmation_error: Some(core::ErrorDetail {
            code: core::ErrorCode::ValidationFailed.into(),
            message: "expired hold reconfirmation rejected".to_string(),
            trace_id: String::new(),
            retryable: false,
            resource_ref: String::new(),
            field_violations: vec![],
        }),
    };
    assert_eq!(
        blotter::ConfirmHoldResult::decode(confirm_expired.encode_to_vec().as_slice()).unwrap(),
        confirm_expired
    );

    let outcome = blotter::TradeFillIngestOutcome {
        source_id: "cfets-1".to_string(),
        external_trade_id: "T-1001".to_string(),
        source_revision: 1,
        feed_seq: 11,
        kind: blotter::TradeFillOutcomeKind::AutoMatched.into(),
        pending_match_id: None,
        allocation_ids: vec![core::Ulid {
            value: "01ARZ3NDEKTSV4RRFFQ69G5FAB".to_string(),
        }],
        effective_revision: Some(1),
    };
    assert_eq!(
        blotter::TradeFillIngestOutcome::decode(outcome.encode_to_vec().as_slice()).unwrap(),
        outcome
    );

    // Compile-time check that all 5 tonic client and server modules are exported.
    let _ = std::any::type_name::<
        blotter::desk_blotter_service_client::DeskBlotterServiceClient<tonic::transport::Channel>,
    >();
    let _ = std::any::type_name::<
        blotter::position_base_provider_service_client::PositionBaseProviderServiceClient<
            tonic::transport::Channel,
        >,
    >();
    let _ = std::any::type_name::<
        blotter::trade_feed_reconciler_service_client::TradeFeedReconcilerServiceClient<
            tonic::transport::Channel,
        >,
    >();
    let _ = std::any::type_name::<
        blotter::market_quote_stream_service_client::MarketQuoteStreamServiceClient<
            tonic::transport::Channel,
        >,
    >();
    let _ = std::any::type_name::<
        blotter::intranet_nlp_intent_parser_service_client::IntranetNlpIntentParserServiceClient<
            tonic::transport::Channel,
        >,
    >();
    assert_eq!(
        blotter::desk_blotter_service_server::DeskBlotterServiceServer::<()>::NAME,
        "ficant.blotter.v1.DeskBlotterService"
    );
    assert_eq!(
        blotter::position_base_provider_service_server::PositionBaseProviderServiceServer::<()>::NAME,
        "ficant.blotter.v1.PositionBaseProviderService"
    );
    assert_eq!(
        blotter::trade_feed_reconciler_service_server::TradeFeedReconcilerServiceServer::<()>::NAME,
        "ficant.blotter.v1.TradeFeedReconcilerService"
    );
    assert_eq!(
        blotter::market_quote_stream_service_server::MarketQuoteStreamServiceServer::<()>::NAME,
        "ficant.blotter.v1.MarketQuoteStreamService"
    );
    assert_eq!(
        blotter::intranet_nlp_intent_parser_service_server::IntranetNlpIntentParserServiceServer::<()>::NAME,
        "ficant.blotter.v1.IntranetNlpIntentParserService"
    );
}

fn decode_hex(hex: &str) -> Vec<u8> {
    assert_eq!(hex.len() % 2, 0);
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("valid hex byte"))
        .collect()
}

#[test]
fn bp1_legacy_wire_vectors_decode_cleanly_in_rust() {
    const LEGACY_PRETAX_BOND_RESPONSE_HEX: &str = concat!(
        "0a82040a99010801120a323032362d30382d30381a0a323032362d30382d313022280a0232",
        "3510011a200a1c0a1a303141525a334e44454b545356345252464651363947354641411001",
        "2a270a033130301a200a1c0a1a303141525a334e44454b5453563452524646513639473546",
        "41411001322a0a043130323510011a200a1c0a1a303141525a334e44454b54535634525246",
        "465136394735464141100112e3020a320a0c313431303935383930343131100c1a200a1c0a",
        "1a303141525a334e44454b54535634525246465136394735464142100112270a033130301a",
        "200a1c0a1a303141525a334e44454b5453563452524646513639473546414210011a340a0e",
        "3130313431303935383930343131100c1a200a1c0a1a303141525a334e44454b5453563452",
        "5246465136394735464142100122280a02323510031a200a1c0a1a30314b32434742564154",
        "3030303030303030303030303030303010012a280a02383510011a200a1c0a1a303141525a",
        "334e44454b54535634525246465136394735464144100132280a02383310011a200a1c0a1a",
        "303141525a334e44454b5453563452524646513639473546414410013a260a0238301a200a",
        "1c0a1a303141525a334e44454b54535634525246465136394735464145100142280a023833",
        "10021a200a1c0a1a303141525a334e44454b545356345252464651363947354641461001"
    );
    const LEGACY_TAX_ADJUSTED_BOND_RESPONSE_HEX: &str = concat!(
        "0ae4050a99010801120a323032362d30382d30381a0a323032362d30382d313022280a0232",
        "3510011a200a1c0a1a303141525a334e44454b545356345252464651363947354641411001",
        "2a270a033130301a200a1c0a1a303141525a334e44454b5453563452524646513639473546",
        "41411001322a0a043130323510011a200a1c0a1a303141525a334e44454b54535634525246",
        "465136394735464141100112e3020a320a0c313431303935383930343131100c1a200a1c0a",
        "1a303141525a334e44454b54535634525246465136394735464142100112270a033130301a",
        "200a1c0a1a303141525a334e44454b5453563452524646513639473546414210011a340a0e",
        "3130313431303935383930343131100c1a200a1c0a1a303141525a334e44454b5453563452",
        "5246465136394735464142100122280a02323510031a200a1c0a1a30314b32434742564154",
        "3030303030303030303030303030303010012a280a02383510011a200a1c0a1a303141525a",
        "334e44454b54535634525246465136394735464144100132280a02383310011a200a1c0a1a",
        "303141525a334e44454b5453563452524646513639473546414410013a260a0238301a200a",
        "1c0a1a303141525a334e44454b54535634525246465136394735464145100142280a023833",
        "10021a200a1c0a1a303141525a334e44454b5453563452524646513639473546414610011a",
        "df010aaf010801120a323032362d30382d30381a0a323032362d30382d313022330a0d3233",
        "3538343930353636303338100c1a200a1c0a1a303141525a334e44454b5453563452524646",
        "513639473546414110012a270a033130301a200a1c0a1a303141525a334e44454b54535634",
        "525246465136394735464141100132350a0f313032333538343930353636303338100c1a20",
        "0a1c0a1a303141525a334e44454b54535634525246465136394735464141100112290a0332",
        "333510041a200a1c0a1a30314b324347425641543030303030303030303030303030303010",
        "011801"
    );
    const LEGACY_TS_HEDGE_RESPONSE_HEX: &str = concat!(
        "0adb010ad8010a350a0f323930333737343930373337393539100c1a200a1c0a1a30314152",
        "5a334e44454b54535634525246465136394735464147100112330a0d2d3836303934383237",
        "35383632100b1a200a1c0a1a303141525a334e44454b545356345252464651363947354641",
        "48100118f7ffffffffffffffff0122340a0e3131333339373431363634313633100c1a200a",
        "1c0a1a303141525a334e44454b5453563452524646513639473546414710012a290a033939",
        "3910031a200a1c0a1a303141525a334e44454b545356345252464651363947354641481001"
    );

    let pretax =
        rates::AnalyzeBondResponse::decode(decode_hex(LEGACY_PRETAX_BOND_RESPONSE_HEX).as_slice())
            .expect("decode legacy pretax response");
    match pretax.result {
        Some(rates::analyze_bond_response::Result::Analysis(res)) => {
            assert_eq!(res.cashflows.len(), 1);
            assert!(res.measures.is_some());
            assert!(res.after_tax.is_none());
            assert_eq!(res.execution_mode, rates::BondTaxBasis::Unspecified as i32);
            assert_eq!(
                res.measures_tax_basis,
                rates::BondTaxBasis::Unspecified as i32
            );
        }
        other => panic!("unexpected pretax result: {other:?}"),
    }

    let tax = rates::AnalyzeBondResponse::decode(
        decode_hex(LEGACY_TAX_ADJUSTED_BOND_RESPONSE_HEX).as_slice(),
    )
    .expect("decode legacy tax-adjusted response");
    match tax.result {
        Some(rates::analyze_bond_response::Result::Analysis(res)) => {
            let after_tax = res.after_tax.expect("after_tax present");
            assert_eq!(
                after_tax.claim_scope,
                market::CouponTaxClaimScope::CouponOutputVatBeforeInputCredit as i32
            );
            assert_eq!(after_tax.tax_basis, rates::BondTaxBasis::Unspecified as i32);
        }
        other => panic!("unexpected tax result: {other:?}"),
    }

    let hedge = rates::AnalyzeFuturesHedgeResponse::decode(
        decode_hex(LEGACY_TS_HEDGE_RESPONSE_HEX).as_slice(),
    )
    .expect("decode legacy TS hedge response");
    match hedge.result {
        Some(rates::analyze_futures_hedge_response::Result::Analysis(res)) => {
            let measures = res.measures.expect("measures present");
            assert_eq!(measures.recommended_contracts, -9);
            assert!(measures.contract_notional.is_none());
        }
        other => panic!("unexpected hedge result: {other:?}"),
    }
}
