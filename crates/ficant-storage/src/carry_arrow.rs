use std::io::Cursor;
use std::sync::Arc;

use arrow::array::{
    Array, ArrayRef, Date32Array, Decimal128Array, FixedSizeBinaryArray, PrimitiveArray,
    RecordBatch, StringArray, TimestampMicrosecondArray, UInt32Array, UInt64Array,
};
use arrow::datatypes::{
    ArrowPrimitiveType, DataType, Date32Type, Decimal128Type, Field, Schema,
    TimestampMicrosecondType, UInt32Type, UInt64Type,
};
use arrow::ipc::MetadataVersion;
use arrow::ipc::reader::FileReader;
use arrow::ipc::writer::{FileWriter, IpcWriteOptions};
use chrono::{NaiveDate, TimeDelta};
use ficant_application::ports::{CarryRollArtifactCodec, EncodedCarryRollArtifact};
use ficant_domain::analytics::{
    ABI_VERSION, AnalyticsError, DECIMAL_SCALE, ENGINE_ID, ENGINE_VERSION, FixedDecimal, utc_micros,
};
use ficant_domain::curves::{
    CARRY_ROLL_ALGORITHM_ID, CARRY_ROLL_ALGORITHM_VERSION, CARRY_ROLL_ARTIFACT_CODEC_ID,
    CARRY_ROLL_ARTIFACT_SCHEMA_ID, CARRY_ROLL_CONVENTION_PROFILE, CarryRollInput,
    CarryRollMeasures, CarryRollResult,
};
use ficant_domain::primitives::ContentHash;

const DECIMAL_PRECISION: u8 = 38;
const DECIMAL_SCALE_I8: i8 = 12;
const COLUMN_COUNT: usize = 37;
const _: () = assert!(DECIMAL_SCALE == 12);

#[derive(Clone, Copy, Debug, Default)]
pub struct ArrowCarryRollCodec;

impl CarryRollArtifactCodec for ArrowCarryRollCodec {
    fn encode(&self, result: &CarryRollResult) -> Result<EncodedCarryRollArtifact, AnalyticsError> {
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
        let hash = ContentHash::digest(&bytes);
        EncodedCarryRollArtifact::new(bytes, hash).map_err(|_| AnalyticsError::Internal)
    }

    fn decode(
        &self,
        bytes: &[u8],
        expected_input: &CarryRollInput,
    ) -> Result<CarryRollResult, AnalyticsError> {
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
        let measures = CarryRollMeasures::new(
            decimal(&batch, 28)?,
            decimal(&batch, 29)?,
            decimal(&batch, 30)?,
            decimal(&batch, 31)?,
            decimal(&batch, 32)?,
            decimal(&batch, 33)?,
            decimal(&batch, 34)?,
            decimal(&batch, 35)?,
            decimal(&batch, 36)?,
        )
        .map_err(|_| AnalyticsError::InvalidInput)?;
        Ok(CarryRollResult::new(expected_input.clone(), measures))
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
        binary_hash_field("input_fingerprint"),
        text_field("tenant_id"),
        text_field("owner_id"),
        text_field("bond_id"),
        Field::new("bond_version", DataType::UInt64, false),
        binary_hash_field("bond_content_hash"),
        text_field("rule_pack_id"),
        Field::new("rule_pack_version", DataType::UInt64, false),
        binary_hash_field("rule_pack_content_hash"),
        text_field("snapshot_id"),
        Field::new("snapshot_version", DataType::UInt64, false),
        binary_hash_field("snapshot_content_hash"),
        text_field("curve_snapshot_id"),
        Field::new("curve_snapshot_version", DataType::UInt64, false),
        binary_hash_field("curve_snapshot_content_hash"),
        Field::new(
            "valuation_at",
            DataType::Timestamp(arrow::datatypes::TimeUnit::Microsecond, Some("UTC".into())),
            false,
        ),
        text_field("market_timezone"),
        date_field("valuation_date"),
        date_field("initial_settlement"),
        date_field("horizon_settlement"),
        decimal_field("initial_yield"),
        decimal_field("rolled_yield"),
        decimal_field("initial_dirty_price"),
        decimal_field("horizon_dirty_at_initial_yield"),
        decimal_field("horizon_dirty_at_rolled_yield"),
        decimal_field("paid_cashflows"),
        decimal_field("carry"),
        decimal_field("roll_down"),
        decimal_field("total_return"),
    ])
}

