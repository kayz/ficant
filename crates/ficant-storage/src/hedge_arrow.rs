use std::io::Cursor;
use std::sync::Arc;

use arrow::array::{
    Array, ArrayRef, Decimal128Array, FixedSizeBinaryArray, Int64Array, PrimitiveArray,
    RecordBatch, StringArray, UInt32Array,
};
use arrow::datatypes::{
    ArrowPrimitiveType, DataType, Decimal128Type, Field, Int64Type, Schema, UInt32Type,
};
use arrow::ipc::MetadataVersion;
use arrow::ipc::reader::FileReader;
use arrow::ipc::writer::{FileWriter, IpcWriteOptions};
use ficant_application::ports::{EncodedFuturesHedgeArtifact, FuturesHedgeArtifactCodec};
use ficant_domain::analytics::{
    ABI_VERSION, AnalyticsError, DECIMAL_SCALE, ENGINE_ID, ENGINE_VERSION, FixedDecimal,
};
use ficant_domain::futures_hedge::{
    FUTURES_HEDGE_ALGORITHM_ID, FUTURES_HEDGE_ALGORITHM_VERSION, FUTURES_HEDGE_ARTIFACT_CODEC_ID,
    FUTURES_HEDGE_ARTIFACT_SCHEMA_ID, FUTURES_HEDGE_CONVENTION_PROFILE, FuturesHedgeInput,
    FuturesHedgeMeasures, FuturesHedgeResult,
};
use ficant_domain::primitives::ContentHash;

const DECIMAL_PRECISION: u8 = 38;
const DECIMAL_SCALE_I8: i8 = 12;
const COLUMN_COUNT: usize = 19;
const _: () = assert!(DECIMAL_SCALE == 12);

#[derive(Clone, Copy, Debug, Default)]
pub struct ArrowFuturesHedgeCodec;

impl FuturesHedgeArtifactCodec for ArrowFuturesHedgeCodec {
    fn encode(
        &self,
        result: &FuturesHedgeResult,
    ) -> Result<EncodedFuturesHedgeArtifact, AnalyticsError> {
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
        EncodedFuturesHedgeArtifact::new(bytes, content_hash).map_err(|_| AnalyticsError::Internal)
    }

