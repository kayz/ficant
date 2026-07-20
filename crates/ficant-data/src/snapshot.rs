use std::collections::BTreeSet;
use std::sync::Arc;

use arrow::array::{RecordBatch, StringArray, TimestampMicrosecondArray, UInt64Array};
use bytes::Bytes;
use chrono::SecondsFormat;
use ficant_domain::primitives::{ContentHash, LineageRef, Ulid, Version, VersionRef};
use ficant_domain::research::{DataSnapshot, DataSnapshotInput};
use ficant_domain::{ContentAddressed, Lineaged, VersionedDefinition};
use parquet::arrow::ArrowWriter;
use parquet::arrow::arrow_reader::{ArrowReaderOptions, ParquetRecordBatchReaderBuilder};
use parquet::basic::Compression;
use parquet::file::properties::{EnabledStatistics, WriterProperties, WriterVersion};
use serde::{Deserialize, Serialize};

use crate::{
    CANONICAL_QUOTE_SCHEMA_ID, CanonicalIngestRequest, CanonicalQuoteBatch, DataError, DataResult,
    canonical_quote_schema, canonical_quote_schema_hash,
};

pub const SNAPSHOT_MANIFEST_SCHEMA_ID: &str = "ficant.data.snapshot-manifest.v1";
pub const PARQUET_CREATED_BY: &str = "ficant-parquet/59.1.0";
const PARQUET_LIBRARY_VERSION: &str = "59.1.0";
const PARQUET_WRITE_BATCH_SIZE: usize = 1_024;
const PARQUET_DATA_PAGE_ROW_LIMIT: usize = 20_000;

#[derive(Clone, Debug)]
pub struct CanonicalSnapshotPackage {
    snapshot: DataSnapshot,
    parquet: Vec<u8>,
    manifest: Vec<u8>,
}

impl CanonicalSnapshotPackage {
    #[must_use]
    pub fn snapshot(&self) -> &DataSnapshot {
        &self.snapshot
    }

    #[must_use]
    pub fn parquet(&self) -> &[u8] {
        &self.parquet
    }

    #[must_use]
    pub fn manifest(&self) -> &[u8] {
        &self.manifest
    }

    #[must_use]
    pub fn into_parts(self) -> (DataSnapshot, Vec<u8>, Vec<u8>) {
        (self.snapshot, self.parquet, self.manifest)
    }
}

#[derive(Clone, Debug)]
pub struct VerifiedCanonicalSnapshot {
    snapshot: DataSnapshot,
    manifest: SnapshotManifest,
    batch: RecordBatch,
}

impl VerifiedCanonicalSnapshot {
    #[must_use]
    pub fn snapshot(&self) -> &DataSnapshot {
        &self.snapshot
    }

    #[must_use]
    pub fn manifest(&self) -> &SnapshotManifest {
        &self.manifest
    }

