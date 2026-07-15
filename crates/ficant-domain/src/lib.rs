//! Pure Phase 1 domain model.
//!
//! This crate contains validated values and immutable domain objects only. It
//! deliberately has no transport, persistence, filesystem, network, async
//! runtime, or container dependencies.

#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::struct_field_names
)]

pub mod analytics;
pub mod market;
pub mod primitives;
pub mod research;

use thiserror::Error;

use crate::primitives::{ContentHash, LineageRef};

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum DomainErrorCode {
    #[error("invalid identifier")]
    InvalidId,
    #[error("invalid unit")]
    InvalidUnit,
    #[error("invalid effective time")]
    InvalidEffectiveTime,
    #[error("version conflict")]
    VersionConflict,
    #[error("content hash mismatch")]
    ContentHashMismatch,
    #[error("broken lineage")]
    BrokenLineage,
    #[error("invalid state transition")]
    InvalidStateTransition,
    #[error("journal sequence conflict")]
    JournalSequenceConflict,
    #[error("invalid domain value")]
    InvalidValue,
}

pub type DomainResult<T> = Result<T, DomainErrorCode>;

pub trait VersionedDefinition {
    fn identity(&self) -> &str;
    fn version(&self) -> u64;
}

pub trait ContentAddressed {
    fn content_hash(&self) -> &ContentHash;

    fn verify_content(&self, bytes: &[u8]) -> DomainResult<()> {
        self.content_hash().verify(bytes)
    }
}

pub trait Lineaged {
    fn lineage(&self) -> &[LineageRef];
}
