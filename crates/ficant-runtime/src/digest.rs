use ficant_domain::ContentAddressed;
use ficant_domain::primitives::ContentHash;
use ficant_domain::research::{RunJournal, RunState};
use sha2::{Digest, Sha256};

const CANONICAL_MAGIC: &[u8; 4] = b"FRPL";
const CANONICAL_VERSION: u16 = 1;

pub(crate) fn replay_digest(events: &[RunJournal], terminal_state: RunState) -> ContentHash {
    let mut bytes = Vec::with_capacity(64 + events.len() * 41);
    bytes.extend_from_slice(CANONICAL_MAGIC);
    bytes.extend_from_slice(&CANONICAL_VERSION.to_be_bytes());
    append_field(&mut bytes, 1, events[0].run_id().as_str().as_bytes());
    let event_count = u64::try_from(events.len()).expect("event count must fit in u64");
    append_field(&mut bytes, 2, &event_count.to_be_bytes());
    for event in events {
        append_field(&mut bytes, 3, event.content_hash().as_bytes());
    }
    append_field(&mut bytes, 4, &[state_code(terminal_state)]);
    ContentHash::from_bytes(&Sha256::digest(bytes)).expect("SHA-256 output has 32 bytes")
}

fn append_field(output: &mut Vec<u8>, tag: u8, value: &[u8]) {
    output.push(tag);
    let length = u64::try_from(value.len()).expect("field length must fit in u64");
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(value);
}

const fn state_code(state: RunState) -> u8 {
    match state {
        RunState::Created => 1,
        RunState::Running => 2,
        RunState::Succeeded => 3,
        RunState::Failed => 4,
        RunState::Cancelled => 5,
    }
}
