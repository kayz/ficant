use std::collections::BTreeSet;
use std::io::Cursor;
use std::sync::Arc;

use arrow::array::{
    Array, ArrayRef, BooleanArray, Date32Array, Decimal128Array, FixedSizeBinaryArray,
    PrimitiveArray, RecordBatch, StringArray, UInt32Array, UInt64Array,
};
use arrow::datatypes::{
    ArrowPrimitiveType, DataType, Date32Type, Decimal128Type, Field, Schema, TimeUnit,
    TimestampSecondType, UInt32Type, UInt64Type,
};
use arrow::ipc::MetadataVersion;
use arrow::ipc::reader::FileReader;
use arrow::ipc::writer::{FileWriter, IpcWriteOptions};
use chrono::{NaiveDate, TimeDelta, Utc};
use ficant_application::ports::{
    EncodedFuturesDeliveryArtifact, FuturesDeliveryArtifactCandidateFacts,
    FuturesDeliveryArtifactCodec, FuturesDeliveryArtifactFacts,
};
use ficant_domain::analytics::{
    ABI_VERSION, AnalyticsError, DECIMAL_SCALE, ENGINE_ID, ENGINE_VERSION, FixedDecimal,
};
use ficant_domain::analytics::{AnalyticsObjectRef, MARKET_TIMEZONE};
use ficant_domain::futures_delivery::{
    FUTURES_DELIVERY_ALGORITHM_ID, FUTURES_DELIVERY_ALGORITHM_VERSION,
    FUTURES_DELIVERY_ARTIFACT_CODEC_ID, FUTURES_DELIVERY_ARTIFACT_SCHEMA_ID,
    FUTURES_DELIVERY_CONVENTION_PROFILE, FuturesDeliverableInput, FuturesDeliveryBasketResult,
    FuturesDeliveryMeasures, FuturesDeliveryResult,
};
use ficant_domain::primitives::{ContentHash, MarketTime, Ulid, Version, VersionRef};

const DECIMAL_PRECISION: u8 = 38;
const DECIMAL_SCALE_I8: i8 = 12;
const V1_COLUMN_COUNT: usize = 27;
const R5D_COLUMN_COUNT: usize = 43;
const R5D_ARTIFACT_SCHEMA_ID: &str = "ficant.cgb-futures-delivery.arrow.v2";
const R5D_ARTIFACT_CODEC_ID: &str = "ficant-cgb-futures-delivery-arrow/2";
const _: () = assert!(DECIMAL_SCALE == 12);

#[derive(Clone, Copy, Debug, Default)]
pub struct ArrowFuturesDeliveryCodec;

impl FuturesDeliveryArtifactCodec for ArrowFuturesDeliveryCodec {
    fn encode(
        &self,
        result: &FuturesDeliveryBasketResult,
    ) -> Result<EncodedFuturesDeliveryArtifact, AnalyticsError> {
        encode_artifact(&encode_batch(result, false)?)
    }

    fn encode_self_describing(
        &self,
        result: &FuturesDeliveryBasketResult,
    ) -> Result<EncodedFuturesDeliveryArtifact, AnalyticsError> {
        encode_artifact(&encode_batch(result, true)?)
    }

    fn decode(
        &self,
        bytes: &[u8],
        expected_inputs: &[FuturesDeliverableInput],
    ) -> Result<FuturesDeliveryBasketResult, AnalyticsError> {
        if expected_inputs.is_empty() {
            return Err(AnalyticsError::InvalidInput);
        }
        let batch = read_batch(bytes, expected_inputs.len())?;
        let self_describing = match batch.num_columns() {
            V1_COLUMN_COUNT => false,
            R5D_COLUMN_COUNT => true,
            _ => return Err(AnalyticsError::InvalidInput),
        };
        let expected_schema = artifact_schema(self_describing);
        if batch.schema().as_ref() != &expected_schema {
            return Err(AnalyticsError::InvalidInput);
        }
        let mut candidates = Vec::with_capacity(expected_inputs.len());
        let mut ctd_index = None;
        for (row, input) in expected_inputs.iter().enumerate() {
            validate_input_columns(&batch, row, input, expected_inputs.len(), self_describing)?;
            if boolean(&batch, 12, row)? && ctd_index.replace(row).is_some() {
                return Err(AnalyticsError::InvalidInput);
            }
            let measures = decode_measures(&batch, row)?;
            candidates.push(FuturesDeliveryResult::new(input.clone(), measures));
        }
        FuturesDeliveryBasketResult::new(candidates, ctd_index.ok_or(AnalyticsError::InvalidInput)?)
            .map_err(|_| AnalyticsError::InvalidInput)
    }

