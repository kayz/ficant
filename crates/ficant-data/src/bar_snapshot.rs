//! Immutable Bar snapshots and point-in-time reads (ADR-0023 §5; input for AC38 replay).
//!
//! A Bar snapshot is a `DataSnapshot` whose Parquet blob holds `ficant.market.bar.canonical.v1`
//! rows and whose manifest binds the source, mapping, calendar, units, instruments, visibility
//! rule and the full rejection report. Reading verifies all three parts and rebuilds every row as
//! a domain [`Bar`], so a decoded snapshot obeys the same rules as a fresh import.

use std::collections::BTreeSet;

use arrow::array::{Array, RecordBatch, StringArray, UInt32Array, UInt64Array};
use chrono::{DateTime, Utc};
use ficant_domain::market::{
    Bar, BarFrequency, BarInput, BarPrices, BarSettlement, PriceSourceType,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, LineageRef, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::research::{DataSnapshot, DataSnapshotInput};
use ficant_domain::{ContentAddressed, Lineaged, VersionedDefinition};
use serde::{Deserialize, Serialize};

use crate::bar_canonical::price_source_from_code;
use crate::snapshot::{
    ParquetWriterBinding, decimal_column, decode_parquet_with_schema, encode_parquet, epoch_date,
    hash_hex, parse_hash, timestamp, writer_binding,
};
use crate::{
    BAR_QUALITY_RULE_SET_ID, BarRejection, BarRejectionReason, CANONICAL_BAR_SCHEMA_ID,
    CanonicalBarBatch, CanonicalBarIngestRequest, CanonicalSnapshotPackage, DataError, DataResult,
    canonical_bar_schema, canonical_bar_schema_hash,
};

pub const BAR_SNAPSHOT_MANIFEST_SCHEMA_ID: &str = "ficant.data.bar-snapshot-manifest.v1";
const MARKET_TIMEZONE: &str = "Asia/Shanghai";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExactVersion {
    id: String,
    version: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BarParquetPayload {
    content_hash: String,
    size: u64,
    row_count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BarPointInTime {
    as_of: String,
    visible_at_cutoff: String,
    market_timezone: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestRejection {
    row_id: String,
    reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    field: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BarQualityBinding {
    rule_set_id: String,
    visibility_rule: String,
    read_rows: u64,
    outside_window_rows: u64,
    emitted_rows: u64,
    rejections: Vec<ManifestRejection>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BarUnits {
    price: ExactVersion,
    amount: ExactVersion,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    yield_unit: Option<ExactVersion>,
}

/// Canonical JSON manifest of one Bar snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BarSnapshotManifest {
    manifest_schema: String,
    snapshot_id: String,
    tenant_id: String,
    owner_id: String,
    canonical_schema_id: String,
    canonical_schema_hash: String,
    parquet: BarParquetPayload,
    point_in_time: BarPointInTime,
    data_source: ExactVersion,
    instrument_mapping_id: String,
    instrument_mapping_digest: String,
    calendar: ExactVersion,
    units: BarUnits,
    instruments: Vec<ExactVersion>,
    quality: BarQualityBinding,
    writer: ParquetWriterBinding,
}

impl BarSnapshotManifest {
    pub fn row_count(&self) -> u64 {
        self.parquet.row_count
    }

    pub fn visibility_rule(&self) -> &str {
        &self.quality.visibility_rule
    }

    pub const fn outside_window_rows(&self) -> u64 {
        self.quality.outside_window_rows
    }

    /// The rejection report recorded at import time.
    pub fn rejections(&self) -> DataResult<Vec<BarRejection>> {
        self.quality
            .rejections
            .iter()
            .map(|item| {
                Ok(BarRejection {
                    row_id: item.row_id.clone(),
                    reason: BarRejectionReason::from_code(&item.reason)
                        .ok_or(DataError::SnapshotIntegrityFailed)?,
                    field: item.field.clone(),
                })
            })
            .collect()
    }
}

/// A verified Bar snapshot, decoded into domain bars in canonical order.
#[derive(Clone, Debug)]
pub struct VerifiedBarSnapshot {
    snapshot: DataSnapshot,
    manifest: BarSnapshotManifest,
    bars: Vec<Bar>,
    price_source_type: PriceSourceType,
}

impl VerifiedBarSnapshot {
    pub fn snapshot(&self) -> &DataSnapshot {
        &self.snapshot
    }

    pub fn manifest(&self) -> &BarSnapshotManifest {
        &self.manifest
    }

    pub fn bars(&self) -> &[Bar] {
        &self.bars
    }

    /// Price source type of every bar in the snapshot (I9); one snapshot has exactly one.
    pub const fn price_source_type(&self) -> PriceSourceType {
        self.price_source_type
    }

    /// Bars known at `decision_time` (I1): `visible_at ≤ decision_time`, which also implies the
    /// bar has ended. Fails closed when the decision time lies beyond what this snapshot can
    /// answer, because bars visible after its cutoff were never imported into it.
    pub fn visible_at(&self, decision_time: DateTime<Utc>) -> DataResult<Vec<&Bar>> {
        if decision_time > self.snapshot.visible_at().instant() {
            return Err(DataError::PointInTimeViolation);
        }
        Ok(self
            .bars
            .iter()
            .filter(|bar| bar.visible_at() <= decision_time)
            .collect())
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct BarSnapshotCodec;

impl BarSnapshotCodec {
    /// Encodes an ingested Bar batch and binds every byte to an immutable domain snapshot.
    pub fn build(
        &self,
        snapshot_id: Ulid,
        request: &CanonicalBarIngestRequest,
        canonical: &CanonicalBarBatch,
    ) -> DataResult<CanonicalSnapshotPackage> {
        let batch = &canonical.batch;
        if batch.num_rows() == 0
            || batch.schema().as_ref() != &canonical_bar_schema()
            || canonical.schema_hash != canonical_bar_schema_hash()
        {
            return Err(DataError::SchemaMismatch);
        }
        let units = request_units(request);
        let instruments = batch_instruments(batch)?;
        let manifest = BarSnapshotManifest {
            manifest_schema: BAR_SNAPSHOT_MANIFEST_SCHEMA_ID.to_owned(),
            snapshot_id: snapshot_id.as_str().to_owned(),
            tenant_id: request.source.owner().tenant_id().as_str().to_owned(),
            owner_id: request.source.owner().owner_id().as_str().to_owned(),
            canonical_schema_id: CANONICAL_BAR_SCHEMA_ID.to_owned(),
            canonical_schema_hash: hash_hex(&canonical_bar_schema_hash()),
            parquet: BarParquetPayload {
                content_hash: String::new(),
                size: 0,
                row_count: count(batch.num_rows())?,
            },
            point_in_time: BarPointInTime {
                as_of: timestamp(request.window.as_of().instant()),
                visible_at_cutoff: timestamp(request.window.visible_at_cutoff().instant()),
                market_timezone: request.window.as_of().market_timezone().to_owned(),
            },
            data_source: ExactVersion {
                id: request.source.id().as_str().to_owned(),
                version: request.source.version(),
            },
            instrument_mapping_id: request.mapping.id().as_str().to_owned(),
            instrument_mapping_digest: hash_hex(&request.mapping.contract_hash()),
            calendar: ExactVersion {
                id: request.calendar.identity().to_owned(),
                version: request.calendar.version(),
            },
            units,
            instruments,
            quality: BarQualityBinding {
                rule_set_id: canonical.report.rule_set_id.to_owned(),
                visibility_rule: canonical.report.visibility_rule.clone(),
                read_rows: count(canonical.report.read_rows)?,
                outside_window_rows: count(canonical.report.outside_window_rows)?,
                emitted_rows: count(canonical.report.emitted_rows)?,
                rejections: canonical
                    .report
                    .rejections
                    .iter()
                    .map(|item| ManifestRejection {
                        row_id: item.row_id.clone(),
                        reason: item.reason.code().to_owned(),
                        field: item.field.clone(),
                    })
                    .collect(),
            },
            writer: writer_binding(),
        };
        // Validate the batch exactly as a reader will before anything is published.
        decode_bars(
            &manifest,
            batch,
            request.window.as_of().instant(),
            request.window.visible_at_cutoff().instant(),
        )?;
        let parquet = encode_parquet(batch)?;
        let parquet_hash = ContentHash::digest(&parquet);
        let manifest = BarSnapshotManifest {
            parquet: BarParquetPayload {
                content_hash: hash_hex(&parquet_hash),
                size: count(parquet.len())?,
                row_count: manifest.parquet.row_count,
            },
            ..manifest
        };
        validate_quality(&manifest)?;
        let manifest_bytes = manifest_bytes(&manifest)?;
        let snapshot = DataSnapshot::new(DataSnapshotInput {
            data_snapshot_id: snapshot_id,
            owner: request.source.owner().clone(),
            visible_at: request.window.visible_at_cutoff().clone(),
            as_of: request.window.as_of().clone(),
            schema_hash: canonical_bar_schema_hash(),
            manifest_hash: ContentHash::digest(&manifest_bytes),
            blob_content_hash: parquet_hash,
            lineage: manifest_lineage(&manifest)?,
        })
        .map_err(|_| DataError::SnapshotIntegrityFailed)?;
        Ok(CanonicalSnapshotPackage::from_parts(
            snapshot,
            parquet,
            manifest_bytes,
        ))
    }

    /// Verifies snapshot, Parquet and manifest against each other and decodes the bars.
    /// Any drift fails without returning partial data.
    pub fn decode_verified(
        &self,
        snapshot: DataSnapshot,
        parquet: &[u8],
        manifest_bytes: &[u8],
    ) -> DataResult<VerifiedBarSnapshot> {
        snapshot
            .content_hash()
            .verify(parquet)
            .map_err(|_| DataError::SnapshotIntegrityFailed)?;
        snapshot
            .manifest_hash()
            .verify(manifest_bytes)
            .map_err(|_| DataError::SnapshotIntegrityFailed)?;
        let manifest: BarSnapshotManifest = serde_json::from_slice(manifest_bytes)
            .map_err(|_| DataError::SnapshotIntegrityFailed)?;
        if self::manifest_bytes(&manifest)? != manifest_bytes {
            return Err(DataError::SnapshotIntegrityFailed);
        }
        if manifest.manifest_schema != BAR_SNAPSHOT_MANIFEST_SCHEMA_ID
            || manifest.snapshot_id != snapshot.id().as_str()
            || manifest.tenant_id != snapshot.owner().tenant_id().as_str()
            || manifest.owner_id != snapshot.owner().owner_id().as_str()
            || manifest.canonical_schema_id != CANONICAL_BAR_SCHEMA_ID
            || manifest.canonical_schema_hash != hash_hex(&canonical_bar_schema_hash())
            || snapshot.schema_hash() != &canonical_bar_schema_hash()
            || manifest.parquet.content_hash != hash_hex(snapshot.content_hash())
            || manifest.parquet.size != count(parquet.len())?
            || manifest.point_in_time.as_of != timestamp(snapshot.as_of().instant())
            || manifest.point_in_time.visible_at_cutoff
                != timestamp(snapshot.visible_at().instant())
            || manifest.point_in_time.market_timezone != MARKET_TIMEZONE
            || snapshot.as_of().market_timezone() != MARKET_TIMEZONE
            || manifest.writer != writer_binding()
            || snapshot.lineage() != manifest_lineage(&manifest)?
        {
            return Err(DataError::SnapshotIntegrityFailed);
        }
        validate_quality(&manifest)?;
        parse_hash(&manifest.instrument_mapping_digest)?;
        let batch = decode_parquet_with_schema(
            parquet,
            manifest.parquet.row_count,
            &canonical_bar_schema(),
        )?;
        let bars = decode_bars(
            &manifest,
            &batch,
            snapshot.as_of().instant(),
            snapshot.visible_at().instant(),
        )?;
        let price_source_type = single_price_source(&batch)?;
        Ok(VerifiedBarSnapshot {
            snapshot,
            manifest,
            bars,
            price_source_type,
        })
    }
}

/// The one price source type all rows carry; mixed, unknown or missing values fail closed.
fn single_price_source(batch: &RecordBatch) -> DataResult<PriceSourceType> {
    let values = strings(batch, "price_source_type")?;
    let mut found = None;
    for index in 0..values.len() {
        let value = price_source_from_code(values.value(index))
            .ok_or(DataError::SnapshotIntegrityFailed)?;
        if found.is_some_and(|previous| previous != value) {
            return Err(DataError::SnapshotIntegrityFailed);
        }
        found = Some(value);
    }
    found.ok_or(DataError::SnapshotIntegrityFailed)
}

fn count(value: usize) -> DataResult<u64> {
    u64::try_from(value).map_err(|_| DataError::SnapshotIntegrityFailed)
}

fn manifest_bytes(manifest: &BarSnapshotManifest) -> DataResult<Vec<u8>> {
    let mut bytes = serde_json::to_vec(manifest).map_err(|_| DataError::SnapshotIntegrityFailed)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn request_units(request: &CanonicalBarIngestRequest) -> BarUnits {
    let exact = |unit: &ficant_domain::market::Unit| ExactVersion {
        id: unit.identity().to_owned(),
        version: unit.version(),
    };
    BarUnits {
        price: exact(&request.price_unit),
        amount: exact(&request.amount_unit),
        yield_unit: request.yield_unit.as_ref().map(exact),
    }
}

/// Every source row lands in exactly one bucket: emitted, outside the window, or rejected.
fn validate_quality(manifest: &BarSnapshotManifest) -> DataResult<()> {
    let quality = &manifest.quality;
    let rejected = count(quality.rejections.len())?;
    let sorted = quality.rejections.windows(2).all(|pair| pair[0] <= pair[1]);
    if quality.rule_set_id != BAR_QUALITY_RULE_SET_ID
        || quality.emitted_rows != manifest.parquet.row_count
        || manifest.parquet.row_count == 0
        || quality.emitted_rows + quality.outside_window_rows + rejected != quality.read_rows
        || !sorted
        || quality
            .rejections
            .iter()
            .any(|item| BarRejectionReason::from_code(&item.reason).is_none())
        || !(quality.visibility_rule == "source-field"
            || quality.visibility_rule.starts_with("fixed-delay:"))
    {
        return Err(DataError::SnapshotIntegrityFailed);
    }
    Ok(())
}

fn version_ref(value: &ExactVersion) -> DataResult<VersionRef> {
    Ok(VersionRef::new(
        Ulid::new(value.id.clone()).map_err(|_| DataError::SnapshotIntegrityFailed)?,
        Version::new(value.version).map_err(|_| DataError::SnapshotIntegrityFailed)?,
    ))
}

fn unit_ref(value: &ExactVersion) -> DataResult<UnitRef> {
    let reference = version_ref(value)?;
    Ok(UnitRef::new(reference.id().clone(), reference.version()))
}

fn manifest_lineage(manifest: &BarSnapshotManifest) -> DataResult<Vec<LineageRef>> {
    let versioned = |value: &ExactVersion| -> DataResult<LineageRef> {
        let reference = version_ref(value)?;
        Ok(LineageRef::versioned(
            reference.id().clone(),
            reference.version(),
        ))
    };
    let mut lineage = vec![
        versioned(&manifest.data_source)?,
        LineageRef::content_addressed(
            Ulid::new(manifest.instrument_mapping_id.clone())
                .map_err(|_| DataError::SnapshotIntegrityFailed)?,
            parse_hash(&manifest.instrument_mapping_digest)?,
        ),
        versioned(&manifest.calendar)?,
        versioned(&manifest.units.price)?,
        versioned(&manifest.units.amount)?,
    ];
    if let Some(unit) = &manifest.units.yield_unit {
        lineage.push(versioned(unit)?);
    }
    let mut previous: Option<&ExactVersion> = None;
    for instrument in &manifest.instruments {
        if previous.is_some_and(|value| value >= instrument) {
            return Err(DataError::SnapshotIntegrityFailed);
        }
        lineage.push(versioned(instrument)?);
        previous = Some(instrument);
    }
    if manifest.instruments.is_empty() {
        return Err(DataError::SnapshotIntegrityFailed);
    }
    Ok(lineage)
}

fn batch_instruments(batch: &RecordBatch) -> DataResult<Vec<ExactVersion>> {
    let ids = strings(batch, "instrument_id")?;
    let versions = u64s(batch, "instrument_version")?;
    let set: BTreeSet<ExactVersion> = (0..batch.num_rows())
        .map(|row| ExactVersion {
            id: ids.value(row).to_owned(),
            version: versions.value(row),
        })
        .collect();
    Ok(set.into_iter().collect())
}

fn strings<'a>(batch: &'a RecordBatch, name: &str) -> DataResult<&'a StringArray> {
    batch
        .column_by_name(name)
        .and_then(|column| column.as_any().downcast_ref::<StringArray>())
        .ok_or(DataError::SchemaMismatch)
}

fn u64s<'a>(batch: &'a RecordBatch, name: &str) -> DataResult<&'a UInt64Array> {
    batch
        .column_by_name(name)
        .and_then(|column| column.as_any().downcast_ref::<UInt64Array>())
        .ok_or(DataError::SchemaMismatch)
}

fn u32s<'a>(batch: &'a RecordBatch, name: &str) -> DataResult<&'a UInt32Array> {
    batch
        .column_by_name(name)
        .and_then(|column| column.as_any().downcast_ref::<UInt32Array>())
        .ok_or(DataError::SchemaMismatch)
}

fn instants(batch: &RecordBatch, name: &str) -> DataResult<Vec<DateTime<Utc>>> {
    let values = batch
        .column_by_name(name)
        .and_then(|column| {
            column
                .as_any()
                .downcast_ref::<arrow::array::TimestampMicrosecondArray>()
        })
        .ok_or(DataError::SchemaMismatch)?;
    (0..values.len())
        .map(|row| {
            DateTime::<Utc>::from_timestamp_micros(values.value(row))
                .ok_or(DataError::SnapshotIntegrityFailed)
        })
        .collect()
}

fn decimal(
    batch: &RecordBatch,
    prefix: &str,
    row: usize,
    unit: &UnitRef,
) -> DataResult<Option<DecimalValue>> {
    decimal_column(
        strings(batch, &format!("{prefix}_coefficient"))?,
        u32s(batch, &format!("{prefix}_scale"))?,
        row,
        unit.clone(),
    )
}

/// Rebuilds each row as a domain Bar and checks it against the manifest's exact bindings.
#[allow(clippy::too_many_lines)]
fn decode_bars(
    manifest: &BarSnapshotManifest,
    batch: &RecordBatch,
    as_of: DateTime<Utc>,
    visible_cutoff: DateTime<Utc>,
) -> DataResult<Vec<Bar>> {
    if batch.schema().as_ref() != &canonical_bar_schema()
        || count(batch.num_rows())? != manifest.parquet.row_count
    {
        return Err(DataError::SchemaMismatch);
    }
    if batch_instruments(batch)? != manifest.instruments {
        return Err(DataError::SnapshotIntegrityFailed);
    }
    let price_unit = unit_ref(&manifest.units.price)?;
    let amount_unit = unit_ref(&manifest.units.amount)?;
    let yield_unit = manifest
        .units
        .yield_unit
        .as_ref()
        .map(unit_ref)
        .transpose()?;
    let tenant = strings(batch, "tenant_id")?;
    let owner = strings(batch, "owner_id")?;
    let source_id = strings(batch, "data_source_id")?;
    let source_version = u64s(batch, "data_source_version")?;
    let record_ids = strings(batch, "source_record_id")?;
    let instrument_ids = strings(batch, "instrument_id")?;
    let instrument_versions = u64s(batch, "instrument_version")?;
    let frequencies = strings(batch, "frequency")?;
    let settlements = strings(batch, "settlement")?;
    let starts = instants(batch, "bar_start")?;
    let ends = instants(batch, "bar_end")?;
    let observed = instants(batch, "observed_at")?;
    let visible = instants(batch, "visible_at")?;
    let dates = batch
        .column_by_name("local_trading_date")
        .and_then(|column| column.as_any().downcast_ref::<arrow::array::Date32Array>())
        .ok_or(DataError::SchemaMismatch)?;
    let price_unit_ids = strings(batch, "price_unit_id")?;
    let price_unit_versions = u64s(batch, "price_unit_version")?;
    let amount_unit_ids = strings(batch, "amount_unit_id")?;
    let amount_unit_versions = u64s(batch, "amount_unit_version")?;
    let yield_unit_ids = strings(batch, "yield_unit_id")?;
    let yield_unit_versions = u64s(batch, "yield_unit_version")?;
    let trade_counts = u64s(batch, "trade_count")?;
    let mut bars = Vec::with_capacity(batch.num_rows());
    for row in 0..batch.num_rows() {
        let exact_unit = |ids: &StringArray, versions: &UInt64Array, unit: &ExactVersion| {
            ids.value(row) == unit.id && versions.value(row) == unit.version
        };
        if tenant.value(row) != manifest.tenant_id
            || owner.value(row) != manifest.owner_id
            || source_id.value(row) != manifest.data_source.id
            || source_version.value(row) != manifest.data_source.version
            || !exact_unit(price_unit_ids, price_unit_versions, &manifest.units.price)
            || !exact_unit(
                amount_unit_ids,
                amount_unit_versions,
                &manifest.units.amount,
            )
            || observed[row] != ends[row]
            || ends[row] > as_of
            || visible[row] > visible_cutoff
        {
            return Err(DataError::SnapshotIntegrityFailed);
        }
        let close_yield = match &yield_unit {
            Some(unit) => decimal(batch, "close_yield", row, unit)?,
            None => None,
        };
        let yield_bound = match (&close_yield, &manifest.units.yield_unit) {
            (Some(_), Some(unit)) => exact_unit(yield_unit_ids, yield_unit_versions, unit),
            (None, _) => yield_unit_ids.is_null(row) && yield_unit_versions.is_null(row),
            (Some(_), None) => false,
        };
        if !yield_bound
            || (yield_unit.is_none() && strings(batch, "close_yield_coefficient")?.is_valid(row))
        {
            return Err(DataError::SnapshotIntegrityFailed);
        }
        let prices = match (
            decimal(batch, "open", row, &price_unit)?,
            decimal(batch, "high", row, &price_unit)?,
            decimal(batch, "low", row, &price_unit)?,
            decimal(batch, "close", row, &price_unit)?,
        ) {
            (Some(open), Some(high), Some(low), Some(close)) => Some(BarPrices {
                open,
                high,
                low,
                close,
            }),
            (None, None, None, None) => None,
            _ => return Err(DataError::SnapshotIntegrityFailed),
        };
        let input = BarInput {
            source_record_id: record_ids.value(row).to_owned(),
            instrument: VersionRef::new(
                Ulid::new(instrument_ids.value(row))
                    .map_err(|_| DataError::SnapshotIntegrityFailed)?,
                Version::new(instrument_versions.value(row))
                    .map_err(|_| DataError::SnapshotIntegrityFailed)?,
            ),
            frequency: BarFrequency::from_code(frequencies.value(row))
                .ok_or(DataError::SnapshotIntegrityFailed)?,
            settlement: BarSettlement::from_code(settlements.value(row))
                .ok_or(DataError::SnapshotIntegrityFailed)?,
            market_timezone: MARKET_TIMEZONE.to_owned(),
            local_trading_date: epoch_date(dates.value(row))?,
            bar_start: starts[row],
            bar_end: ends[row],
            visible_at: visible[row],
            prices,
            volume: decimal(batch, "volume", row, &amount_unit)?
                .ok_or(DataError::SnapshotIntegrityFailed)?,
            turnover: decimal(batch, "turnover", row, &amount_unit)?,
            trade_count: trade_counts.is_valid(row).then(|| trade_counts.value(row)),
            vwap: decimal(batch, "vwap", row, &price_unit)?,
            close_yield,
        };
        bars.push(Bar::new(input).map_err(|_| DataError::SnapshotIntegrityFailed)?);
    }
    Ok(bars)
}