    #[must_use]
    pub fn batch(&self) -> &RecordBatch {
        &self.batch
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotManifest {
    manifest_schema: String,
    snapshot_id: String,
    tenant_id: String,
    owner_id: String,
    canonical_schema_id: String,
    canonical_schema_hash: String,
    parquet: ParquetPayload,
    point_in_time: PointInTimeBinding,
    data_source: ExactVersion,
    instrument_mapping_digest: String,
    calendar: ExactVersion,
    unit: ExactVersion,
    instruments: Vec<ExactVersion>,
    quality: QualityBinding,
    writer: ParquetWriterBinding,
}

impl SnapshotManifest {
    #[must_use]
    pub fn row_count(&self) -> u64 {
        self.parquet.row_count
    }

    #[must_use]
    pub fn data_source_id(&self) -> &str {
        &self.data_source.id
    }

    #[must_use]
    pub fn instrument_mapping_digest(&self) -> &str {
        &self.instrument_mapping_digest
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParquetPayload {
    content_hash: String,
    size: u64,
    row_count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PointInTimeBinding {
    as_of: String,
    visible_at_cutoff: String,
    market_timezone: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExactVersion {
    id: String,
    version: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct QualityBinding {
    rule_set_id: String,
    validated_rows: u64,
    emitted_rows: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParquetWriterBinding {
    library: String,
    version: String,
    created_by: String,
    compression: String,
    dictionary: bool,
    writer_version: String,
    data_page_version: String,
    row_groups: u64,
    write_batch_size: u64,
    data_page_row_limit: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CanonicalSnapshotCodec;

impl CanonicalSnapshotCodec {
    /// Encodes a validated Canonical Quote batch and binds every byte to a domain snapshot.
    ///
    /// # Errors
    ///
    /// Returns a data error when schema, owner, source, unit, row count, or encoding drifts.
    pub fn build(
        &self,
        snapshot_id: Ulid,
        request: &CanonicalIngestRequest,
        canonical: &CanonicalQuoteBatch,
    ) -> DataResult<CanonicalSnapshotPackage> {
        validate_canonical_batch(request, canonical.batch())?;
        let parquet = encode_parquet(canonical.batch())?;
        let parquet_hash = ContentHash::digest(&parquet);
        let instruments = batch_instruments(canonical.batch())?;
        let lineage = snapshot_lineage(request, &instruments)?;
        let manifest = SnapshotManifest {
            manifest_schema: SNAPSHOT_MANIFEST_SCHEMA_ID.to_owned(),
            snapshot_id: snapshot_id.as_str().to_owned(),
            tenant_id: request.source().owner().tenant_id().as_str().to_owned(),
            owner_id: request.source().owner().owner_id().as_str().to_owned(),
            canonical_schema_id: CANONICAL_QUOTE_SCHEMA_ID.to_owned(),
            canonical_schema_hash: hash_hex(canonical.schema_hash()),
            parquet: ParquetPayload {
                content_hash: hash_hex(&parquet_hash),
                size: u64::try_from(parquet.len())
                    .map_err(|_| DataError::SnapshotIntegrityFailed)?,
                row_count: u64::try_from(canonical.batch().num_rows())
                    .map_err(|_| DataError::SnapshotIntegrityFailed)?,
            },
            point_in_time: PointInTimeBinding {
                as_of: timestamp(request.window().as_of().instant()),
                visible_at_cutoff: timestamp(request.window().visible_at_cutoff().instant()),
                market_timezone: request.window().as_of().market_timezone().to_owned(),
            },
            data_source: exact_version(request.source().id(), request.source().version()),
            instrument_mapping_digest: hash_hex(&request.mapping().contract_hash()),
            calendar: ExactVersion {
                id: request.calendar().identity().to_owned(),
                version: request.calendar().version(),
            },
            unit: ExactVersion {
                id: request.unit().identity().to_owned(),
                version: request.unit().version(),
            },
            instruments: instruments.iter().map(exact_version_ref).collect(),
            quality: QualityBinding {
                rule_set_id: canonical.quality().rule_set_id().to_owned(),
                validated_rows: u64::try_from(canonical.quality().validated_rows())
                    .map_err(|_| DataError::SnapshotIntegrityFailed)?,
                emitted_rows: u64::try_from(canonical.quality().emitted_rows())
                    .map_err(|_| DataError::SnapshotIntegrityFailed)?,
            },
            writer: writer_binding(),
        };
        let manifest_bytes = canonical_manifest_bytes(&manifest)?;
        let snapshot = DataSnapshot::new(DataSnapshotInput {
            data_snapshot_id: snapshot_id,
            owner: request.source().owner().clone(),
            visible_at: request.window().visible_at_cutoff().clone(),
            as_of: request.window().as_of().clone(),
            schema_hash: canonical.schema_hash().clone(),
            manifest_hash: ContentHash::digest(&manifest_bytes),
            blob_content_hash: parquet_hash,
            lineage,
        })
        .map_err(|_| DataError::SnapshotIntegrityFailed)?;
        Ok(CanonicalSnapshotPackage {
            snapshot,
            parquet,
            manifest: manifest_bytes,
        })
    }

    /// Verifies a required-read snapshot and decodes its only experiment-facing batch.
    ///
    /// # Errors
    ///
    /// Returns a data error without a partial batch for any three-way binding or codec drift.
    pub fn decode_verified(
        &self,
        snapshot: DataSnapshot,
        parquet: &[u8],
        manifest_bytes: &[u8],
    ) -> DataResult<VerifiedCanonicalSnapshot> {
        snapshot
            .content_hash()
            .verify(parquet)
            .map_err(|_| DataError::SnapshotIntegrityFailed)?;
        snapshot
            .manifest_hash()
            .verify(manifest_bytes)
            .map_err(|_| DataError::SnapshotIntegrityFailed)?;
        let manifest: SnapshotManifest = serde_json::from_slice(manifest_bytes)
            .map_err(|_| DataError::SnapshotIntegrityFailed)?;
        if canonical_manifest_bytes(&manifest)? != manifest_bytes {
            return Err(DataError::SnapshotIntegrityFailed);
        }
        validate_manifest(&snapshot, parquet, &manifest)?;
        let batch = decode_parquet(parquet, manifest.parquet.row_count)?;
        validate_decoded_batch(&snapshot, &manifest, &batch)?;
        Ok(VerifiedCanonicalSnapshot {
            snapshot,
            manifest,
            batch,
        })
    }
}

fn encode_parquet(batch: &RecordBatch) -> DataResult<Vec<u8>> {
    let properties = WriterProperties::builder()
        .set_writer_version(WriterVersion::PARQUET_2_0)
        .set_created_by(PARQUET_CREATED_BY.to_owned())
        .set_compression(Compression::UNCOMPRESSED)
        .set_dictionary_enabled(false)
        .set_statistics_enabled(EnabledStatistics::None)
        .set_offset_index_disabled(true)
        .set_max_row_group_row_count(None)
        .set_max_row_group_bytes(None)
        .set_write_batch_size(PARQUET_WRITE_BATCH_SIZE)
        .set_data_page_row_count_limit(PARQUET_DATA_PAGE_ROW_LIMIT)
        .build();
    let mut bytes = Vec::new();
    let mut writer = ArrowWriter::try_new(&mut bytes, batch.schema(), Some(properties))
        .map_err(|_| DataError::SnapshotIntegrityFailed)?;
    writer
        .write(batch)
        .map_err(|_| DataError::SnapshotIntegrityFailed)?;
    writer
        .close()
        .map_err(|_| DataError::SnapshotIntegrityFailed)?;
    Ok(bytes)
}

fn decode_parquet(bytes: &[u8], row_count: u64) -> DataResult<RecordBatch> {
    let builder = ParquetRecordBatchReaderBuilder::try_new_with_options(
        Bytes::copy_from_slice(bytes),
        ArrowReaderOptions::new().with_skip_arrow_metadata(false),
    )
    .map_err(|_| DataError::SnapshotIntegrityFailed)?;
    let metadata = builder.metadata();
    if builder.schema().as_ref() != &canonical_quote_schema()
        || metadata.num_row_groups() != 1
        || metadata.file_metadata().version() != 2
        || metadata.file_metadata().created_by() != Some(PARQUET_CREATED_BY)
        || metadata.row_group(0).num_rows() != i64::try_from(row_count).unwrap_or(-1)
        || metadata
            .row_group(0)
            .columns()
            .iter()
            .any(|column| column.compression() != Compression::UNCOMPRESSED)
    {
        return Err(DataError::SnapshotIntegrityFailed);
    }
    let batch_size = usize::try_from(row_count).map_err(|_| DataError::SnapshotIntegrityFailed)?;
    let mut reader = builder
        .with_batch_size(batch_size.max(1))
        .build()
        .map_err(|_| DataError::SnapshotIntegrityFailed)?;
    let decoded = reader
        .next()
        .transpose()
        .map_err(|_| DataError::SnapshotIntegrityFailed)?
        .ok_or(DataError::SnapshotIntegrityFailed)?;
    if reader.next().is_some() {
        return Err(DataError::SnapshotIntegrityFailed);
    }
    let canonical_schema = canonical_quote_schema();
    if decoded.schema().fields() != canonical_schema.fields() {
        return Err(DataError::SchemaMismatch);
    }
    RecordBatch::try_new(Arc::new(canonical_schema), decoded.columns().to_vec())
        .map_err(|_| DataError::SchemaMismatch)
}

fn validate_canonical_batch(
    request: &CanonicalIngestRequest,
    batch: &RecordBatch,
) -> DataResult<()> {
    if batch.num_rows() == 0 || batch.schema().as_ref() != &canonical_quote_schema() {
        return Err(DataError::SchemaMismatch);
    }
    let tenant = string_column(batch, 0)?;
    let owner = string_column(batch, 1)?;
    let source_id = string_column(batch, 2)?;
    let source_version = u64_column(batch, 3)?;
    let unit_id = string_column(batch, 14)?;
    let unit_version = u64_column(batch, 15)?;
    for row in 0..batch.num_rows() {
        if tenant.value(row) != request.source().owner().tenant_id().as_str()
            || owner.value(row) != request.source().owner().owner_id().as_str()
            || source_id.value(row) != request.source().id().as_str()
            || source_version.value(row) != request.source().version()
            || unit_id.value(row) != request.unit().identity()
            || unit_version.value(row) != request.unit().version()
        {
            return Err(DataError::SnapshotIntegrityFailed);
        }
    }
    Ok(())
}

fn validate_manifest(
    snapshot: &DataSnapshot,
    parquet: &[u8],
    manifest: &SnapshotManifest,
) -> DataResult<()> {
    let instruments = manifest_instruments(manifest)?;
    let expected_lineage = manifest_lineage(manifest, &instruments)?;
    if manifest.manifest_schema != SNAPSHOT_MANIFEST_SCHEMA_ID
        || manifest.snapshot_id != snapshot.id().as_str()
        || manifest.tenant_id != snapshot.owner().tenant_id().as_str()
        || manifest.owner_id != snapshot.owner().owner_id().as_str()
        || manifest.canonical_schema_id != CANONICAL_QUOTE_SCHEMA_ID
        || manifest.canonical_schema_hash != hash_hex(snapshot.schema_hash())
        || manifest.canonical_schema_hash != hash_hex(&canonical_quote_schema_hash())
        || manifest.parquet.content_hash != hash_hex(snapshot.content_hash())
        || manifest.parquet.size != u64::try_from(parquet.len()).unwrap_or(u64::MAX)
        || manifest.parquet.row_count == 0
        || manifest.point_in_time.as_of != timestamp(snapshot.as_of().instant())
        || manifest.point_in_time.visible_at_cutoff != timestamp(snapshot.visible_at().instant())
        || manifest.point_in_time.market_timezone != snapshot.as_of().market_timezone()
        || snapshot.visible_at().market_timezone() != snapshot.as_of().market_timezone()
        || manifest.quality.emitted_rows != manifest.parquet.row_count
        || manifest.quality.validated_rows < manifest.quality.emitted_rows
        || manifest.quality.rule_set_id != "ficant.market.quote.quality.v1"
        || manifest.writer != writer_binding()
        || snapshot.lineage() != expected_lineage
    {
        return Err(DataError::SnapshotIntegrityFailed);
    }
    parse_hash(&manifest.instrument_mapping_digest)?;
    Ok(())
}

fn validate_decoded_batch(
    snapshot: &DataSnapshot,
    manifest: &SnapshotManifest,
    batch: &RecordBatch,
) -> DataResult<()> {
    if batch.schema().as_ref() != &canonical_quote_schema()
        || batch.num_rows()
            != usize::try_from(manifest.parquet.row_count)
                .map_err(|_| DataError::SnapshotIntegrityFailed)?
    {
        return Err(DataError::SchemaMismatch);
    }
    let tenant = string_column(batch, 0)?;
    let owner = string_column(batch, 1)?;
    let source_id = string_column(batch, 2)?;
    let source_version = u64_column(batch, 3)?;
    let observed = batch
        .column(7)
        .as_any()
        .downcast_ref::<TimestampMicrosecondArray>()
        .ok_or(DataError::SchemaMismatch)?;
    let visible = batch
        .column(8)
        .as_any()
        .downcast_ref::<TimestampMicrosecondArray>()
        .ok_or(DataError::SchemaMismatch)?;
    let unit_id = string_column(batch, 14)?;
    let unit_version = u64_column(batch, 15)?;
    for row in 0..batch.num_rows() {
        if tenant.value(row) != snapshot.owner().tenant_id().as_str()
            || owner.value(row) != snapshot.owner().owner_id().as_str()
            || source_id.value(row) != manifest.data_source.id
            || source_version.value(row) != manifest.data_source.version
            || unit_id.value(row) != manifest.unit.id
            || unit_version.value(row) != manifest.unit.version
            || observed.value(row) > snapshot.as_of().instant().timestamp_micros()
            || visible.value(row) > snapshot.visible_at().instant().timestamp_micros()
        {
            return Err(DataError::SnapshotIntegrityFailed);
        }
    }
    if batch_instruments(batch)? != manifest_instruments(manifest)? {
        return Err(DataError::SnapshotIntegrityFailed);
    }
    Ok(())
}

fn batch_instruments(batch: &RecordBatch) -> DataResult<Vec<VersionRef>> {
    let ids = string_column(batch, 5)?;
    let versions = u64_column(batch, 6)?;
    let mut values = BTreeSet::new();
    for row in 0..batch.num_rows() {
        values.insert((ids.value(row).to_owned(), versions.value(row)));
    }
    values
        .into_iter()
        .map(|(id, version)| {
            Ok(VersionRef::new(
                Ulid::new(id).map_err(|_| DataError::SnapshotIntegrityFailed)?,
                Version::new(version).map_err(|_| DataError::SnapshotIntegrityFailed)?,
            ))
        })
        .collect()
}

fn snapshot_lineage(
    request: &CanonicalIngestRequest,
    instruments: &[VersionRef],
) -> DataResult<Vec<LineageRef>> {
    let mut lineage = vec![
        LineageRef::versioned(
            request.source().id().clone(),
            Version::new(request.source().version())
                .map_err(|_| DataError::SnapshotIntegrityFailed)?,
        ),
        LineageRef::versioned(
            Ulid::new(request.calendar().identity())
                .map_err(|_| DataError::SnapshotIntegrityFailed)?,
            Version::new(request.calendar().version())
                .map_err(|_| DataError::SnapshotIntegrityFailed)?,
        ),
        LineageRef::versioned(
            Ulid::new(request.unit().identity()).map_err(|_| DataError::SnapshotIntegrityFailed)?,
            Version::new(request.unit().version())
                .map_err(|_| DataError::SnapshotIntegrityFailed)?,
        ),
    ];
    lineage.extend(
        instruments
            .iter()
            .map(|value| LineageRef::versioned(value.id().clone(), value.version())),
    );
    Ok(lineage)
}

fn manifest_instruments(manifest: &SnapshotManifest) -> DataResult<Vec<VersionRef>> {
    let mut previous: Option<&ExactVersion> = None;
    let mut values = Vec::with_capacity(manifest.instruments.len());
    for item in &manifest.instruments {
        if previous.is_some_and(|value| value >= item) {
            return Err(DataError::SnapshotIntegrityFailed);
        }
        values.push(version_ref(item)?);
        previous = Some(item);
    }
    if values.is_empty() {
        return Err(DataError::SnapshotIntegrityFailed);
    }
    Ok(values)
}

fn manifest_lineage(
    manifest: &SnapshotManifest,
    instruments: &[VersionRef],
) -> DataResult<Vec<LineageRef>> {
    let source = version_ref(&manifest.data_source)?;
    let calendar = version_ref(&manifest.calendar)?;
    let unit = version_ref(&manifest.unit)?;
    let mut lineage = vec![
        LineageRef::versioned(source.id().clone(), source.version()),
        LineageRef::versioned(calendar.id().clone(), calendar.version()),
        LineageRef::versioned(unit.id().clone(), unit.version()),
    ];
    lineage.extend(
        instruments
            .iter()
            .map(|value| LineageRef::versioned(value.id().clone(), value.version())),
    );
    Ok(lineage)
}

fn writer_binding() -> ParquetWriterBinding {
    ParquetWriterBinding {
        library: "apache-arrow-rs-parquet".to_owned(),
        version: PARQUET_LIBRARY_VERSION.to_owned(),
        created_by: PARQUET_CREATED_BY.to_owned(),
        compression: "UNCOMPRESSED".to_owned(),
        dictionary: false,
        writer_version: "PARQUET_2_0".to_owned(),
        data_page_version: "DATA_PAGE_V2".to_owned(),
        row_groups: 1,
        write_batch_size: PARQUET_WRITE_BATCH_SIZE as u64,
        data_page_row_limit: PARQUET_DATA_PAGE_ROW_LIMIT as u64,
    }
}

fn canonical_manifest_bytes(manifest: &SnapshotManifest) -> DataResult<Vec<u8>> {
    let mut bytes = serde_json::to_vec(manifest).map_err(|_| DataError::SnapshotIntegrityFailed)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn exact_version(id: &Ulid, version: u64) -> ExactVersion {
    ExactVersion {
        id: id.as_str().to_owned(),
        version,
    }
}

fn exact_version_ref(value: &VersionRef) -> ExactVersion {
    exact_version(value.id(), value.version().get())
}

fn version_ref(value: &ExactVersion) -> DataResult<VersionRef> {
    Ok(VersionRef::new(
        Ulid::new(&value.id).map_err(|_| DataError::SnapshotIntegrityFailed)?,
        Version::new(value.version).map_err(|_| DataError::SnapshotIntegrityFailed)?,
    ))
}

fn string_column(batch: &RecordBatch, index: usize) -> DataResult<&StringArray> {
    batch
        .column(index)
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or(DataError::SchemaMismatch)
}

fn u64_column(batch: &RecordBatch, index: usize) -> DataResult<&UInt64Array> {
    batch
        .column(index)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .ok_or(DataError::SchemaMismatch)
}

fn timestamp(value: chrono::DateTime<chrono::Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Micros, true)
}

fn hash_hex(hash: &ContentHash) -> String {
    hash.as_bytes()
        .iter()
        .fold(String::with_capacity(64), |mut encoded, byte| {
            use std::fmt::Write as _;
            write!(encoded, "{byte:02x}").expect("writing to String cannot fail");
            encoded
        })
}

fn parse_hash(value: &str) -> DataResult<ContentHash> {
    if value.len() != 64 || !value.bytes().all(|value| value.is_ascii_hexdigit()) {
        return Err(DataError::SnapshotIntegrityFailed);
    }
    let mut bytes = [0_u8; 32];
    for (index, target) in bytes.iter_mut().enumerate() {
        let start = index * 2;
        *target = u8::from_str_radix(&value[start..start + 2], 16)
            .map_err(|_| DataError::SnapshotIntegrityFailed)?;
    }
    ContentHash::from_bytes(&bytes).map_err(|_| DataError::SnapshotIntegrityFailed)
}
