use chrono::{NaiveDate, TimeZone, Utc};

use ficant_domain::market::{
    FactSource, Quote, QuoteInput, Trade, TradeInput, Unit, UnitInput, Valuation, ValuationInput,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, EffectivePeriod, LineageRef, MarketTime, Ulid, UnitRef, Version,
    VersionRef,
};
use ficant_domain::research::{
    ExperimentRun, ExperimentRunInput, JournalEventType, RunJournal, RunJournalInput, RunState,
};
use ficant_domain::{ContentAddressed, DomainErrorCode, primitives::OwnerRef};

const ULID_PREFIX: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA";

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("{ULID_PREFIX}{suffix}")).unwrap()
}

fn owner() -> OwnerRef {
    OwnerRef::new(id('A'), id('B'))
}

fn version(value: u64) -> Version {
    Version::new(value).unwrap()
}

fn hash(seed: u8) -> ContentHash {
    ContentHash::from_bytes(&[seed; 32]).unwrap()
}

fn time(day: u32, hour: u32) -> MarketTime {
    let instant = Utc
        .with_ymd_and_hms(2026, 2, day, hour, 0, 0)
        .single()
        .unwrap();
    MarketTime::new(
        instant,
        "Asia/Shanghai",
        instant
            .with_timezone(&chrono_tz::Asia::Shanghai)
            .date_naive(),
    )
    .unwrap()
}

fn journal_input(
    suffix: char,
    sequence: u64,
    event_type: JournalEventType,
    occurred_at: MarketTime,
    payload: Vec<u8>,
    prev_hash: Option<ContentHash>,
) -> RunJournalInput {
    RunJournalInput {
        journal_event_id: id(suffix),
        run_id: id('E'),
        sequence,
        event_type,
        occurred_at,
        payload_type: "run.event".to_owned(),
        payload_schema: "v1".to_owned(),
        payload,
        prev_hash,
    }
}

fn create_journal(input: RunJournalInput) -> RunJournal {
    let claimed_hash = input.canonical_hash().unwrap();
    RunJournal::new(input, &claimed_hash).unwrap()
}

#[test]
fn domain_error_code_is_the_exact_stable_nine_variant_set() {
    let codes = [
        DomainErrorCode::InvalidId,
        DomainErrorCode::InvalidUnit,
        DomainErrorCode::InvalidEffectiveTime,
        DomainErrorCode::VersionConflict,
        DomainErrorCode::ContentHashMismatch,
        DomainErrorCode::BrokenLineage,
        DomainErrorCode::InvalidStateTransition,
        DomainErrorCode::JournalSequenceConflict,
        DomainErrorCode::InvalidValue,
    ];
    let stable_numbers = codes.map(|code| match code {
        DomainErrorCode::InvalidId => 1,
        DomainErrorCode::InvalidUnit => 2,
        DomainErrorCode::InvalidEffectiveTime => 3,
        DomainErrorCode::VersionConflict => 4,
        DomainErrorCode::ContentHashMismatch => 5,
        DomainErrorCode::BrokenLineage => 6,
        DomainErrorCode::InvalidStateTransition => 7,
        DomainErrorCode::JournalSequenceConflict => 8,
        DomainErrorCode::InvalidValue => 9,
    });
    assert_eq!(stable_numbers, [1, 2, 3, 4, 5, 6, 7, 8, 9]);
}

#[test]
fn ordinary_domain_values_use_invalid_value() {
    assert_eq!(
        FactSource::new("", "external-1", 1).unwrap_err(),
        DomainErrorCode::InvalidValue
    );
    assert_eq!(
        DecimalValue::new("1.25", 2, UnitRef::new(id('D'), version(1))).unwrap_err(),
        DomainErrorCode::InvalidValue
    );
    let unit = UnitRef::new(id('D'), version(1));
    let amount = |coefficient| DecimalValue::new(coefficient, 0, unit.clone()).unwrap();
    let instrument = VersionRef::new(id('F'), version(1));
    let source = || FactSource::new("feed", "external-1", 1).unwrap();
    assert_eq!(
        Trade::new(TradeInput {
            trade_id: id('G'),
            instrument: instrument.clone(),
            owner: owner(),
            source: source(),
            executed_at: time(1, 1),
            price: amount("100"),
            quantity: amount("0"),
            supersedes_id: None,
        })
        .unwrap_err(),
        DomainErrorCode::InvalidValue
    );
    assert_eq!(
        Valuation::new(ValuationInput {
            valuation_id: id('H'),
            instrument: instrument.clone(),
            owner: owner(),
            source: source(),
            valuation_at: time(1, 1),
            method: "external".to_owned(),
            rule_pack: VersionRef::new(id('J'), version(1)),
            values: vec![],
            supersedes_id: None,
        })
        .unwrap_err(),
        DomainErrorCode::InvalidValue
    );
    assert_eq!(
        Quote::new(QuoteInput {
            quote_id: id('K'),
            instrument,
            owner: owner(),
            source: source(),
            observed_at: time(1, 1),
            received_at: time(1, 2),
            bid: Some(amount("101")),
            ask: Some(amount("100")),
            supersedes_id: None,
        })
        .unwrap_err(),
        DomainErrorCode::InvalidValue
    );
}

