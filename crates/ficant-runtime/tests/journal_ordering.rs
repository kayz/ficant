use std::sync::{Arc, Barrier};
use std::thread;

use ficant_domain::primitives::{ContentHash, MarketTime, Ulid};
use ficant_domain::research::{JournalEventType, RunJournalInput};
use ficant_domain::{ContentAddressed, DomainErrorCode};
use ficant_runtime::{
    IdempotencyKey, JournalAppend, PerRunJournal, RuntimeError, SharedRunJournal,
};

const ULID_PREFIX: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA";

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("{ULID_PREFIX}{suffix}")).unwrap()
}

fn time(hour: u32) -> MarketTime {
    MarketTime::new(
        format!("2026-03-01T{hour:02}:00:00Z").parse().unwrap(),
        "Asia/Shanghai",
        "2026-03-01".parse().unwrap(),
    )
    .unwrap()
}

fn input(
    event_suffix: char,
    run_suffix: char,
    sequence: u64,
    event_type: JournalEventType,
    hour: u32,
    payload: u8,
    prev_hash: Option<ContentHash>,
) -> RunJournalInput {
    RunJournalInput {
        journal_event_id: id(event_suffix),
        run_id: id(run_suffix),
        sequence,
        event_type,
        occurred_at: time(hour),
        payload_type: "ficant.research.v1.Event".to_owned(),
        payload_schema: "v1".to_owned(),
        payload: vec![payload],
        prev_hash,
    }
}

fn append(input: RunJournalInput) -> JournalAppend {
    let claimed_hash = input.canonical_hash().unwrap();
    JournalAppend::new(input, claimed_hash)
}

fn key(value: &str) -> IdempotencyKey {
    IdempotencyKey::new(value).unwrap()
}

#[test]
fn q2_inv_08_first_sequence_must_be_one_and_append_must_be_continuous() {
    let mut journal = PerRunJournal::new(id('A'));
    let invalid = append(input(
        'B',
        'A',
        2,
        JournalEventType::RunCreated,
        1,
        1,
        Some(ContentHash::digest(b"imaginary previous")),
    ));

    assert_eq!(
        journal.append(key("first"), 1, invalid).unwrap_err(),
        RuntimeError::Domain(DomainErrorCode::JournalSequenceConflict)
    );
    assert_eq!(journal.len(), 0);
}

#[test]
fn q2_inv_08_identical_idempotent_retry_returns_original_without_duplication() {
    let mut journal = PerRunJournal::new(id('A'));
    let command = append(input('B', 'A', 1, JournalEventType::RunCreated, 1, 1, None));
    let first = journal.append(key("same-key"), 1, command.clone()).unwrap();
    let retry = journal.append(key("same-key"), 1, command).unwrap();

    assert!(first.inserted());
    assert!(!retry.inserted());
    assert_eq!(first.event().id(), retry.event().id());
    assert_eq!(first.event().content_hash(), retry.event().content_hash());
    assert_eq!(journal.len(), 1);
}

#[test]
fn q2_inv_08_same_idempotency_key_with_changed_event_is_a_stable_conflict() {
    let mut journal = PerRunJournal::new(id('A'));
    let first = append(input('B', 'A', 1, JournalEventType::RunCreated, 1, 1, None));
    journal.append(key("same-key"), 1, first).unwrap();
    let changed = append(input('C', 'A', 1, JournalEventType::RunCreated, 1, 2, None));

    assert_eq!(
        journal.append(key("same-key"), 1, changed).unwrap_err(),
        RuntimeError::IdempotencyConflict
    );
    assert_eq!(journal.len(), 1);
}

#[test]
fn q2_inv_08_real_concurrent_same_sequence_attempts_have_one_winner() {
    let shared = SharedRunJournal::new(id('A'));
    let barrier = Arc::new(Barrier::new(3));
    let mut handles = Vec::new();

    for (event_suffix, payload) in [('B', 1_u8), ('C', 2_u8)] {
        let worker = shared.clone();
        let start = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            let command = append(input(
                event_suffix,
                'A',
                1,
                JournalEventType::RunCreated,
                1,
                payload,
                None,
            ));
            start.wait();
            worker.append(key(&format!("worker-{payload}")), 1, command)
        }));
    }

    barrier.wait();
    let results: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    let successes = results.iter().filter(|result| result.is_ok()).count();
    let conflicts = results
        .iter()
        .filter(|result| matches!(result, Err(RuntimeError::ConcurrencyConflict { .. })))
        .count();

    assert_eq!(successes, 1);
    assert_eq!(conflicts, 1);
    assert_eq!(shared.len().unwrap(), 1);
}

#[test]
fn q2_inv_08_claimed_self_hash_is_revalidated() {
    let mut journal = PerRunJournal::new(id('A'));
    let forged = JournalAppend::new(
        input('B', 'A', 1, JournalEventType::RunCreated, 1, 1, None),
        ContentHash::digest(b"forged"),
    );

    assert_eq!(
        journal.append(key("forged"), 1, forged).unwrap_err(),
        RuntimeError::Domain(DomainErrorCode::ContentHashMismatch)
    );
}

#[test]
fn q2_inv_08_previous_hash_chain_is_revalidated() {
    let mut journal = PerRunJournal::new(id('A'));
    let first = journal
        .append(
            key("first"),
            1,
            append(input('B', 'A', 1, JournalEventType::RunCreated, 1, 1, None)),
        )
        .unwrap();
    let second = append(input(
        'C',
        'A',
        2,
        JournalEventType::RunStarted,
        2,
        2,
        Some(ContentHash::digest(b"wrong previous")),
    ));

    assert_eq!(
        journal.append(key("second"), 2, second).unwrap_err(),
        RuntimeError::Domain(DomainErrorCode::ContentHashMismatch)
    );
    assert_eq!(
        journal.events()[0].content_hash(),
        first.event().content_hash()
    );
}

#[test]
fn q2_inv_08_cross_run_event_is_rejected() {
    let mut journal = PerRunJournal::new(id('A'));
    let other_run = append(input('B', 'Z', 1, JournalEventType::RunCreated, 1, 1, None));

    assert_eq!(
        journal.append(key("cross-run"), 1, other_run).unwrap_err(),
        RuntimeError::RunIdentityConflict
    );
    assert_eq!(journal.len(), 0);
}
