use chrono::{TimeZone, Utc};
use ficant_domain::market::PriceSourceType;
use ficant_domain::primitives::{
    ContentHash, DecimalValue, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::research::{
    AccountingBook, AccountingClassification, AccountingClassificationState, DataHealthIssueCode,
    DataHealthPriceEvidence, DataHealthPriceEvidenceInput, DataHealthReport, DataHealthReportInput,
    DataHealthState, DataHealthThresholdProfile, DataHealthThresholdProfileInput, Position,
    PositionHealthEvaluation, PositionHoldingForm, PositionInput, PositionSetState,
    PositionSnapshot, PositionSnapshotInput, evaluate_position_snapshot,
};
use ficant_domain::{ContentAddressed, Lineaged};

const PREFIX: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA";

#[test]
fn verified_empty_snapshot_is_a_warning_with_positive_empty_state_and_zero_coverage() {
    let snapshot = empty_snapshot();
    let profile = profile(3_600, 5_000, 7_200, 5_000);
    let evaluation = evaluate_position_snapshot(&snapshot, &profile, &market_time(7_200)).unwrap();

    assert_eq!(
        evaluation.position_set_state(),
        PositionSetState::VerifiedEmpty
    );
    assert_eq!(evaluation.coverage().imported_position_count(), 0);
    assert_eq!(evaluation.coverage().participating_position_count(), 0);
    assert!(
        evaluation
            .coverage()
            .imported_gross_economic_value_by_unit()
            .is_empty()
    );
    assert!(
        evaluation
            .coverage()
            .participating_gross_economic_value_by_unit()
            .is_empty()
    );
    assert_eq!(
        evaluation.coverage().missing_critical_field_record_count(),
        0
    );
    assert!(evaluation.coverage().source_confidence().is_none());
    assert_eq!(
        evaluation
            .coverage()
            .distinct_external_data_source_version_count(),
        0
    );
    assert_eq!(evaluation.position_snapshot_hash(), snapshot.content_hash());
    assert!(
        evaluation
            .issues()
            .iter()
            .any(|issue| issue.code() == DataHealthIssueCode::EmptyPositions)
    );
}

#[test]
fn threshold_profile_is_exactly_content_addressed() {
    let mut input = profile_input(3_600, 5_000, 7_200, 5_000);
    input.content_hash = DataHealthThresholdProfile::content_hash_for(&input);
    let profile = DataHealthThresholdProfile::new(input.clone()).unwrap();
    assert_eq!(profile.content_hash(), &input.content_hash);

    input.unknown_accounting_warning_basis_points = 5_001;
    assert!(DataHealthThresholdProfile::new(input).is_err());
}

#[test]
fn unknown_ratio_uses_exact_integer_threshold_and_age_uses_full_instant() {
    let snapshot = snapshot_with_positions(
        'S',
        0,
        vec![
            position('A', AccountingClassificationState::Unknown),
            position('B', AccountingClassificationState::Classified),
            position('C', AccountingClassificationState::Unknown),
            position('D', AccountingClassificationState::Classified),
        ],
    );
    let profile = profile(3_600, 5_000, 7_200, 5_000);

    let exact = evaluate_position_snapshot(&snapshot, &profile, &market_time(3_600)).unwrap();
    assert_eq!(exact.position_set_state(), PositionSetState::NonEmpty);
    assert_eq!(exact.coverage().imported_position_count(), 4);
    let unknown = issue(&exact, DataHealthIssueCode::UnknownAccountingClassification);
    assert_eq!(unknown.record_count(), 2);
    assert_eq!(unknown.ratio_basis_points(), 5_000);
    assert_eq!(
        unknown
            .affected_position_ids()
            .iter()
            .map(Ulid::as_str)
            .collect::<Vec<_>>(),
        vec![id('A').as_str(), id('C').as_str()]
    );
    assert!(
        exact
            .issues()
            .iter()
            .all(|value| value.code() != DataHealthIssueCode::StalePositionSnapshot)
    );

    let one_nanosecond_over =
        evaluate_position_snapshot(&snapshot, &profile, &market_time_nanos(3_600, 1)).unwrap();
    assert_eq!(
        issue(
            &one_nanosecond_over,
            DataHealthIssueCode::StalePositionSnapshot
        )
        .observed_age_seconds(),
        3_601,
        "full MarketTime precision decides staleness and display seconds round upward"
    );

    let stale = evaluate_position_snapshot(&snapshot, &profile, &market_time(3_601)).unwrap();
    assert_eq!(
        issue(&stale, DataHealthIssueCode::StalePositionSnapshot).observed_age_seconds(),
        3_601
    );
    assert!(evaluate_position_snapshot(&snapshot, &profile, &market_time(59)).is_err());
}

#[test]
fn report_binds_optional_price_evidence_profile_fingerprint_hash_and_lineage() {
    let snapshot = snapshot_with_positions(
        'S',
        0,
        vec![position('A', AccountingClassificationState::Classified)],
    );
    let base_profile = profile(10_000, 5_000, 100, 10_000);
    let evaluation =
        evaluate_position_snapshot(&snapshot, &base_profile, &market_time(200)).unwrap();
    let evidence = price_evidence(None, 10);
    let report = DataHealthReport::new(DataHealthReportInput {
        position_snapshot: snapshot.clone(),
        evaluated_at: market_time(200),
        position_evaluation: evaluation.clone(),
        threshold_profile: base_profile.clone(),
        price_evidence: Some(evidence.clone()),
    })
    .unwrap();
    let repeated = DataHealthReport::new(DataHealthReportInput {
        position_snapshot: snapshot.clone(),
        evaluated_at: market_time(200),
        position_evaluation: evaluation,
        threshold_profile: base_profile.clone(),
        price_evidence: Some(evidence),
    })
    .unwrap();

    assert_eq!(report, repeated);
    assert_eq!(report.state(), DataHealthState::Warning);
    assert!(report.price_evidence_evaluated());
    assert_eq!(report.data_snapshot_id(), Some(&id('D')));
    assert_eq!(
        report.data_source_ref(),
        Some(&VersionRef::new(id('R'), version()))
    );
    assert!(
        report
            .issues()
            .iter()
            .any(|value| value.code() == DataHealthIssueCode::UntypedPriceSource)
    );
    assert!(
        report
            .issues()
            .iter()
            .any(|value| value.code() == DataHealthIssueCode::StaleDataSnapshot)
    );
    assert!(report.lineage().iter().any(|reference| {
        reference.object_id() == base_profile.profile_ref().id()
            && reference.version() == Some(base_profile.profile_ref().version())
            && reference.content_hash() == Some(base_profile.content_hash())
    }));

    let drifted_same_version = profile(10_000, 5_001, 100, 10_000);
    let base_evaluation =
        evaluate_position_snapshot(&snapshot, &base_profile, &market_time(200)).unwrap();
    assert!(
        DataHealthReport::new(DataHealthReportInput {
            position_snapshot: snapshot.clone(),
            evaluated_at: market_time(200),
            position_evaluation: base_evaluation,
            threshold_profile: drifted_same_version,
            price_evidence: None,
        })
        .is_err()
    );

    let changed_profile = profile_with_version(2, 10_000, 5_001, 100, 10_000);
    let changed_evaluation =
        evaluate_position_snapshot(&snapshot, &changed_profile, &market_time(200)).unwrap();
    let changed = DataHealthReport::new(DataHealthReportInput {
        position_snapshot: snapshot,
        evaluated_at: market_time(200),
        position_evaluation: changed_evaluation,
        threshold_profile: changed_profile,
        price_evidence: Some(price_evidence(Some(PriceSourceType::ModelValuation), 10)),
    })
    .unwrap();
    assert_ne!(report.request_fingerprint(), changed.request_fingerprint());
    assert_ne!(report.content_hash(), changed.content_hash());
    assert_ne!(report.lineage(), changed.lineage());
}

#[test]
fn report_rejects_an_evaluation_from_a_different_snapshot() {
    let profile = profile(3_600, 5_000, 7_200, 5_000);
    let left = snapshot_with_positions(
        'S',
        0,
        vec![position('A', AccountingClassificationState::Classified)],
    );
    let right = snapshot_with_positions(
        'Q',
        0,
        vec![position('B', AccountingClassificationState::Classified)],
    );
    let evaluation = evaluate_position_snapshot(&left, &profile, &market_time(100)).unwrap();

    assert!(
        DataHealthReport::new(DataHealthReportInput {
            position_snapshot: right,
            evaluated_at: market_time(100),
            position_evaluation: evaluation,
            threshold_profile: profile,
            price_evidence: None,
        })
        .is_err()
    );
}

fn empty_snapshot() -> PositionSnapshot {
    snapshot_with_positions('S', 0, Vec::new())
}

fn snapshot_with_positions(
    snapshot_suffix: char,
    observed_seconds: i64,
    positions: Vec<Position>,
) -> PositionSnapshot {
    let mut input = PositionSnapshotInput {
        snapshot_id: id(snapshot_suffix),
        owner: OwnerRef::new(id('T'), id('O')),
        subject_ref: VersionRef::new(id('U'), version()),
        observed_at: market_time(observed_seconds),
        visible_at: market_time(60),
        content_hash: ContentHash::digest(b"placeholder"),
        lineage: vec![LineageRef::content_addressed(
            id('L'),
            ContentHash::digest(b"source"),
        )],
        positions,
    };
    input.content_hash = PositionSnapshot::content_hash_for(&input);
    PositionSnapshot::new(input).unwrap()
}

fn position(suffix: char, state: AccountingClassificationState) -> Position {
    Position::new(PositionInput {
        position_id: id(suffix),
        instrument_ref: VersionRef::new(id('I'), version()),
        quantity: decimal("1", unit('N')),
        economic_value: decimal("100", unit('C')),
        economic_pnl: decimal("0", unit('C')),
        accounting_pnl: decimal("0", unit('C')),
        capital_requirement: decimal("10", unit('C')),
        accounting_classification: AccountingClassification::new(
            state,
            (state == AccountingClassificationState::Classified).then_some(AccountingBook::Ac),
        )
        .unwrap(),
        holding_form: PositionHoldingForm::Owned,
    })
    .unwrap()
}

fn price_evidence(
    source_type: Option<PriceSourceType>,
    as_of_seconds: i64,
) -> DataHealthPriceEvidence {
    DataHealthPriceEvidence::new(DataHealthPriceEvidenceInput {
        data_snapshot_id: id('D'),
        owner: OwnerRef::new(id('T'), id('O')),
        data_snapshot_content_hash: ContentHash::digest(b"data"),
        data_snapshot_manifest_hash: ContentHash::digest(b"manifest"),
        data_source_ref: VersionRef::new(id('R'), version()),
        source_type,
        record_count: 4,
        visible_at: market_time(20),
        as_of: market_time(as_of_seconds),
        lineage: vec![LineageRef::content_addressed(
            id('E'),
            ContentHash::digest(b"price-source"),
        )],
    })
    .unwrap()
}

fn issue(
    evaluation: &PositionHealthEvaluation,
    code: DataHealthIssueCode,
) -> &ficant_domain::research::DataHealthIssue {
    evaluation
        .issues()
        .iter()
        .find(|issue| issue.code() == code)
        .unwrap()
}

fn decimal(value: &str, unit: UnitRef) -> DecimalValue {
    DecimalValue::new(value, 0, unit).unwrap()
}

fn unit(suffix: char) -> UnitRef {
    UnitRef::new(id(suffix), version())
}

fn profile(
    max_position_age: u64,
    unknown_bps: u32,
    max_data_age: u64,
    model_bps: u32,
) -> DataHealthThresholdProfile {
    profile_with_version(1, max_position_age, unknown_bps, max_data_age, model_bps)
}

fn profile_with_version(
    profile_version: u64,
    max_position_age: u64,
    unknown_bps: u32,
    max_data_age: u64,
    model_bps: u32,
) -> DataHealthThresholdProfile {
    let mut input = profile_input(max_position_age, unknown_bps, max_data_age, model_bps);
    input.profile_ref = VersionRef::new(id('P'), Version::new(profile_version).unwrap());
    input.content_hash = DataHealthThresholdProfile::content_hash_for(&input);
    DataHealthThresholdProfile::new(input).unwrap()
}

fn profile_input(
    max_position_age: u64,
    unknown_bps: u32,
    max_data_age: u64,
    model_bps: u32,
) -> DataHealthThresholdProfileInput {
    DataHealthThresholdProfileInput {
        profile_snapshot_id: id('H'),
        owner: OwnerRef::new(id('T'), id('O')),
        profile_ref: VersionRef::new(id('P'), version()),
        visible_at: market_time(0),
        effective_from: market_time(0),
        effective_to: market_time(10_000),
        max_position_snapshot_age_seconds: max_position_age,
        unknown_accounting_warning_basis_points: unknown_bps,
        max_data_snapshot_age_seconds: max_data_age,
        model_valuation_warning_basis_points: model_bps,
        content_hash: ContentHash::digest(b"placeholder"),
        lineage: Vec::new(),
    }
}

fn market_time(seconds: i64) -> MarketTime {
    let instant = Utc.timestamp_opt(1_767_225_600 + seconds, 0).unwrap();
    MarketTime::new(instant, "UTC", instant.date_naive()).unwrap()
}

fn market_time_nanos(seconds: i64, nanos: u32) -> MarketTime {
    let instant = Utc.timestamp_opt(1_767_225_600 + seconds, nanos).unwrap();
    MarketTime::new(instant, "UTC", instant.date_naive()).unwrap()
}

fn version() -> Version {
    Version::new(1).unwrap()
}

fn id(suffix: char) -> Ulid {
    let suffix = match suffix {
        'I' => '1',
        'L' => '2',
        'O' => '3',
        'U' => '4',
        value => value,
    };
    Ulid::new(format!("{PREFIX}{suffix}")).unwrap()
}
