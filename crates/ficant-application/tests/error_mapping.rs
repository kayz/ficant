use ficant_application::{
    AccessScope, AeadCursorCodec, ApplicationErrorCategory, Cursor, CursorKey, IdempotencyKey,
    PageRequest, map_domain_error, map_runtime_error, replay_collected_journal,
};
use ficant_domain::ContentAddressed;
use ficant_domain::DomainErrorCode;
use ficant_domain::primitives::{ContentHash, MarketTime, Ulid};
use ficant_domain::research::{JournalEventType, RunJournal, RunJournalInput, RunState};
use ficant_runtime::RuntimeError;

#[test]
fn all_nine_domain_codes_map_exhaustively_to_safe_application_categories() {
    let cases = [
        (
            DomainErrorCode::InvalidId,
            ApplicationErrorCategory::ValidationFailed,
            false,
        ),
        (
            DomainErrorCode::InvalidUnit,
            ApplicationErrorCategory::ValidationFailed,
            false,
        ),
        (
            DomainErrorCode::InvalidEffectiveTime,
            ApplicationErrorCategory::ValidationFailed,
            false,
        ),
        (
            DomainErrorCode::VersionConflict,
            ApplicationErrorCategory::VersionConflict,
            true,
        ),
        (
            DomainErrorCode::ContentHashMismatch,
            ApplicationErrorCategory::HashMismatch,
            false,
        ),
        (
            DomainErrorCode::BrokenLineage,
            ApplicationErrorCategory::LineageIncomplete,
            false,
        ),
        (
            DomainErrorCode::InvalidStateTransition,
            ApplicationErrorCategory::StateConflict,
            false,
        ),
        (
            DomainErrorCode::JournalSequenceConflict,
            ApplicationErrorCategory::ConcurrencyConflict,
            true,
        ),
        (
            DomainErrorCode::InvalidValue,
            ApplicationErrorCategory::ValidationFailed,
            false,
        ),
    ];

    for (code, expected_category, expected_retryable) in cases {
        let mapped = map_domain_error(code);
        assert_eq!(mapped.category(), expected_category);
        assert_eq!(mapped.retryable(), expected_retryable);
    }
}

#[test]
fn runtime_conflicts_map_without_exposing_internal_details() {
    let idempotency = map_runtime_error(&RuntimeError::IdempotencyConflict);
    assert_eq!(
        idempotency.category(),
        ApplicationErrorCategory::AlreadyExists
    );
    assert!(!idempotency.retryable());

    let concurrency = map_runtime_error(&RuntimeError::ConcurrencyConflict {
        expected: 1,
        actual: 2,
    });
    assert_eq!(
        concurrency.category(),
        ApplicationErrorCategory::ConcurrencyConflict
    );
    assert!(concurrency.retryable());

    let run_identity = map_runtime_error(&RuntimeError::RunIdentityConflict);
    assert_eq!(
        run_identity.category(),
        ApplicationErrorCategory::LineageIncomplete
    );
    assert!(!run_identity.retryable());
}

#[test]
fn application_owned_port_values_reject_blank_keys_and_invalid_page_limits() {
    let codec = AeadCursorCodec::new(CursorKey::new("test", [7; 32]).unwrap(), Vec::new()).unwrap();
    assert_eq!(
        IdempotencyKey::new(" ").unwrap_err().category(),
        ApplicationErrorCategory::ValidationFailed
    );
    assert_eq!(
        Cursor::issue(&codec, &scope(), "").unwrap_err().category(),
        ApplicationErrorCategory::ValidationFailed
    );
    assert_eq!(
        PageRequest::new(scope(), None, 0).unwrap_err().category(),
        ApplicationErrorCategory::ValidationFailed
    );
}

fn scope() -> AccessScope {
    AccessScope::new(id('T'), id('X'), vec![id('Y')]).unwrap()
}

#[test]
fn phase1_use_case_replays_collected_real_domain_events() {
    let run_id = id('A');
    let created = event(
        'B',
        run_id.clone(),
        1,
        JournalEventType::RunCreated,
        1,
        None,
    );
    let started = event(
        'C',
        run_id.clone(),
        2,
        JournalEventType::RunStarted,
        2,
        Some(created.content_hash().clone()),
    );
    let succeeded = event(
        'D',
        run_id.clone(),
        3,
        JournalEventType::RunSucceeded,
        3,
        Some(started.content_hash().clone()),
    );

    let result = replay_collected_journal(&run_id, &[created, started, succeeded]).unwrap();
    assert_eq!(result.run_id(), &run_id);
    assert_eq!(result.terminal_state(), RunState::Succeeded);
    assert_eq!(result.event_count(), 3);
}

const ULID_PREFIX: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA";

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("{ULID_PREFIX}{suffix}")).unwrap()
}

fn time(hour: u32) -> MarketTime {
    MarketTime::new(
        format!("2026-03-03T{hour:02}:00:00Z").parse().unwrap(),
        "Asia/Shanghai",
        "2026-03-03".parse().unwrap(),
    )
    .unwrap()
}

fn event(
    event_suffix: char,
    run_id: Ulid,
    sequence: u64,
    event_type: JournalEventType,
    hour: u32,
    prev_hash: Option<ContentHash>,
) -> RunJournal {
    let input = RunJournalInput {
        journal_event_id: id(event_suffix),
        run_id,
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
