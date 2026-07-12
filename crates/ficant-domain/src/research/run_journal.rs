use chrono::Datelike;

use crate::primitives::{ContentHash, MarketTime, Ulid};
use crate::{ContentAddressed, DomainErrorCode, DomainResult};

const CANONICAL_MAGIC: &[u8; 4] = b"FJRN";
const CANONICAL_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JournalEventType {
    RunCreated,
    RunStarted,
    RunSucceeded,
    RunFailed,
    RunCancelled,
    ArtifactPublished,
    SignalSetPublished,
}

impl JournalEventType {
    const fn canonical_code(self) -> u8 {
        match self {
            Self::RunCreated => 1,
            Self::RunStarted => 2,
            Self::RunSucceeded => 3,
            Self::RunFailed => 4,
            Self::RunCancelled => 5,
            Self::ArtifactPublished => 6,
            Self::SignalSetPublished => 7,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunJournalInput {
    pub journal_event_id: Ulid,
    pub run_id: Ulid,
    pub sequence: u64,
    pub event_type: JournalEventType,
    pub occurred_at: MarketTime,
    pub payload_type: String,
    pub payload_schema: String,
    pub payload: Vec<u8>,
    pub prev_hash: Option<ContentHash>,
}

impl RunJournalInput {
    pub fn canonical_hash(&self) -> DomainResult<ContentHash> {
        self.validate()?;
        Ok(ContentHash::digest(&self.canonical_bytes()))
    }

    fn validate(&self) -> DomainResult<()> {
        if self.sequence == 0
            || (self.sequence == 1 && self.prev_hash.is_some())
            || (self.sequence > 1 && self.prev_hash.is_none())
        {
            return Err(DomainErrorCode::JournalSequenceConflict);
        }
        if self.payload_type.trim().is_empty()
            || self.payload_type != self.payload_type.trim()
            || self.payload_schema.trim().is_empty()
            || self.payload_schema != self.payload_schema.trim()
            || self.payload.is_empty()
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(())
    }

    fn canonical_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(256 + self.payload.len());
        bytes.extend_from_slice(CANONICAL_MAGIC);
        bytes.extend_from_slice(&CANONICAL_VERSION.to_be_bytes());
        append_field(&mut bytes, 1, self.journal_event_id.as_str().as_bytes());
        append_field(&mut bytes, 2, self.run_id.as_str().as_bytes());
        append_field(&mut bytes, 3, &self.sequence.to_be_bytes());
        append_field(&mut bytes, 4, &[self.event_type.canonical_code()]);
        append_field(
            &mut bytes,
            5,
            &self.occurred_at.instant().timestamp().to_be_bytes(),
        );
        append_field(
            &mut bytes,
            6,
            &self
                .occurred_at
                .instant()
                .timestamp_subsec_nanos()
                .to_be_bytes(),
        );
        append_field(&mut bytes, 7, self.occurred_at.market_timezone().as_bytes());
        let local_date = self.occurred_at.local_trading_date();
        let mut date_bytes = [0_u8; 6];
        date_bytes[..4].copy_from_slice(&local_date.year().to_be_bytes());
        date_bytes[4] = u8::try_from(local_date.month()).expect("month is in 1..=12");
        date_bytes[5] = u8::try_from(local_date.day()).expect("day is in 1..=31");
        append_field(&mut bytes, 8, &date_bytes);
        append_field(&mut bytes, 9, self.payload_type.as_bytes());
        append_field(&mut bytes, 10, self.payload_schema.as_bytes());
        append_field(&mut bytes, 11, &self.payload);

        let mut previous = Vec::with_capacity(33);
        match &self.prev_hash {
            Some(hash) => {
                previous.push(1);
                previous.extend_from_slice(hash.as_bytes());
            }
            None => previous.push(0),
        }
        append_field(&mut bytes, 12, &previous);
        bytes
    }
}

fn append_field(output: &mut Vec<u8>, tag: u8, value: &[u8]) {
    output.push(tag);
    let length = u64::try_from(value.len()).expect("field length must fit in u64");
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(value);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunJournal {
    journal_event_id: Ulid,
    run_id: Ulid,
    sequence: u64,
    event_type: JournalEventType,
    occurred_at: MarketTime,
    payload_type: String,
    payload_schema: String,
    payload: Vec<u8>,
    prev_hash: Option<ContentHash>,
    event_hash: ContentHash,
}

impl RunJournal {
    pub fn new(input: RunJournalInput, claimed_hash: &ContentHash) -> DomainResult<Self> {
        let canonical_hash = input.canonical_hash()?;
        if &canonical_hash != claimed_hash {
            return Err(DomainErrorCode::ContentHashMismatch);
        }
        Ok(Self {
            journal_event_id: input.journal_event_id,
            run_id: input.run_id,
            sequence: input.sequence,
            event_type: input.event_type,
            occurred_at: input.occurred_at,
            payload_type: input.payload_type,
            payload_schema: input.payload_schema,
            payload: input.payload,
            prev_hash: input.prev_hash,
            event_hash: canonical_hash,
        })
    }

    pub fn validate_after(&self, previous: &Self) -> DomainResult<()> {
        self.verify_canonical()?;
        previous.verify_canonical()?;
        if self.run_id != previous.run_id || previous.sequence.checked_add(1) != Some(self.sequence)
        {
            return Err(DomainErrorCode::JournalSequenceConflict);
        }
        if self.prev_hash.as_ref() != Some(&previous.event_hash) {
            return Err(DomainErrorCode::ContentHashMismatch);
        }
        if self.occurred_at.instant() < previous.occurred_at.instant() {
            return Err(DomainErrorCode::InvalidEffectiveTime);
        }
        Ok(())
    }

    fn verify_canonical(&self) -> DomainResult<()> {
        let input = RunJournalInput {
            journal_event_id: self.journal_event_id.clone(),
            run_id: self.run_id.clone(),
            sequence: self.sequence,
            event_type: self.event_type,
            occurred_at: self.occurred_at.clone(),
            payload_type: self.payload_type.clone(),
            payload_schema: self.payload_schema.clone(),
            payload: self.payload.clone(),
            prev_hash: self.prev_hash.clone(),
        };
        if input.canonical_hash()? != self.event_hash {
            return Err(DomainErrorCode::ContentHashMismatch);
        }
        Ok(())
    }

    pub fn id(&self) -> &Ulid {
        &self.journal_event_id
    }

    pub fn run_id(&self) -> &Ulid {
        &self.run_id
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn event_type(&self) -> JournalEventType {
        self.event_type
    }

    pub fn occurred_at(&self) -> &MarketTime {
        &self.occurred_at
    }

    pub fn payload_type(&self) -> &str {
        &self.payload_type
    }

    pub fn payload_schema(&self) -> &str {
        &self.payload_schema
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    pub fn prev_hash(&self) -> Option<&ContentHash> {
        self.prev_hash.as_ref()
    }
}

impl ContentAddressed for RunJournal {
    fn content_hash(&self) -> &ContentHash {
        &self.event_hash
    }
}