    fn decode_facts(&self, bytes: &[u8]) -> Result<FuturesDeliveryArtifactFacts, AnalyticsError> {
        let batch = read_batch(bytes, 0)?;
        if batch.num_columns() != R5D_COLUMN_COUNT
            || batch.schema().as_ref() != &artifact_schema(true)
        {
            return Err(AnalyticsError::InvalidInput);
        }
        let rows = batch.num_rows();
        let mut candidates = Vec::with_capacity(rows);
        let mut bond_ids = BTreeSet::new();
        let mut ctd_index = None;
        for row in 0..rows {
            validate_common_columns(&batch, row, rows, true)?;
            if row > 0 {
                validate_shared_columns(&batch, row)?;
            }
            binary(&batch, 10, row)?;
            if boolean(&batch, 12, row)? && ctd_index.replace(row).is_some() {
                return Err(AnalyticsError::InvalidInput);
            }
            let bond = object_ref(&batch, row, 11, 29, 30)?;
            if !bond_ids.insert(bond.version_ref().id().clone()) {
                return Err(AnalyticsError::InvalidInput);
            }
            let measures = decode_measures(&batch, row)?;
            candidates.push(FuturesDeliveryArtifactCandidateFacts::new(
                bond,
                measures.conversion_factor(),
            ));
        }
        let first = 0;
        let instant = chrono::DateTime::<Utc>::from_timestamp(
            timestamp(&batch, 41, first)?,
            uint32(&batch, 42, first)?,
        )
        .ok_or(AnalyticsError::InvalidInput)?;
        let valuation_at = MarketTime::new(
            instant,
            string(&batch, 27, first)?,
            date(&batch, 28, first)?,
        )
        .map_err(|_| AnalyticsError::InvalidInput)?;
        let product = decode_product(&batch, first)?;
        Ok(FuturesDeliveryArtifactFacts::new(
            valuation_at,
            object_ref(&batch, first, 31, 32, 33)?,
            object_ref(&batch, first, 34, 35, 36)?,
            object_ref(&batch, first, 37, 38, 39)?,
            product,
            candidates,
            ctd_index.ok_or(AnalyticsError::InvalidInput)?,
        ))
    }
}

fn decode_product(
    batch: &RecordBatch,
    row: usize,
) -> Result<ficant_domain::futures_delivery::CgbFuturesProduct, AnalyticsError> {
    match uint32(batch, 40, row)? {
        1 => Ok(ficant_domain::futures_delivery::CgbFuturesProduct::TwoYear),
        2 => Ok(ficant_domain::futures_delivery::CgbFuturesProduct::FiveYear),
        3 => Ok(ficant_domain::futures_delivery::CgbFuturesProduct::TenYear),
        4 => Ok(ficant_domain::futures_delivery::CgbFuturesProduct::ThirtyYear),
        _ => Err(AnalyticsError::InvalidInput),
    }
}

fn decode_measures(
    batch: &RecordBatch,
    row: usize,
) -> Result<FuturesDeliveryMeasures, AnalyticsError> {
    FuturesDeliveryMeasures::new(
        uint32(batch, 13, row)?,
        uint32(batch, 14, row)?,
        decimal(batch, 15, row)?,
        decimal(batch, 16, row)?,
        decimal(batch, 17, row)?,
        decimal(batch, 18, row)?,
        decimal(batch, 19, row)?,
        decimal(batch, 20, row)?,
        decimal(batch, 21, row)?,
        decimal(batch, 22, row)?,
        decimal(batch, 23, row)?,
        decimal(batch, 24, row)?,
        decimal(batch, 25, row)?,
        decimal(batch, 26, row)?,
    )
    .map_err(|_| AnalyticsError::InvalidInput)
}

fn validate_shared_columns(batch: &RecordBatch, row: usize) -> Result<(), AnalyticsError> {
    let checks = [
        string(batch, 27, row)? == string(batch, 27, 0)?,
        date(batch, 28, row)? == date(batch, 28, 0)?,
        string(batch, 31, row)? == string(batch, 31, 0)?,
        uint64(batch, 32, row)? == uint64(batch, 32, 0)?,
        binary(batch, 33, row)? == binary(batch, 33, 0)?,
        string(batch, 34, row)? == string(batch, 34, 0)?,
        uint64(batch, 35, row)? == uint64(batch, 35, 0)?,
        binary(batch, 36, row)? == binary(batch, 36, 0)?,
        string(batch, 37, row)? == string(batch, 37, 0)?,
        uint64(batch, 38, row)? == uint64(batch, 38, 0)?,
        binary(batch, 39, row)? == binary(batch, 39, 0)?,
        uint32(batch, 40, row)? == uint32(batch, 40, 0)?,
        timestamp(batch, 41, row)? == timestamp(batch, 41, 0)?,
        uint32(batch, 42, row)? == uint32(batch, 42, 0)?,
    ];
    if checks.into_iter().all(|value| value) {
        Ok(())
    } else {
        Err(AnalyticsError::InvalidInput)
    }
}

