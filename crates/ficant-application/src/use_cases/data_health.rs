use ficant_domain::primitives::{ContentHash, LineageRef, MarketTime, Ulid, VersionRef};
use ficant_domain::research::{
    DataHealthPriceEvidence, DataHealthPriceEvidenceInput, DataHealthReport, DataHealthReportInput,
    DataHealthThresholdProfile, evaluate_position_snapshot,
};
use ficant_domain::{ContentAddressed, Lineaged, VersionedDefinition};

use crate::ports::{
    AccessScope, ApplicationResult, CanonicalSnapshotDecoder, DataSourceRepository,
    IntegrityEventSink, PositionSnapshotRepository, SafeTraceContext,
    SnapshotVerifiedReadMetadataRepository, VerifiedBlobReader,
};
use crate::use_cases::verified_reads::{VerifiedSnapshotRead, VerifiedSnapshotReader};
use crate::{ApplicationError, ApplicationErrorCategory, map_domain_error};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataHealthQuery {
    subject_ref: VersionRef,
    position_snapshot_id: Ulid,
    data_snapshot_id: Option<Ulid>,
    evaluated_at: MarketTime,
    threshold_profile: DataHealthThresholdProfile,
}

impl DataHealthQuery {
    #[must_use]
    pub fn new(
        subject_ref: VersionRef,
        position_snapshot_id: Ulid,
        data_snapshot_id: Option<Ulid>,
        evaluated_at: MarketTime,
        threshold_profile: DataHealthThresholdProfile,
    ) -> Self {
        Self {
            subject_ref,
            position_snapshot_id,
            data_snapshot_id,
            evaluated_at,
            threshold_profile,
        }
    }

    #[must_use]
    pub fn subject_ref(&self) -> &VersionRef {
        &self.subject_ref
    }

    #[must_use]
    pub fn position_snapshot_id(&self) -> &Ulid {
        &self.position_snapshot_id
    }

    #[must_use]
    pub fn data_snapshot_id(&self) -> Option<&Ulid> {
        self.data_snapshot_id.as_ref()
    }

    #[must_use]
    pub fn evaluated_at(&self) -> &MarketTime {
        &self.evaluated_at
    }

    #[must_use]
    pub fn threshold_profile(&self) -> &DataHealthThresholdProfile {
        &self.threshold_profile
    }
}

/// Stateless, read-only data-health orchestration.
///
/// Its dependency set deliberately contains no calculation use case, engine, cache, clock,
/// journal, lease, or mutable repository.
pub struct GetDataHealthReport<'a> {
    positions: &'a dyn PositionSnapshotRepository,
    snapshot_metadata: &'a dyn SnapshotVerifiedReadMetadataRepository,
    blob_reader: &'a dyn VerifiedBlobReader,
    integrity_events: &'a dyn IntegrityEventSink,
    decoder: &'a dyn CanonicalSnapshotDecoder,
    data_sources: &'a dyn DataSourceRepository,
}

impl<'a> GetDataHealthReport<'a> {
    #[must_use]
    pub const fn new(
        positions: &'a dyn PositionSnapshotRepository,
        snapshot_metadata: &'a dyn SnapshotVerifiedReadMetadataRepository,
        blob_reader: &'a dyn VerifiedBlobReader,
        integrity_events: &'a dyn IntegrityEventSink,
        decoder: &'a dyn CanonicalSnapshotDecoder,
        data_sources: &'a dyn DataSourceRepository,
    ) -> Self {
        Self {
            positions,
            snapshot_metadata,
            blob_reader,
            integrity_events,
            decoder,
            data_sources,
        }
    }

    /// Reads the exact immutable inputs, derives one health evaluation, and returns its report.
    ///
    /// Integrity, identity, owner, version, and time drift remain fail-closed. Only health facts
    /// whose inputs remain trustworthy are represented as warnings.
    ///
    /// # Errors
    ///
    /// Returns a stable application error when any referenced object is missing, unauthorized,
    /// not yet visible, content-inconsistent, or cannot be decoded through the verified-read path.
    pub async fn execute(
        &self,
        scope: &AccessScope,
        query: DataHealthQuery,
    ) -> ApplicationResult<DataHealthReport> {
        let snapshot = self
            .positions
            .get_position_snapshot(
                scope,
                query.position_snapshot_id.clone(),
                query.evaluated_at.clone(),
            )
            .await?
            .ok_or_else(not_found)?;
        scope.authorize(snapshot.owner())?;
        if snapshot.id() != &query.position_snapshot_id
            || snapshot.subject_ref() != &query.subject_ref
            || snapshot.visible_at().instant() > query.evaluated_at.instant()
        {
            return Err(lineage());
        }
        let evaluation =
            evaluate_position_snapshot(&snapshot, &query.threshold_profile, &query.evaluated_at)
                .map_err(map_domain_error)?;
        let price_evidence = match query.data_snapshot_id.as_ref() {
            Some(data_snapshot_id) => Some(
                self.read_price_evidence(scope, data_snapshot_id.clone(), &query)
                    .await?,
            ),
            None => None,
        };
        DataHealthReport::new(DataHealthReportInput {
            position_snapshot: snapshot,
            evaluated_at: query.evaluated_at,
            position_evaluation: evaluation,
            threshold_profile: query.threshold_profile,
            price_evidence,
        })
        .map_err(map_domain_error)
    }