#[test]
fn specialized_error_codes_take_precedence_over_invalid_value() {
    assert_eq!(
        Ulid::new("invalid").unwrap_err(),
        DomainErrorCode::InvalidId
    );
    assert_eq!(
        Unit::new(UnitInput {
            unit_id: id('C'),
            version: version(1),
            owner: owner(),
            code: "cny".to_owned(),
            dimension: "currency".to_owned(),
            scale: 2,
            precision: 18,
        })
        .unwrap_err(),
        DomainErrorCode::InvalidUnit
    );
    let instant = Utc.with_ymd_and_hms(2026, 2, 1, 1, 0, 0).single().unwrap();
    assert_eq!(
        MarketTime::new(
            instant,
            "Asia/Shanghai",
            NaiveDate::from_ymd_opt(2026, 2, 2).unwrap(),
        )
        .unwrap_err(),
        DomainErrorCode::InvalidEffectiveTime
    );
    assert_eq!(
        Version::new(0).unwrap_err(),
        DomainErrorCode::VersionConflict
    );
    assert_eq!(
        ContentHash::from_bytes(&[0; 31]).unwrap_err(),
        DomainErrorCode::ContentHashMismatch
    );
    assert_eq!(
        LineageRef::new(id('C'), None, None).unwrap_err(),
        DomainErrorCode::BrokenLineage
    );

    let run = ExperimentRun::new(ExperimentRunInput {
        experiment_run_id: id('F'),
        owner: owner(),
        data_snapshot: LineageRef::content_addressed(id('G'), hash(1)),
        universe_snapshot: LineageRef::content_addressed(id('H'), hash(2)),
        rule_packs: vec![ficant_domain::primitives::VersionRef::new(
            id('J'),
            version(1),
        )],
        runtime_image_digest: hash(3),
        parameters_hash: hash(4),
        seed: 42,
    })
    .unwrap();
    assert_eq!(
        run.transition(RunState::Succeeded, 1).unwrap_err(),
        DomainErrorCode::InvalidStateTransition
    );

    let invalid_journal = journal_input(
        'K',
        0,
        JournalEventType::RunCreated,
        time(1, 1),
        vec![1],
        None,
    );
    assert_eq!(
        invalid_journal.canonical_hash().unwrap_err(),
        DomainErrorCode::JournalSequenceConflict
    );
}

#[test]
fn journal_canonical_schema_is_versioned_and_byte_stable() {
    let input = RunJournalInput {
        journal_event_id: id('S'),
        run_id: id('E'),
        sequence: 1,
        event_type: JournalEventType::RunCreated,
        occurred_at: time(1, 1),
        payload_type: "run.created".to_owned(),
        payload_schema: "v1".to_owned(),
        payload: vec![1, 2, 3],
        prev_hash: None,
    };
    let expected = ContentHash::from_bytes(&[
        106, 239, 182, 43, 245, 115, 79, 104, 135, 60, 160, 83, 99, 47, 26, 164, 251, 43, 173, 30,
        40, 86, 176, 195, 3, 172, 143, 74, 203, 75, 31, 49,
    ])
    .unwrap();
    assert_eq!(input.canonical_hash().unwrap(), expected);
}

#[test]
fn primitive_errors_are_stable_and_specific() {
    assert_eq!(
        Ulid::new("not-a-ulid").unwrap_err(),
        DomainErrorCode::InvalidId
    );
    assert_eq!(
        Version::new(0).unwrap_err(),
        DomainErrorCode::VersionConflict
    );
    assert_eq!(
        ContentHash::from_bytes(&[0; 31]).unwrap_err(),
        DomainErrorCode::ContentHashMismatch
    );
    assert_eq!(
        LineageRef::new(id('C'), None, None).unwrap_err(),
        DomainErrorCode::BrokenLineage
    );
}