fn artifact_schema(self_describing: bool) -> Schema {
    let mut fields = vec![
        text_field("schema_id"),
        text_field("codec_id"),
        text_field("engine_id"),
        text_field("engine_version"),
        text_field("algorithm_id"),
        text_field("convention_profile"),
        Field::new("algorithm_version", DataType::UInt32, false),
        Field::new("abi_version", DataType::UInt32, false),
        Field::new("basket_size", DataType::UInt32, false),
        Field::new("row_index", DataType::UInt32, false),
        Field::new("input_fingerprint", DataType::FixedSizeBinary(32), false),
        text_field("bond_id"),
        Field::new("is_ctd", DataType::Boolean, false),
        Field::new("months_to_next_coupon", DataType::UInt32, false),
        Field::new("remaining_coupon_count", DataType::UInt32, false),
        decimal_field("conversion_factor"),
        decimal_field("purchase_accrued_interest"),
        decimal_field("delivery_accrued_interest"),
        decimal_field("interim_coupons"),
        decimal_field("invoice_price"),
        decimal_field("purchase_dirty_price"),
        decimal_field("gross_basis"),
        decimal_field("financing_cost"),
        decimal_field("holding_carry"),
        decimal_field("net_basis"),
        decimal_field("implied_repo_rate"),
        decimal_field("delivery_profit"),
    ];
    if self_describing {
        fields.extend([
            text_field("market_timezone"),
            Field::new("valuation_local_date", DataType::Date32, false),
            Field::new("bond_version", DataType::UInt64, false),
            Field::new("bond_content_hash", DataType::FixedSizeBinary(32), false),
            text_field("futures_contract_id"),
            Field::new("futures_contract_version", DataType::UInt64, false),
            Field::new(
                "futures_contract_content_hash",
                DataType::FixedSizeBinary(32),
                false,
            ),
            text_field("rule_pack_id"),
            Field::new("rule_pack_version", DataType::UInt64, false),
            Field::new(
                "rule_pack_content_hash",
                DataType::FixedSizeBinary(32),
                false,
            ),
            text_field("snapshot_id"),
            Field::new("snapshot_version", DataType::UInt64, false),
            Field::new(
                "snapshot_content_hash",
                DataType::FixedSizeBinary(32),
                false,
            ),
            Field::new("product", DataType::UInt32, false),
            Field::new(
                "valuation_at_seconds",
                DataType::Timestamp(TimeUnit::Second, Some("UTC".into())),
                false,
            ),
            Field::new("valuation_at_nanos", DataType::UInt32, false),
        ]);
    }
    Schema::new(fields)
}

fn text_field(name: &str) -> Field {
    Field::new(name, DataType::Utf8, false)
}

fn decimal_field(name: &str) -> Field {
    Field::new(
        name,
        DataType::Decimal128(DECIMAL_PRECISION, DECIMAL_SCALE_I8),
        false,
    )
}