    async fn read_price_evidence(
        &self,
        scope: &AccessScope,
        data_snapshot_id: Ulid,
        query: &DataHealthQuery,
    ) -> ApplicationResult<DataHealthPriceEvidence> {
        let reader = VerifiedSnapshotReader::new(
            self.snapshot_metadata,
            self.blob_reader,
            self.integrity_events,
        );
        let read = reader
            .read(scope, data_snapshot_id.clone(), trace_for(query)?)
            .await?;
        let VerifiedSnapshotRead::Data {
            snapshot,
            parquet,
            manifest,
        } = read
        else {
            return Err(lineage());
        };
        scope.authorize(snapshot.owner())?;
        if snapshot.id() != &data_snapshot_id
            || snapshot.visible_at().instant() > query.evaluated_at.instant()
            || snapshot.as_of().instant() > query.evaluated_at.instant()
        {
            return Err(lineage());
        }
        let decoded = self
            .decoder
            .decode_quotes(&snapshot, parquet.bytes(), manifest.bytes())
            .await?;
        let source_ref = decoded.data_source().clone();
        let source = self
            .data_sources
            .get_exact(scope, source_ref.clone())
            .await?
            .ok_or_else(not_found)?;
        scope.authorize(source.owner())?;
        if source.owner() != snapshot.owner()
            || source.identity() != source_ref.id().as_str()
            || source.version() != source_ref.version().get()
        {
            return Err(lineage());
        }
        let record_count = u64::try_from(decoded.quotes().len()).map_err(|_| validation())?;
        let mut lineage = snapshot.lineage().to_vec();
        lineage.push(LineageRef::versioned(
            source_ref.id().clone(),
            source_ref.version(),
        ));
        DataHealthPriceEvidence::new(DataHealthPriceEvidenceInput {
            data_snapshot_id,
            owner: snapshot.owner().clone(),
            data_snapshot_content_hash: snapshot.content_hash().clone(),
            data_snapshot_manifest_hash: snapshot.manifest_hash().clone(),
            data_source_ref: source_ref,
            source_type: source.price_source_type(),
            record_count,
            visible_at: snapshot.visible_at().clone(),
            as_of: snapshot.as_of().clone(),
            lineage,
        })
        .map_err(map_domain_error)
    }
}

fn trace_for(query: &DataHealthQuery) -> ApplicationResult<SafeTraceContext> {
    let mut bytes = Vec::new();
    append(&mut bytes, query.subject_ref.id().as_str().as_bytes());
    append(&mut bytes, &query.subject_ref.version().get().to_be_bytes());
    append(&mut bytes, query.position_snapshot_id.as_str().as_bytes());
    if let Some(data_snapshot_id) = query.data_snapshot_id.as_ref() {
        append(&mut bytes, data_snapshot_id.as_str().as_bytes());
    }
    append(
        &mut bytes,
        &query.evaluated_at.instant().timestamp().to_be_bytes(),
    );
    append(
        &mut bytes,
        &query
            .evaluated_at
            .instant()
            .timestamp_subsec_nanos()
            .to_be_bytes(),
    );
    append(
        &mut bytes,
        query.threshold_profile.content_hash().as_bytes(),
    );
    let digest = ContentHash::digest(&bytes);
    let token =
        digest.as_bytes()[..16]
            .iter()
            .fold(String::with_capacity(32), |mut output, byte| {
                use std::fmt::Write as _;
                write!(output, "{byte:02x}").expect("writing to String cannot fail");
                output
            });
    SafeTraceContext::new(token)
}

fn append(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
    bytes.extend_from_slice(value);
}

fn validation() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}

fn lineage() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::LineageIncomplete, false)
}

fn not_found() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::NotFound, false)
}