fn text_field(name: &str) -> Field {
    Field::new(name, DataType::Utf8, false)
}

fn date_field(name: &str) -> Field {
    Field::new(name, DataType::Date32, false)
}

fn binary_hash_field(name: &str) -> Field {
    Field::new(name, DataType::FixedSizeBinary(32), false)
}

fn decimal_field(name: &str) -> Field {
    Field::new(
        name,
        DataType::Decimal128(DECIMAL_PRECISION, DECIMAL_SCALE_I8),
        false,
    )
}

fn encode_batch(result: &CarryRollResult) -> Result<RecordBatch, AnalyticsError> {
    let input = result.input();
    let curve = input.curve().curve_snapshot();
    let measures = result.measures();
    let columns: Vec<ArrayRef> = vec![
        text_array(CARRY_ROLL_ARTIFACT_SCHEMA_ID),
        text_array(CARRY_ROLL_ARTIFACT_CODEC_ID),
        text_array(ENGINE_ID),
        text_array(ENGINE_VERSION),
        text_array(CARRY_ROLL_ALGORITHM_ID),
        text_array(CARRY_ROLL_CONVENTION_PROFILE),
        Arc::new(UInt32Array::from(vec![CARRY_ROLL_ALGORITHM_VERSION])),
        Arc::new(UInt32Array::from(vec![ABI_VERSION])),
        hash_array(&input.fingerprint())?,
        text_array(input.owner().tenant_id().as_str()),
        text_array(input.owner().owner_id().as_str()),
        text_array(input.bond().version_ref().id().as_str()),
        Arc::new(UInt64Array::from(vec![
            input.bond().version_ref().version().get(),
        ])),
        hash_array(input.bond().content_hash())?,
        text_array(input.rule_pack().version_ref().id().as_str()),
        Arc::new(UInt64Array::from(vec![
            input.rule_pack().version_ref().version().get(),
        ])),
        hash_array(input.rule_pack().content_hash())?,
        text_array(input.snapshot().version_ref().id().as_str()),
        Arc::new(UInt64Array::from(vec![
            input.snapshot().version_ref().version().get(),
        ])),
        hash_array(input.snapshot().content_hash())?,
        text_array(curve.version_ref().id().as_str()),
        Arc::new(UInt64Array::from(vec![curve.version_ref().version().get()])),
        hash_array(curve.content_hash())?,
        Arc::new(
            TimestampMicrosecondArray::from(vec![utc_micros(input.valuation_at())])
                .with_timezone("UTC"),
        ),
        text_array(input.valuation_at().market_timezone()),
        date_array(input.valuation_at().local_trading_date())?,
        date_array(input.initial_settlement())?,
        date_array(input.horizon_settlement())?,
        decimal_array(measures.initial_yield())?,
        decimal_array(measures.rolled_yield())?,
        decimal_array(measures.initial_dirty_price())?,
        decimal_array(measures.horizon_dirty_at_initial_yield())?,
        decimal_array(measures.horizon_dirty_at_rolled_yield())?,
        decimal_array(measures.paid_cashflows())?,
        decimal_array(measures.carry())?,
        decimal_array(measures.roll_down())?,
        decimal_array(measures.total_return())?,
    ];
    RecordBatch::try_new(Arc::new(artifact_schema()), columns).map_err(|_| AnalyticsError::Internal)
}