fn encode_batch(
    result: &FuturesDeliveryBasketResult,
    self_describing: bool,
) -> Result<RecordBatch, AnalyticsError> {
    let candidates = result.candidates();
    let row_count = candidates.len();
    let basket_size = u32::try_from(row_count).map_err(|_| AnalyticsError::InvalidInput)?;
    let row_indices = (0..row_count)
        .map(|value| u32::try_from(value).map_err(|_| AnalyticsError::InvalidInput))
        .collect::<Result<Vec<_>, _>>()?;
    let fingerprints = candidates
        .iter()
        .map(|candidate| candidate.input().fingerprint())
        .collect::<Vec<_>>();
    let mut columns: Vec<ArrayRef> = vec![
        text_array(
            row_count,
            if self_describing {
                R5D_ARTIFACT_SCHEMA_ID
            } else {
                FUTURES_DELIVERY_ARTIFACT_SCHEMA_ID
            },
        ),
        text_array(
            row_count,
            if self_describing {
                R5D_ARTIFACT_CODEC_ID
            } else {
                FUTURES_DELIVERY_ARTIFACT_CODEC_ID
            },
        ),
        text_array(row_count, ENGINE_ID),
        text_array(row_count, ENGINE_VERSION),
        text_array(row_count, FUTURES_DELIVERY_ALGORITHM_ID),
        text_array(row_count, FUTURES_DELIVERY_CONVENTION_PROFILE),
        Arc::new(UInt32Array::from(vec![
            FUTURES_DELIVERY_ALGORITHM_VERSION;
            row_count
        ])),
        Arc::new(UInt32Array::from(vec![ABI_VERSION; row_count])),
        Arc::new(UInt32Array::from(vec![basket_size; row_count])),
        Arc::new(UInt32Array::from(row_indices)),
        hash_array(&fingerprints)?,
        Arc::new(StringArray::from(
            candidates
                .iter()
                .map(|candidate| candidate.input().bond().version_ref().id().as_str())
                .collect::<Vec<_>>(),
        )),
        Arc::new(BooleanArray::from(
            (0..row_count)
                .map(|index| index == result.ctd_index())
                .collect::<Vec<_>>(),
        )),
        uint32_measures(candidates, FuturesDeliveryMeasures::months_to_next_coupon),
        uint32_measures(candidates, FuturesDeliveryMeasures::remaining_coupon_count),
        decimal_measures(candidates, FuturesDeliveryMeasures::conversion_factor)?,
        decimal_measures(
            candidates,
            FuturesDeliveryMeasures::purchase_accrued_interest,
        )?,
        decimal_measures(
            candidates,
            FuturesDeliveryMeasures::delivery_accrued_interest,
        )?,
        decimal_measures(candidates, FuturesDeliveryMeasures::interim_coupons)?,
        decimal_measures(candidates, FuturesDeliveryMeasures::invoice_price)?,
        decimal_measures(candidates, FuturesDeliveryMeasures::purchase_dirty_price)?,
        decimal_measures(candidates, FuturesDeliveryMeasures::gross_basis)?,
        decimal_measures(candidates, FuturesDeliveryMeasures::financing_cost)?,
        decimal_measures(candidates, FuturesDeliveryMeasures::holding_carry)?,
        decimal_measures(candidates, FuturesDeliveryMeasures::net_basis)?,
        decimal_measures(candidates, FuturesDeliveryMeasures::implied_repo_rate)?,
        decimal_measures(candidates, FuturesDeliveryMeasures::delivery_profit)?,
    ];
    if self_describing {
        columns.extend(self_describing_columns(candidates, row_count)?);
    }
    RecordBatch::try_new(Arc::new(artifact_schema(self_describing)), columns)
        .map_err(|_| AnalyticsError::Internal)
}

fn self_describing_columns(
    candidates: &[FuturesDeliveryResult],
    row_count: usize,
) -> Result<Vec<ArrayRef>, AnalyticsError> {
    let common = candidates[0].input();
    Ok(vec![
        text_array(row_count, common.valuation_at().market_timezone()),
        date_array(row_count, common.valuation_at().local_trading_date())?,
        Arc::new(UInt64Array::from(
            candidates
                .iter()
                .map(|candidate| candidate.input().bond().version_ref().version().get())
                .collect::<Vec<_>>(),
        )),
        hash_array(
            &candidates
                .iter()
                .map(|candidate| candidate.input().bond().content_hash().clone())
                .collect::<Vec<_>>(),
        )?,
        text_array(
            row_count,
            common.futures_contract().version_ref().id().as_str(),
        ),
        Arc::new(UInt64Array::from(vec![
            common
                .futures_contract()
                .version_ref()
                .version()
                .get();
            row_count
        ])),
        repeated_hash_array(row_count, common.futures_contract().content_hash())?,
        text_array(row_count, common.rule_pack().version_ref().id().as_str()),
        Arc::new(UInt64Array::from(vec![
            common
                .rule_pack()
                .version_ref()
                .version()
                .get();
            row_count
        ])),
        repeated_hash_array(row_count, common.rule_pack().content_hash())?,
        text_array(row_count, common.snapshot().version_ref().id().as_str()),
        Arc::new(UInt64Array::from(vec![
            common
                .snapshot()
                .version_ref()
                .version()
                .get();
            row_count
        ])),
        repeated_hash_array(row_count, common.snapshot().content_hash())?,
        Arc::new(UInt32Array::from(vec![common.product() as u32; row_count])),
        Arc::new(
            arrow::array::TimestampSecondArray::from(vec![
                common
                    .valuation_at()
                    .instant()
                    .timestamp();
                row_count
            ])
            .with_timezone("UTC"),
        ),
        Arc::new(UInt32Array::from(vec![
            common
                .valuation_at()
                .instant()
                .timestamp_subsec_nanos();
            row_count
        ])),
    ])
}