    fn decode(
        &self,
        bytes: &[u8],
        expected_input: &FuturesHedgeInput,
    ) -> Result<FuturesHedgeResult, AnalyticsError> {
        let mut reader = FileReader::try_new(Cursor::new(bytes), None)
            .map_err(|_| AnalyticsError::InvalidInput)?;
        if reader.schema().as_ref() != &artifact_schema() {
            return Err(AnalyticsError::InvalidInput);
        }
        let batch = reader
            .next()
            .ok_or(AnalyticsError::InvalidInput)?
            .map_err(|_| AnalyticsError::InvalidInput)?;
        if reader.next().is_some() || batch.num_rows() != 1 || batch.num_columns() != COLUMN_COUNT {
            return Err(AnalyticsError::InvalidInput);
        }
        validate_input_columns(&batch, expected_input)?;
        let measures = FuturesHedgeMeasures::new(
            decimal(&batch, 14)?,
            decimal(&batch, 15)?,
            int64(&batch, 16)?,
            decimal(&batch, 17)?,
            decimal(&batch, 18)?,
        )
        .map_err(|_| AnalyticsError::InvalidInput)?;
        Ok(FuturesHedgeResult::new(expected_input.clone(), measures))
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
        Field::new("input_fingerprint", DataType::FixedSizeBinary(32), false),
        Field::new("product", DataType::UInt32, false),
        decimal_field("target_dv01"),
        decimal_field("ctd_dv01_per_100"),
        decimal_field("conversion_factor"),
        decimal_field("contract_notional"),
        decimal_field("futures_contract_dv01"),
        decimal_field("raw_contracts"),
        Field::new("recommended_contracts", DataType::Int64, false),
        decimal_field("residual_dv01"),
        decimal_field("hedge_effectiveness"),
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

fn encode_batch(result: &FuturesHedgeResult) -> Result<RecordBatch, AnalyticsError> {
    let input = result.input();
    let measures = result.measures();
    let fingerprint = input.fingerprint();
    let columns: Vec<ArrayRef> = vec![
        text_array(FUTURES_HEDGE_ARTIFACT_SCHEMA_ID),
        text_array(FUTURES_HEDGE_ARTIFACT_CODEC_ID),
        text_array(ENGINE_ID),
        text_array(ENGINE_VERSION),
        text_array(FUTURES_HEDGE_ALGORITHM_ID),
        text_array(FUTURES_HEDGE_CONVENTION_PROFILE),
        Arc::new(UInt32Array::from(vec![FUTURES_HEDGE_ALGORITHM_VERSION])),
        Arc::new(UInt32Array::from(vec![ABI_VERSION])),
        hash_array(&fingerprint)?,
        Arc::new(UInt32Array::from(vec![input.product() as u32])),
        decimal_array(input.target_dv01())?,
        decimal_array(input.ctd_dv01_per_100())?,
        decimal_array(input.conversion_factor())?,
        decimal_array(input.contract_notional())?,
        decimal_array(measures.futures_contract_dv01())?,
        decimal_array(measures.raw_contracts())?,
        Arc::new(Int64Array::from(vec![measures.recommended_contracts()])),
        decimal_array(measures.residual_dv01())?,
        decimal_array(measures.hedge_effectiveness())?,
    ];
    RecordBatch::try_new(Arc::new(artifact_schema()), columns).map_err(|_| AnalyticsError::Internal)
}

fn validate_input_columns(
    batch: &RecordBatch,
    input: &FuturesHedgeInput,
) -> Result<(), AnalyticsError> {
    let checks = [
        string(batch, 0)? == FUTURES_HEDGE_ARTIFACT_SCHEMA_ID,
        string(batch, 1)? == FUTURES_HEDGE_ARTIFACT_CODEC_ID,
        string(batch, 2)? == ENGINE_ID,
        string(batch, 3)? == ENGINE_VERSION,
        string(batch, 4)? == FUTURES_HEDGE_ALGORITHM_ID,
        string(batch, 5)? == FUTURES_HEDGE_CONVENTION_PROFILE,
        uint32(batch, 6)? == FUTURES_HEDGE_ALGORITHM_VERSION,
        uint32(batch, 7)? == ABI_VERSION,
        binary(batch, 8)? == input.fingerprint().as_bytes(),
        uint32(batch, 9)? == input.product() as u32,
        decimal(batch, 10)? == input.target_dv01(),
        decimal(batch, 11)? == input.ctd_dv01_per_100(),
        decimal(batch, 12)? == input.conversion_factor(),
        decimal(batch, 13)? == input.contract_notional(),
    ];
    if checks.into_iter().all(|value| value) {
        Ok(())
    } else {
        Err(AnalyticsError::InvalidInput)
    }
}

fn text_array(value: &str) -> ArrayRef {
    Arc::new(StringArray::from(vec![value]))
}

fn decimal_array(value: FixedDecimal) -> Result<ArrayRef, AnalyticsError> {
    let array = Decimal128Array::from(vec![value.scaled()])
        .with_precision_and_scale(DECIMAL_PRECISION, DECIMAL_SCALE_I8)
        .map_err(|_| AnalyticsError::InvalidInput)?;
    Ok(Arc::new(array))
}

fn hash_array(value: &ContentHash) -> Result<ArrayRef, AnalyticsError> {
    FixedSizeBinaryArray::try_from_iter(std::iter::once(value.as_bytes()))
        .map(|array| Arc::new(array) as ArrayRef)
        .map_err(|_| AnalyticsError::Internal)
}

fn string(batch: &RecordBatch, column: usize) -> Result<&str, AnalyticsError> {
    let array = downcast::<StringArray>(batch.column(column))?;
    non_null(array)?;
    Ok(array.value(0))
}

fn uint32(batch: &RecordBatch, column: usize) -> Result<u32, AnalyticsError> {
    primitive_value::<UInt32Type>(batch.column(column))
}

fn int64(batch: &RecordBatch, column: usize) -> Result<i64, AnalyticsError> {
    primitive_value::<Int64Type>(batch.column(column))
}

fn decimal(batch: &RecordBatch, column: usize) -> Result<FixedDecimal, AnalyticsError> {
    primitive_value::<Decimal128Type>(batch.column(column)).map(FixedDecimal::from_scaled)
}

fn binary(batch: &RecordBatch, column: usize) -> Result<&[u8], AnalyticsError> {
    let array = downcast::<FixedSizeBinaryArray>(batch.column(column))?;
    non_null(array)?;
    Ok(array.value(0))
}

fn primitive_value<T>(array: &ArrayRef) -> Result<T::Native, AnalyticsError>
where
    T: ArrowPrimitiveType,
{
    let array = downcast::<PrimitiveArray<T>>(array)?;
    non_null(array)?;
    Ok(array.value(0))
}

fn downcast<T: Array + 'static>(array: &dyn Array) -> Result<&T, AnalyticsError> {
    array
        .as_any()
        .downcast_ref::<T>()
        .ok_or(AnalyticsError::InvalidInput)
}

fn non_null(array: &dyn Array) -> Result<(), AnalyticsError> {
    if array.len() != 1 || array.is_null(0) {
        Err(AnalyticsError::InvalidInput)
    } else {
        Ok(())
    }
}