fn validate_input_columns(
    batch: &RecordBatch,
    input: &CarryRollInput,
) -> Result<(), AnalyticsError> {
    let curve = input.curve().curve_snapshot();
    let checks = [
        string(batch, 0)? == CARRY_ROLL_ARTIFACT_SCHEMA_ID,
        string(batch, 1)? == CARRY_ROLL_ARTIFACT_CODEC_ID,
        string(batch, 2)? == ENGINE_ID,
        string(batch, 3)? == ENGINE_VERSION,
        string(batch, 4)? == CARRY_ROLL_ALGORITHM_ID,
        string(batch, 5)? == CARRY_ROLL_CONVENTION_PROFILE,
        uint32(batch, 6)? == CARRY_ROLL_ALGORITHM_VERSION,
        uint32(batch, 7)? == ABI_VERSION,
        binary(batch, 8)? == input.fingerprint().as_bytes(),
        string(batch, 9)? == input.owner().tenant_id().as_str(),
        string(batch, 10)? == input.owner().owner_id().as_str(),
        string(batch, 11)? == input.bond().version_ref().id().as_str(),
        uint64(batch, 12)? == input.bond().version_ref().version().get(),
        binary(batch, 13)? == input.bond().content_hash().as_bytes(),
        string(batch, 14)? == input.rule_pack().version_ref().id().as_str(),
        uint64(batch, 15)? == input.rule_pack().version_ref().version().get(),
        binary(batch, 16)? == input.rule_pack().content_hash().as_bytes(),
        string(batch, 17)? == input.snapshot().version_ref().id().as_str(),
        uint64(batch, 18)? == input.snapshot().version_ref().version().get(),
        binary(batch, 19)? == input.snapshot().content_hash().as_bytes(),
        string(batch, 20)? == curve.version_ref().id().as_str(),
        uint64(batch, 21)? == curve.version_ref().version().get(),
        binary(batch, 22)? == curve.content_hash().as_bytes(),
        timestamp(batch, 23)? == utc_micros(input.valuation_at()),
        string(batch, 24)? == input.valuation_at().market_timezone(),
        date(batch, 25)? == input.valuation_at().local_trading_date(),
        date(batch, 26)? == input.initial_settlement(),
        date(batch, 27)? == input.horizon_settlement(),
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

fn date_array(value: NaiveDate) -> Result<ArrayRef, AnalyticsError> {
    Ok(Arc::new(Date32Array::from(vec![epoch_days(value)?])))
}

fn decimal_array(value: FixedDecimal) -> Result<ArrayRef, AnalyticsError> {
    let array = Decimal128Array::from(vec![value.scaled()])
        .with_precision_and_scale(DECIMAL_PRECISION, DECIMAL_SCALE_I8)
        .map_err(|_| AnalyticsError::InvalidInput)?;
    Ok(Arc::new(array))
}

fn hash_array(value: &ContentHash) -> Result<ArrayRef, AnalyticsError> {
    FixedSizeBinaryArray::try_from_iter([value.as_bytes()].into_iter())
        .map(|array| Arc::new(array) as ArrayRef)
        .map_err(|_| AnalyticsError::Internal)
}

fn string(batch: &RecordBatch, index: usize) -> Result<&str, AnalyticsError> {
    let array = downcast::<StringArray>(batch.column(index))?;
    non_null(array, 0)?;
    Ok(array.value(0))
}

fn uint32(batch: &RecordBatch, index: usize) -> Result<u32, AnalyticsError> {
    primitive_value::<UInt32Type>(batch.column(index), 0)
}

fn uint64(batch: &RecordBatch, index: usize) -> Result<u64, AnalyticsError> {
    primitive_value::<UInt64Type>(batch.column(index), 0)
}

fn timestamp(batch: &RecordBatch, index: usize) -> Result<i64, AnalyticsError> {
    primitive_value::<TimestampMicrosecondType>(batch.column(index), 0)
}

fn date(batch: &RecordBatch, index: usize) -> Result<NaiveDate, AnalyticsError> {
    date_from_epoch_days(primitive_value::<Date32Type>(batch.column(index), 0)?)
}

fn decimal(batch: &RecordBatch, index: usize) -> Result<FixedDecimal, AnalyticsError> {
    primitive_value::<Decimal128Type>(batch.column(index), 0).map(FixedDecimal::from_scaled)
}

fn binary(batch: &RecordBatch, index: usize) -> Result<&[u8], AnalyticsError> {
    let array = downcast::<FixedSizeBinaryArray>(batch.column(index))?;
    non_null(array, 0)?;
    Ok(array.value(0))
}

fn primitive_value<T>(array: &ArrayRef, index: usize) -> Result<T::Native, AnalyticsError>
where
    T: ArrowPrimitiveType,
{
    let array = downcast::<PrimitiveArray<T>>(array)?;
    non_null(array, index)?;
    Ok(array.value(index))
}

fn downcast<T: Array + 'static>(array: &dyn Array) -> Result<&T, AnalyticsError> {
    array
        .as_any()
        .downcast_ref::<T>()
        .ok_or(AnalyticsError::InvalidInput)
}

fn non_null(array: &dyn Array, index: usize) -> Result<(), AnalyticsError> {
    if index >= array.len() || array.is_null(index) {
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