fn validate_input_columns(
    batch: &RecordBatch,
    row: usize,
    input: &FuturesDeliverableInput,
    basket_size: usize,
    self_describing: bool,
) -> Result<(), AnalyticsError> {
    validate_common_columns(batch, row, basket_size, self_describing)?;
    let base_checks = [
        string(batch, 2, row)? == ENGINE_ID,
        string(batch, 3, row)? == ENGINE_VERSION,
        string(batch, 4, row)? == FUTURES_DELIVERY_ALGORITHM_ID,
        string(batch, 5, row)? == FUTURES_DELIVERY_CONVENTION_PROFILE,
        uint32(batch, 6, row)? == FUTURES_DELIVERY_ALGORITHM_VERSION,
        uint32(batch, 7, row)? == ABI_VERSION,
        usize::try_from(uint32(batch, 8, row)?).ok() == Some(basket_size),
        usize::try_from(uint32(batch, 9, row)?).ok() == Some(row),
        binary(batch, 10, row)? == input.fingerprint().as_bytes(),
        string(batch, 11, row)? == input.bond().version_ref().id().as_str(),
    ];
    if !base_checks.into_iter().all(|value| value) {
        return Err(AnalyticsError::InvalidInput);
    }
    if !self_describing {
        return Ok(());
    }
    let extended_checks = [
        string(batch, 27, row)? == input.valuation_at().market_timezone(),
        date(batch, 28, row)? == input.valuation_at().local_trading_date(),
        uint64(batch, 29, row)? == input.bond().version_ref().version().get(),
        binary(batch, 30, row)? == input.bond().content_hash().as_bytes(),
        string(batch, 31, row)? == input.futures_contract().version_ref().id().as_str(),
        uint64(batch, 32, row)? == input.futures_contract().version_ref().version().get(),
        binary(batch, 33, row)? == input.futures_contract().content_hash().as_bytes(),
        string(batch, 34, row)? == input.rule_pack().version_ref().id().as_str(),
        uint64(batch, 35, row)? == input.rule_pack().version_ref().version().get(),
        binary(batch, 36, row)? == input.rule_pack().content_hash().as_bytes(),
        string(batch, 37, row)? == input.snapshot().version_ref().id().as_str(),
        uint64(batch, 38, row)? == input.snapshot().version_ref().version().get(),
        binary(batch, 39, row)? == input.snapshot().content_hash().as_bytes(),
        uint32(batch, 40, row)? == input.product() as u32,
        timestamp(batch, 41, row)? == input.valuation_at().instant().timestamp(),
        uint32(batch, 42, row)? == input.valuation_at().instant().timestamp_subsec_nanos(),
    ];
    if extended_checks.into_iter().all(|value| value) {
        Ok(())
    } else {
        Err(AnalyticsError::InvalidInput)
    }
}

fn validate_common_columns(
    batch: &RecordBatch,
    row: usize,
    basket_size: usize,
    self_describing: bool,
) -> Result<(), AnalyticsError> {
    let checks = [
        string(batch, 0, row)?
            == if self_describing {
                R5D_ARTIFACT_SCHEMA_ID
            } else {
                FUTURES_DELIVERY_ARTIFACT_SCHEMA_ID
            },
        string(batch, 1, row)?
            == if self_describing {
                R5D_ARTIFACT_CODEC_ID
            } else {
                FUTURES_DELIVERY_ARTIFACT_CODEC_ID
            },
        string(batch, 2, row)? == ENGINE_ID,
        string(batch, 3, row)? == ENGINE_VERSION,
        string(batch, 4, row)? == FUTURES_DELIVERY_ALGORITHM_ID,
        string(batch, 5, row)? == FUTURES_DELIVERY_CONVENTION_PROFILE,
        uint32(batch, 6, row)? == FUTURES_DELIVERY_ALGORITHM_VERSION,
        uint32(batch, 7, row)? == ABI_VERSION,
        usize::try_from(uint32(batch, 8, row)?).ok() == Some(basket_size),
        usize::try_from(uint32(batch, 9, row)?).ok() == Some(row),
    ];
    if checks.into_iter().all(|value| value)
        && (!self_describing || string(batch, 27, row)? == MARKET_TIMEZONE)
    {
        Ok(())
    } else {
        Err(AnalyticsError::InvalidInput)
    }
}

fn text_array(rows: usize, value: &str) -> ArrayRef {
    Arc::new(StringArray::from(vec![value; rows]))
}

fn date_array(rows: usize, value: NaiveDate) -> Result<ArrayRef, AnalyticsError> {
    Ok(Arc::new(Date32Array::from(vec![epoch_days(value)?; rows])))
}

fn uint32_measures(
    candidates: &[FuturesDeliveryResult],
    value: impl Fn(FuturesDeliveryMeasures) -> u32,
) -> ArrayRef {
    Arc::new(UInt32Array::from(
        candidates
            .iter()
            .map(|candidate| value(candidate.measures()))
            .collect::<Vec<_>>(),
    ))
}

