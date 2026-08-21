use std::fs;
use std::path::PathBuf;

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("contract-test crate must live under the repository crates directory")
        .to_path_buf()
}

fn proto(relative: &str) -> String {
    fs::read_to_string(repository_root().join(relative))
        .unwrap_or_else(|error| panic!("failed to read {relative}: {error}"))
}

#[test]
fn r7b_declares_one_shared_formal_output_evidence_contract() {
    let evidence = proto("interface/proto/ficant/core/v1/evidence.proto");
    for required in [
        "enum FormalInputKind",
        "FORMAL_INPUT_KIND_CURVE_NODE_DEFINITION = 15;",
        "FORMAL_INPUT_KIND_PORTFOLIO = 16;",
        "FORMAL_INPUT_KIND_BOOK = 17;",
        "FORMAL_INPUT_KIND_PORTFOLIO_GROUP = 18;",
        "FORMAL_INPUT_KIND_BENCHMARK = 19;",
        "FORMAL_INPUT_KIND_PORTFOLIO_METRIC_CONVENTION = 20;",
        "FORMAL_INPUT_KIND_FACT = 21;",
        "message NamedContentRef",
        "message FormalInputBinding",
        "oneof reference",
        "LineageRef object_ref = 4;",
        "NamedContentRef named_ref = 9;",
        "MarketTime observed_at = 5;",
        "MarketTime effective_to = 8;",
        "message CodeBinding",
        "message RuntimeBinding",
        "message FormalImplementationBinding",
        "message FormalOutputEvidence",
        "optional uint64 seed = 8;",
    ] {
        assert!(
            evidence.contains(required),
            "core evidence contract is missing {required}"
        );
    }

    assert!(
        !evidence.contains("LineageRef exact_ref"),
        "formal evidence must not force named definitions into fabricated object identities"
    );

    let rates = proto("interface/proto/ficant/rates/v1/analytics.proto");
    assert!(rates.contains("ficant.core.v1.FormalOutputEvidence formal_evidence = 11;"));

    let exposure = proto("interface/proto/ficant/research/v1/exposure.proto");
    assert!(exposure.contains("ficant.core.v1.FormalOutputEvidence formal_evidence = 11;"));

    let position = proto("interface/proto/ficant/research/v1/position.proto");
    assert_eq!(
        position
            .matches("ficant.core.v1.FormalOutputEvidence formal_evidence = 6;")
            .count(),
        2
    );

    let health = proto("interface/proto/ficant/research/v1/health.proto");
    assert!(health.contains("ficant.core.v1.FormalOutputEvidence formal_evidence = 18;"));

    let artifact = proto("interface/proto/ficant/research/v1/artifact.proto");
    assert!(artifact.contains("ficant.core.v1.FormalOutputEvidence formal_evidence = 8;"));

    let signal = proto("interface/proto/ficant/research/v1/signal.proto");
    assert!(signal.contains("ficant.core.v1.FormalOutputEvidence formal_evidence = 11;"));

    let portfolio = proto("interface/proto/ficant/portfolio/v1/portfolio.proto");
    assert!(portfolio.contains("ficant.core.v1.FormalOutputEvidence formal_evidence = 11;"));
}

#[test]
fn r7b_graph_contract_adds_exact_subject_code_and_thirteen_dimensions() {
    let execution = proto("interface/proto/ficant/research/v1/execution.proto");
    assert!(execution.contains("ficant.core.v1.FormalInputBinding subject = 12;"));
    assert!(execution.contains("ficant.core.v1.CodeBinding code = 13;"));
    assert!(execution.contains("ficant.core.v1.FormalOutputEvidence formal_evidence = 5;"));

    let experiment = proto("interface/proto/ficant/research/v1/experiment.proto");
    assert!(experiment.contains("ficant.core.v1.FormalInputBinding subject = 12;"));
    assert!(
        experiment.contains("repeated ficant.core.v1.FormalOutputEvidence formal_outputs = 4;")
    );
    assert!(experiment.contains("GRAPH_RUN_COMPARISON_DIMENSION_SUBJECT = 12;"));
    assert!(experiment.contains("GRAPH_RUN_COMPARISON_DIMENSION_CODE = 13;"));
}
