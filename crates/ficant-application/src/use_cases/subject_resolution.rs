use ficant_domain::primitives::VersionRef;
use ficant_domain::subject::SubjectVersion;

use crate::ApplicationError;
use crate::ports::{ApplicationResult, SubjectRepository};

/// Resolves one exact Subject version and verifies its market/tool access before computation.
pub struct ResolveSubject<'a> {
    subjects: &'a dyn SubjectRepository,
}

impl<'a> ResolveSubject<'a> {
    #[must_use]
    pub const fn new(subjects: &'a dyn SubjectRepository) -> Self {
        Self { subjects }
    }

    /// Reads exactly `reference` and proves it grants both requested access codes.
    ///
    /// # Errors
    ///
    /// Returns the one client-safe Subject-binding validation failure for every missing,
    /// mismatched, or unauthorized Subject state, before the numerical engine is reached.
    pub async fn execute(
        &self,
        reference: &VersionRef,
        market_code: &str,
        tool_code: &str,
    ) -> ApplicationResult<SubjectVersion> {
        let record = self
            .subjects
            .get_subject(reference.clone())
            .await?
            .ok_or_else(ApplicationError::subject_binding_invalid)?;
        if record.subject().id() != reference.id() || record.version().reference() != reference {
            return Err(ApplicationError::subject_binding_invalid());
        }
        let access = record.version().access_set();
        if access
            .market_codes()
            .binary_search_by(|candidate| candidate.as_str().cmp(market_code))
            .is_err()
            || access
                .tool_codes()
                .binary_search_by(|candidate| candidate.as_str().cmp(tool_code))
                .is_err()
        {
            return Err(ApplicationError::subject_binding_invalid());
        }
        Ok(record.version().clone())
    }
}