fn decimal_measures(
    candidates: &[FuturesDeliveryResult],
    value: impl Fn(FuturesDeliveryMeasures) -> FixedDecimal,
) -> Result<ArrayRef, AnalyticsError> {
    let array = Decimal128Array::from(
        candidates
            .iter()
            .map(|candidate| value(candidate.measures()).scaled())
            .collect::<Vec<_>>(),
    )
    .with_precision_and_scale(DECIMAL_PRECISION, DECIMAL_SCALE_I8)
    .map_err(|_| AnalyticsError::InvalidInput)?;
    Ok(Arc::new(array))
}

fn hash_array(values: &[ContentHash]) -> Result<ArrayRef, AnalyticsError> {
    FixedSizeBinaryArray::try_from_iter(values.iter().map(ContentHash::as_bytes))
        .map(|array| Arc::new(array) as ArrayRef)
        .map_err(|_| AnalyticsError::Internal)
}

fn repeated_hash_array(rows: usize, value: &ContentHash) -> Result<ArrayRef, AnalyticsError> {
    FixedSizeBinaryArray::try_from_iter((0..rows).map(|_| value.as_bytes()))
        .map(|array| Arc::new(array) as ArrayRef)
        .map_err(|_| AnalyticsError::Internal)
}

fn encode_artifact(batch: &RecordBatch) -> Result<EncodedFuturesDeliveryArtifact, AnalyticsError> {
    let options = IpcWriteOptions::try_new(64, false, MetadataVersion::V5)
        .map_err(|_| AnalyticsError::Internal)?;
    let mut bytes = Vec::new();
    {
        let mut writer =
            FileWriter::try_new_with_options(&mut bytes, batch.schema().as_ref(), options)
                .map_err(|_| AnalyticsError::Internal)?;
        writer.write(batch).map_err(|_| AnalyticsError::Internal)?;
        writer.finish().map_err(|_| AnalyticsError::Internal)?;
    }
    let content_hash = ContentHash::digest(&bytes);
    EncodedFuturesDeliveryArtifact::new(bytes, content_hash).map_err(|_| AnalyticsError::Internal)
}

fn read_batch(bytes: &[u8], expected_rows: usize) -> Result<RecordBatch, AnalyticsError> {
    let mut reader =
        FileReader::try_new(Cursor::new(bytes), None).map_err(|_| AnalyticsError::InvalidInput)?;
    let batch = reader
        .next()
        .ok_or(AnalyticsError::InvalidInput)?
        .map_err(|_| AnalyticsError::InvalidInput)?;
    if reader.next().is_some()
        || batch.num_rows() == 0
        || (expected_rows != 0 && batch.num_rows() != expected_rows)
    {
        return Err(AnalyticsError::InvalidInput);
    }
    Ok(batch)
}

fn object_ref(
    batch: &RecordBatch,
    row: usize,
    id_column: usize,
    version_column: usize,
    hash_column: usize,
) -> Result<AnalyticsObjectRef, AnalyticsError> {
    let id = Ulid::new(string(batch, id_column, row)?).map_err(|_| AnalyticsError::InvalidInput)?;
    let version = Version::new(uint64(batch, version_column, row)?)
        .map_err(|_| AnalyticsError::InvalidInput)?;
    let hash = ContentHash::from_bytes(binary(batch, hash_column, row)?)
        .map_err(|_| AnalyticsError::InvalidInput)?;
    Ok(AnalyticsObjectRef::new(VersionRef::new(id, version), hash))
}

fn string(batch: &RecordBatch, column: usize, row: usize) -> Result<&str, AnalyticsError> {
    let array = downcast::<StringArray>(batch.column(column))?;
    non_null(array, row)?;
    Ok(array.value(row))
}

fn uint32(batch: &RecordBatch, column: usize, row: usize) -> Result<u32, AnalyticsError> {
    primitive_value::<UInt32Type>(batch.column(column), row)
}

fn uint64(batch: &RecordBatch, column: usize, row: usize) -> Result<u64, AnalyticsError> {
    primitive_value::<UInt64Type>(batch.column(column), row)
}

fn timestamp(batch: &RecordBatch, column: usize, row: usize) -> Result<i64, AnalyticsError> {
    primitive_value::<TimestampSecondType>(batch.column(column), row)
}

fn date(batch: &RecordBatch, column: usize, row: usize) -> Result<NaiveDate, AnalyticsError> {
    date_from_epoch_days(primitive_value::<Date32Type>(batch.column(column), row)?)
}

fn boolean(batch: &RecordBatch, column: usize, row: usize) -> Result<bool, AnalyticsError> {
    let array = downcast::<BooleanArray>(batch.column(column))?;
    non_null(array, row)?;
    Ok(array.value(row))
}

