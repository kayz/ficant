use std::io::Cursor;
use std::sync::Arc;

use arrow::array::{
    Array, ArrayRef, BooleanArray, Decimal128Array, FixedSizeBinaryArray, PrimitiveArray,
    RecordBatch, StringArray, UInt32Array,
};
use arrow::datatypes::{ArrowPrimitiveType, DataType, Decimal128Type, Field, Schema, UInt32Type};
use arrow::ipc::MetadataVersion;
use arrow::ipc::reader::FileReader;
use arrow::ipc::writer::{FileWriter, IpcWriteOptions};
use ficant_application::ports::{EncodedFuturesDeliveryArtifact, FuturesDeliveryArtifactCodec};
use ficant_domain::analytics::{
    ABI_VERSION, AnalyticsError, DECIMAL_SCALE, ENGINE_ID, ENGINE_VERSION, FixedDecimal,
};
use ficant_domain::futures_delivery::{
    FUTURES_DELIVERY_ALGORITHM_ID, FUTURES_DELIVERY_ALGORITHM_VERSION,
    FUTURES_DELIVERY_ARTIFACT_CODEC_ID, FUTURES_DELIVERY_ARTIFACT_SCHEMA_ID,
    FUTURES_DELIVERY_CONVENTION_PROFILE, FuturesDeliverableInput, FuturesDeliveryBasketResult,
    FuturesDeliveryMeasures, FuturesDeliveryResult,
};
use ficant_domain::primitives::ContentHash;

const DECIMAL_PRECISION: u8 = 38;
const DECIMAL_SCALE_I8: i8 = 12;
const COLUMN_COUNT: usize = 27;
const _: () = assert!(DECIMAL_SCALE == 12);

#[derive(Clone, Copy, Debug, Default)]
pub struct ArrowFuturesDeliveryCodec;

impl FuturesDeliveryArtifactCodec for ArrowFuturesDeliveryCodec {
    fn encode(
        &self,
        result: &FuturesDeliveryBasketResult,
    ) -> Result<EncodedFuturesDeliveryArtifact, AnalyticsError> {
        let batch = encode_batch(result)?;
        let options = IpcWriteOptions::try_new(64, false, MetadataVersion::V5)
            .map_err(|_| AnalyticsError::Internal)?;
        let mut bytes = Vec::new();
        {
            let mut writer =
                FileWriter::try_new_with_options(&mut bytes, batch.schema().as_ref(), options)
                    .map_err(|_| AnalyticsError::Internal)?;
            writer.write(&batch).map_err(|_| AnalyticsError::Internal)?;
            writer.finish().map_err(|_| AnalyticsError::Internal)?;
        }
        let content_hash = ContentHash::digest(&bytes);
        EncodedFuturesDeliveryArtifact::new(bytes, content_hash)
            .map_err(|_| AnalyticsError::Internal)
    }

    fn decode(
        &self,
        bytes: &[u8],
        expected_inputs: &[FuturesDeliverableInput],
    ) -> Result<FuturesDeliveryBasketResult, AnalyticsError> {
        if expected_inputs.is_empty() {
            return Err(AnalyticsError::InvalidInput);
        }
        let mut reader = FileReader::try_new(Cursor::new(bytes), None)
            .map_err(|_| AnalyticsError::InvalidInput)?;
        if reader.schema().as_ref() != &artifact_schema() {
            return Err(AnalyticsError::InvalidInput);
        }
        let batch = reader
            .next()
            .ok_or(AnalyticsError::InvalidInput)?
            .map_err(|_| AnalyticsError::InvalidInput)?;
        if reader.next().is_some()
            || batch.num_rows() != expected_inputs.len()
            || batch.num_columns() != COLUMN_COUNT
        {
            return Err(AnalyticsError::InvalidInput);
        }
        let mut candidates = Vec::with_capacity(expected_inputs.len());
        let mut ctd_index = None;
        for (row, input) in expected_inputs.iter().enumerate() {
            validate_input_columns(&batch, row, input, expected_inputs.len())?;
            if boolean(&batch, 12, row)? && ctd_index.replace(row).is_some() {
                return Err(AnalyticsError::InvalidInput);
            }
            let measures = FuturesDeliveryMeasures::new(
                uint32(&batch, 13, row)?,
                uint32(&batch, 14, row)?,
                decimal(&batch, 15, row)?,
                decimal(&batch, 16, row)?,
                decimal(&batch, 17, row)?,
                decimal(&batch, 18, row)?,
                decimal(&batch, 19, row)?,
                decimal(&batch, 20, row)?,
                decimal(&batch, 21, row)?,
                decimal(&batch, 22, row)?,
                decimal(&batch, 23, row)?,
                decimal(&batch, 24, row)?,
                decimal(&batch, 25, row)?,
                decimal(&batch, 26, row)?,
            )
            .map_err(|_| AnalyticsError::InvalidInput)?;
            candidates.push(FuturesDeliveryResult::new(input.clone(), measures));
        }
        FuturesDeliveryBasketResult::new(candidates, ctd_index.ok_or(AnalyticsError::InvalidInput)?)
            .map_err(|_| AnalyticsError::InvalidInput)
    }
}

fn artifact_schema() -> Schema {
    Schema::new(vec![
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
    ])
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

fn encode_batch(result: &FuturesDeliveryBasketResult) -> Result<RecordBatch, AnalyticsError> {
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
    let columns: Vec<ArrayRef> = vec![
        text_array(row_count, FUTURES_DELIVERY_ARTIFACT_SCHEMA_ID),
        text_array(row_count, FUTURES_DELIVERY_ARTIFACT_CODEC_ID),
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
    RecordBatch::try_new(Arc::new(artifact_schema()), columns).map_err(|_| AnalyticsError::Internal)
}

fn validate_input_columns(
    batch: &RecordBatch,
    row: usize,
    input: &FuturesDeliverableInput,
    basket_size: usize,
) -> Result<(), AnalyticsError> {
    let checks = [
        string(batch, 0, row)? == FUTURES_DELIVERY_ARTIFACT_SCHEMA_ID,
        string(batch, 1, row)? == FUTURES_DELIVERY_ARTIFACT_CODEC_ID,
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
    if checks.into_iter().all(|value| value) {
        Ok(())
    } else {
        Err(AnalyticsError::InvalidInput)
    }
}

fn text_array(rows: usize, value: &str) -> ArrayRef {
    Arc::new(StringArray::from(vec![value; rows]))
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

fn string(batch: &RecordBatch, column: usize, row: usize) -> Result<&str, AnalyticsError> {
    let array = downcast::<StringArray>(batch.column(column))?;
    non_null(array, row)?;
    Ok(array.value(row))
}

fn uint32(batch: &RecordBatch, column: usize, row: usize) -> Result<u32, AnalyticsError> {
    primitive_value::<UInt32Type>(batch.column(column), row)
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