#[test]
fn decimal_is_canonical_and_never_accepts_float_input() {
    let unit = UnitRef::new(id('D'), version(1));
    let value = DecimalValue::new("+0001000", 3, unit.clone()).unwrap();
    assert_eq!(value.coefficient(), "1");
    assert_eq!(value.scale(), 0);
    assert_eq!(value.unit(), &unit);

    assert_eq!(
        DecimalValue::new("1.25", 2, unit).unwrap_err(),
        DomainErrorCode::InvalidValue
    );
}

#[test]
fn invalid_timezone_local_date_and_period_are_rejected() {
    let instant = Utc.with_ymd_and_hms(2026, 2, 1, 1, 0, 0).single().unwrap();
    assert_eq!(
        MarketTime::new(
            instant,
            "Invalid/Timezone",
            NaiveDate::from_ymd_opt(2026, 2, 1).unwrap(),
        )
        .unwrap_err(),
        DomainErrorCode::InvalidEffectiveTime
    );
    assert_eq!(
        EffectivePeriod::new(time(2, 1), time(1, 1)).unwrap_err(),
        DomainErrorCode::InvalidEffectiveTime
    );
}

#[test]
fn run_state_transitions_are_revision_checked_and_terminal() {
    let run = ExperimentRun::new(ExperimentRunInput {
        experiment_run_id: id('E'),
        owner: owner(),
        data_snapshot: LineageRef::content_addressed(id('F'), hash(1)),
        universe_snapshot: LineageRef::content_addressed(id('G'), hash(2)),
        rule_packs: vec![ficant_domain::primitives::VersionRef::new(
            id('H'),
            version(1),
        )],
        runtime_image_digest: hash(3),
        parameters_hash: hash(4),
        seed: 42,
    })
    .unwrap();
    assert_eq!(
        run.transition(RunState::Running, 2).unwrap_err(),
        DomainErrorCode::VersionConflict
    );
    let running = run.transition(RunState::Running, 1).unwrap();
    let succeeded = running.transition(RunState::Succeeded, 2).unwrap();
    assert_eq!(
        succeeded.transition(RunState::Running, 3).unwrap_err(),
        DomainErrorCode::InvalidStateTransition
    );
}

#[test]
fn journal_rejects_sequence_and_previous_hash_breaks() {
    let first = create_journal(journal_input(
        'J',
        1,
        JournalEventType::RunCreated,
        time(1, 1),
        vec![1],
        None,
    ));
    let wrong_sequence = create_journal(journal_input(
        'K',
        3,
        JournalEventType::RunStarted,
        time(1, 2),
        vec![2],
        Some(first.content_hash().clone()),
    ));
    assert_eq!(
        wrong_sequence.validate_after(&first).unwrap_err(),
        DomainErrorCode::JournalSequenceConflict
    );

    let wrong_hash = create_journal(journal_input(
        'M',
        2,
        JournalEventType::RunStarted,
        time(1, 2),
        vec![2],
        Some(hash(9)),
    ));
    assert_eq!(
        wrong_hash.validate_after(&first).unwrap_err(),
        DomainErrorCode::ContentHashMismatch
    );
}

#[test]
fn forged_self_consistent_journal_chain_is_rejected() {
    let forged_first_hash = hash(41);
    let first = journal_input(
        'N',
        1,
        JournalEventType::RunCreated,
        time(1, 1),
        vec![1, 2, 3],
        None,
    );
    let second = journal_input(
        'P',
        2,
        JournalEventType::RunStarted,
        time(1, 2),
        vec![4, 5, 6],
        Some(forged_first_hash.clone()),
    );

    assert_eq!(
        RunJournal::new(first, &forged_first_hash).unwrap_err(),
        DomainErrorCode::ContentHashMismatch
    );
    assert_eq!(
        RunJournal::new(second, &hash(42)).unwrap_err(),
        DomainErrorCode::ContentHashMismatch
    );
}

#[test]
fn journal_payload_mutation_retaining_claimed_hash_is_rejected() {
    let original = journal_input(
        'Q',
        1,
        JournalEventType::RunCreated,
        time(1, 1),
        vec![1, 2, 3],
        None,
    );
    let claimed_hash = original.canonical_hash().unwrap();
    let mutated = journal_input(
        'Q',
        1,
        JournalEventType::RunCreated,
        time(1, 1),
        vec![1, 2, 4],
        None,
    );

    assert_eq!(
        RunJournal::new(mutated, &claimed_hash).unwrap_err(),
        DomainErrorCode::ContentHashMismatch
    );
}