fn decimal(batch: &RecordBatch, column: usize, row: usize) -> Result<FixedDecimal, AnalyticsError> {
    primitive_value::<Decimal128Type>(batch.column(column), row).map(FixedDecimal::from_scaled)
}

fn binary(batch: &RecordBatch, column: usize, row: usize) -> Result<&[u8], AnalyticsError> {
    let array = downcast::<FixedSizeBinaryArray>(batch.column(column))?;
    non_null(array, row)?;
    Ok(array.value(row))
}

fn primitive_value<T>(array: &ArrayRef, row: usize) -> Result<T::Native, AnalyticsError>
where
    T: ArrowPrimitiveType,
{
    let array = downcast::<PrimitiveArray<T>>(array)?;
    non_null(array, row)?;
    Ok(array.value(row))
}

fn downcast<T: Array + 'static>(array: &dyn Array) -> Result<&T, AnalyticsError> {
    array
        .as_any()
        .downcast_ref::<T>()
        .ok_or(AnalyticsError::InvalidInput)
}

fn non_null(array: &dyn Array, row: usize) -> Result<(), AnalyticsError> {
    if row >= array.len() || array.is_null(row) {
        Err(AnalyticsError::InvalidInput)
    } else {
        Ok(())
    }
}

fn epoch() -> NaiveDate {
    NaiveDate::from_ymd_opt(1970, 1, 1).expect("Unix epoch is a valid date")
}

fn epoch_days(date: NaiveDate) -> Result<i32, AnalyticsError> {
    i32::try_from(date.signed_duration_since(epoch()).num_days())
        .map_err(|_| AnalyticsError::InvalidInput)
}

