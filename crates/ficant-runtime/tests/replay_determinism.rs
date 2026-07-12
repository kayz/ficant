use ficant_domain::primitives::{ContentHash, MarketTime, Ulid};
use ficant_domain::research::{JournalEventType, RunJournal, RunJournalInput, RunState};
use ficant_domain::{ContentAddressed, DomainErrorCode};
use ficant_runtime::{RuntimeError, replay};

const ULID_PREFIX: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA";

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("{ULID_PREFIX}{suffix}")).unwrap()
}

fn time(hour: u32) -> MarketTime {
    MarketTime::new(
        format!("2026-03-02T{hour:02}:00:00Z").parse().unwrap(),
        "Asia/Shanghai",
        "2026-03-02".parse().unwrap(),
    )
    .unwrap()
}

fn event(
    event_suffix: char,
    run_suffix: char,
    sequence: u64,
    event_type: JournalEventType,
    hour: u32,
    prev_hash: Option<ContentHash>,
) -> RunJournal {
    let input = RunJournalInput {
        journal_event_id: id(event_suffix),
        run_id: id(run_suffix),
        sequence,
        event_type,
        occurred_at: time(hour),
        payload_type: "ficant.research.v1.Event".to_owned(),
        payload_schema: "v1".to_owned(),
        payload: vec![event_suffix as u8],
        prev_hash,
    };
    let claimed = input.canonical_hash().unwrap();
    RunJournal::new(input, &claimed).unwrap()
}

fn successful_run(run_suffix: char) -> Vec<RunJournal> {
    let created = event('B', run_suffix, 1, JournalEventType::RunCreated, 1, None);
    let started = event(
        'C',
        run_suffix,
        2,
        JournalEventType::RunStarted,
        2,
        Some(created.content_hash().clone()),
    );
    let succeeded = event(
        'D',
        run_suffix,
        3,
        JournalEventType::RunSucceeded,
        3,
        Some(started.content_hash().clone()),
    );
    vec![created, started, succeeded]
}

fn append_field(bytes: &mut Vec<u8>, tag: u8, value: &[u8]) {
    bytes.push(tag);
    bytes.extend_from_slice(&u64::try_from(value.len()).unwrap().to_be_bytes());
    bytes.extend_from_slice(value);
}

fn expected_replay_digest(events: &[RunJournal], state_code: u8) -> ContentHash {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"FRPL");
    bytes.extend_from_slice(&1_u16.to_be_bytes());
    append_field(&mut bytes, 1, events[0].run_id().as_str().as_bytes());
    append_field(
        &mut bytes,
        2,
        &u64::try_from(events.len()).unwrap().to_be_bytes(),
    );
    for event in events {
        append_field(&mut bytes, 3, event.content_hash().as_bytes());
    }
    append_field(&mut bytes, 4, &[state_code]);
    ContentHash::digest(&bytes)
}

#[test]
fn q2_inv_08_identical_ordered_facts_replay_to_same_digest_and_terminal_state() {
    let first_events = successful_run('A');
    let second_events = successful_run('A');

    let first = replay(&first_events).unwrap();
    let second = replay(&second_events).unwrap();

    assert_eq!(first.terminal_state(), RunState::Succeeded);
    assert_eq!(first.event_count(), 3);
    assert_eq!(first.digest(), second.digest());
    assert_eq!(first.digest(), &expected_replay_digest(&first_events, 3));
}

#[test]
fn q2_inv_08_replay_rejects_gap_without_sorting_or_repair() {
    let created = event('B', 'A', 1, JournalEventType::RunCreated, 1, None);
    let gap = event(
        'D',
        'A',
        3,
        JournalEventType::RunSucceeded,
        3,
        Some(created.content_hash().clone()),
    );

    assert_eq!(
        replay(&[created, gap]).unwrap_err(),
        RuntimeError::Domain(DomainErrorCode::JournalSequenceConflict)
    );
}

#[test]
fn q2_inv_08_replay_rejects_reordered_events_without_sorting() {
    let mut events = successful_run('A');
    events.swap(0, 1);

    assert_eq!(
        replay(&events).unwrap_err(),
        RuntimeError::Domain(DomainErrorCode::JournalSequenceConflict)
    );
}

#[test]
fn q2_inv_08_replay_rejects_cross_run_link() {
    let created = event('B', 'A', 1, JournalEventType::RunCreated, 1, None);
    let other_run = event(
        'C',
        'Z',
        2,
        JournalEventType::RunStarted,
        2,
        Some(created.content_hash().clone()),
    );

    assert_eq!(
        replay(&[created, other_run]).unwrap_err(),
        RuntimeError::RunIdentityConflict
    );
}

#[test]
fn q2_inv_08_replay_rejects_invalid_transition() {
    let created = event('B', 'A', 1, JournalEventType::RunCreated, 1, None);
    let succeeded = event(
        'C',
        'A',
        2,
        JournalEventType::RunSucceeded,
        2,
        Some(created.content_hash().clone()),
    );

    assert_eq!(
        replay(&[created, succeeded]).unwrap_err(),
        RuntimeError::Domain(DomainErrorCode::InvalidStateTransition)
    );
}

#[test]
fn q2_inv_08_replay_rejects_event_after_terminal() {
    let mut events = successful_run('A');
    let terminal_hash = events.last().unwrap().content_hash().clone();
    events.push(event(
        'E',
        'A',
        4,
        JournalEventType::ArtifactPublished,
        4,
        Some(terminal_hash),
    ));

    assert_eq!(
        replay(&events).unwrap_err(),
        RuntimeError::Domain(DomainErrorCode::InvalidStateTransition)
    );
}

#[test]
fn q2_inv_08_replay_rejects_created_prefix_without_terminal_proof() {
    let created = event('B', 'A', 1, JournalEventType::RunCreated, 1, None);

    assert_eq!(
        replay(&[created]).unwrap_err(),
        RuntimeError::Domain(DomainErrorCode::InvalidStateTransition)
    );
}

#[test]
fn q2_inv_08_replay_rejects_running_prefix_without_terminal_proof() {
    let created = event('B', 'A', 1, JournalEventType::RunCreated, 1, None);
    let started = event(
        'C',
        'A',
        2,
        JournalEventType::RunStarted,
        2,
        Some(created.content_hash().clone()),
    );

    assert_eq!(
        replay(&[created, started]).unwrap_err(),
        RuntimeError::Domain(DomainErrorCode::InvalidStateTransition)
    );
}