fn date_from_epoch_days(days: i32) -> Result<NaiveDate, AnalyticsError> {
    epoch()
        .checked_add_signed(TimeDelta::days(i64::from(days)))
        .ok_or(AnalyticsError::InvalidInput)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Timelike, Utc};
    use ficant_application::CalculateFuturesDeliveryBasket;
    use ficant_domain::analytics::{
        BondTerms, BusinessDayConvention, CouponFrequency, DayCountConvention,
    };
    use ficant_domain::futures_delivery::{
        CgbFuturesProduct, FuturesDeliveryRule, FuturesDeliveryRuleInput,
    };
    use ficant_domain::primitives::{OwnerRef, VersionRef};
    use ficant_fixed_income_native::NativeFuturesDeliveryEngine;

    #[test]
    fn r5d_self_describing_artifact_round_trips_facts_and_preserves_nanoseconds() {
        let inputs = inputs();
        let result = CalculateFuturesDeliveryBasket::new(&NativeFuturesDeliveryEngine)
            .execute(&inputs)
            .unwrap();
        let codec = ArrowFuturesDeliveryCodec;
        let first = codec.encode_self_describing(&result).unwrap();
        let second = codec.encode_self_describing(&result).unwrap();
        assert_eq!(first, second);
        assert_eq!(codec.decode(first.bytes(), &inputs).unwrap(), result);

        let facts = codec.decode_facts(first.bytes()).unwrap();
        assert_eq!(facts.valuation_at(), inputs[0].valuation_at());
        assert_eq!(facts.futures_contract(), inputs[0].futures_contract());
        assert_eq!(facts.rule_pack(), inputs[0].rule_pack());
        assert_eq!(facts.snapshot(), inputs[0].snapshot());
        assert_eq!(facts.product(), inputs[0].product());
        assert_eq!(facts.ctd_index(), result.ctd_index());
        assert_eq!(facts.candidates().len(), inputs.len());
        for ((candidate, input), calculated) in facts
            .candidates()
            .iter()
            .zip(&inputs)
            .zip(result.candidates())
        {
            assert_eq!(candidate.bond(), input.bond());
            assert_eq!(
                candidate.conversion_factor(),
                calculated.measures().conversion_factor()
            );
        }

        let legacy = codec.encode(&result).unwrap();
        assert_eq!(
            codec.decode_facts(legacy.bytes()),
            Err(AnalyticsError::InvalidInput),
            "Phase 2C v1 omits authority facts and must never be guessed"
        );
    }

    #[test]
    fn r5d_self_describing_facts_fail_closed_on_schema_shared_fact_and_time_tamper() {
        let inputs = inputs();
        let result = CalculateFuturesDeliveryBasket::new(&NativeFuturesDeliveryEngine)
            .execute(&inputs)
            .unwrap();
        let codec = ArrowFuturesDeliveryCodec;
        let encoded = codec.encode_self_describing(&result).unwrap();
        let batch = read_batch(encoded.bytes(), inputs.len()).unwrap();

        let schema = batch.schema();
        let mut fields = schema
            .fields()
            .iter()
            .map(|field| field.as_ref().clone())
            .collect::<Vec<_>>();
        fields[31] = Field::new(
            "untrusted_futures_contract_id",
            fields[31].data_type().clone(),
            false,
        );
        let schema_tamper = RecordBatch::try_new(
            Arc::new(Schema::new_with_metadata(fields, schema.metadata().clone())),
            batch.columns().to_vec(),
        )
        .unwrap();
        assert_eq!(
            codec.decode_facts(&write_batch(&schema_tamper)),
            Err(AnalyticsError::InvalidInput)
        );

        let mut columns = batch.columns().to_vec();
        columns[31] = Arc::new(StringArray::from(vec![
            inputs[0].futures_contract().version_ref().id().as_str(),
            id('J').as_str(),
        ]));
        let shared_tamper = RecordBatch::try_new(batch.schema(), columns).unwrap();
        assert_eq!(
            codec.decode_facts(&write_batch(&shared_tamper)),
            Err(AnalyticsError::InvalidInput)
        );

        let mut columns = batch.columns().to_vec();
        columns[42] = Arc::new(UInt32Array::from(vec![1_000_000_000; inputs.len()]));
        let time_tamper = RecordBatch::try_new(batch.schema(), columns).unwrap();
        assert_eq!(
            codec.decode_facts(&write_batch(&time_tamper)),
            Err(AnalyticsError::InvalidInput)
        );

        let mut truncated = encoded.bytes().to_vec();
        truncated.truncate(truncated.len() / 2);
        assert_eq!(
            codec.decode_facts(&truncated),
            Err(AnalyticsError::InvalidInput)
        );
    }

    fn write_batch(batch: &RecordBatch) -> Vec<u8> {
        let options = IpcWriteOptions::try_new(64, false, MetadataVersion::V5).unwrap();
        let mut bytes = Vec::new();
        {
            let mut writer =
                FileWriter::try_new_with_options(&mut bytes, batch.schema().as_ref(), options)
                    .unwrap();
            writer.write(batch).unwrap();
            writer.finish().unwrap();
        }
        bytes
    }

    fn inputs() -> Vec<FuturesDeliverableInput> {
        vec![
            input('G', fixed(102_000_000_000_000)),
            input('H', fixed(100_000_000_000_000)),
        ]
    }

    fn input(bond_suffix: char, spot_clean_price: FixedDecimal) -> FuturesDeliverableInput {
        FuturesDeliverableInput::new(
            OwnerRef::new(id('A'), id('B')),
            object('C'),
            object(bond_suffix),
            object('D'),
            object('E'),
            MarketTime::new(
                Utc.with_ymd_and_hms(2026, 7, 20, 4, 0, 0)
                    .single()
                    .unwrap()
                    .with_nanosecond(123_456_789)
                    .unwrap(),
                MARKET_TIMEZONE,
                date(2026, 7, 20),
            )
            .unwrap(),
            date(2026, 7, 21),
            date(2026, 9, 1),
            date(2026, 9, 18),
            CgbFuturesProduct::TenYear,
            rule(),
            BondTerms::new(
                date(2024, 8, 15),
                date(2034, 8, 15),
                CouponFrequency::Semiannual,
                DayCountConvention::ActActBondIsma,
                BusinessDayConvention::Following,
                fixed(25_000_000_000),
                fixed(100_000_000_000_000),
            )
            .unwrap(),
            spot_clean_price,
            fixed(99_500_000_000_000),
            fixed(18_000_000_000),
        )
        .unwrap()
    }

    fn rule() -> FuturesDeliveryRule {
        FuturesDeliveryRule::new(FuturesDeliveryRuleInput {
            original_term_max_months: 120,
            residual_min_months: 78,
            residual_max_months: None,
            delivery_months: vec![3, 6, 9, 12],
            nominal_coupon: fixed(30_000_000_000),
            face_quote_basis: fixed(100_000_000_000_000),
            accrued_interest_day_count: 1,
            conversion_factor_rounding_places: 4,
            accrued_interest_rounding_places: 7,
            annual_day_basis: 365,
        })
        .unwrap()
    }

    fn object(suffix: char) -> AnalyticsObjectRef {
        AnalyticsObjectRef::new(
            VersionRef::new(id(suffix), Version::new(1).unwrap()),
            ContentHash::digest(suffix.to_string().as_bytes()),
        )
    }

    fn id(suffix: char) -> Ulid {
        Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
    }

    const fn fixed(value: i128) -> FixedDecimal {
        FixedDecimal::from_scaled(value)
    }

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }
}
