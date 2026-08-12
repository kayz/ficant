use std::io::Cursor;
use std::sync::Arc;

use arrow::array::{
    Array, ArrayRef, Date32Array, Decimal128Array, FixedSizeBinaryArray, ListArray, PrimitiveArray,
    RecordBatch, StringArray, StructArray, TimestampMicrosecondArray, UInt8Array, UInt32Array,
    UInt64Array,
};
use arrow::buffer::{OffsetBuffer, ScalarBuffer};
use arrow::datatypes::{
    ArrowPrimitiveType, Date32Type, Decimal128Type, TimestampMicrosecondType, UInt8Type,
    UInt32Type, UInt64Type,
};
use arrow::datatypes::{DataType, Field, Fields, Schema, TimeUnit};
use arrow::ipc::MetadataVersion;
use arrow::ipc::reader::FileReader;
use arrow::ipc::writer::{FileWriter, IpcWriteOptions};
use chrono::{NaiveDate, TimeDelta, Utc};
use ficant_application::ports::{
    BondAnalyticsArtifactCodec, BondAnalyticsArtifactFacts, EncodedBondAnalyticsArtifact,
};
use ficant_domain::analytics::AnalyticsObjectRef;
use ficant_domain::analytics::{
    ABI_VERSION, ALGORITHM_ID, ALGORITHM_VERSION, ARTIFACT_CODEC_ID, ARTIFACT_SCHEMA_ID,
    AnalyticsError, AnalyticsMeasures, AnalyticsMode, BondAnalyticsInput, BondAnalyticsResult,
    CONVENTION_PROFILE, CalendarRequirement, CalendarResolution, DECIMAL_SCALE, DerivedCashflow,
    ENGINE_ID, ENGINE_VERSION, FixedDecimal, MARKET_TIMEZONE, utc_micros,
};
use ficant_domain::primitives::{ContentHash, MarketTime, Ulid, Version, VersionRef};

const DECIMAL_PRECISION: u8 = 38;
const DECIMAL_SCALE_I8: i8 = 12;
const _: () = assert!(DECIMAL_SCALE == 12);

#[derive(Clone, Copy, Debug, Default)]
pub struct ArrowBondAnalyticsCodec;

impl BondAnalyticsArtifactCodec for ArrowBondAnalyticsCodec {
    fn encode(
        &self,
        result: &BondAnalyticsResult,
    ) -> Result<EncodedBondAnalyticsArtifact, AnalyticsError> {
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
        EncodedBondAnalyticsArtifact::new(bytes, hash).map_err(|_| AnalyticsError::Internal)
    }

    fn decode(
        &self,
        bytes: &[u8],
        expected_input: &BondAnalyticsInput,
    ) -> Result<BondAnalyticsResult, AnalyticsError> {
        let mut reader = FileReader::try_new(Cursor::new(bytes), None)
            .map_err(|_| AnalyticsError::InvalidInput)?;
        if reader.schema().as_ref() != &artifact_schema() {
            return Err(AnalyticsError::InvalidInput);
        }
        let batch = reader
            .next()
            .ok_or(AnalyticsError::InvalidInput)?
            .map_err(|_| AnalyticsError::InvalidInput)?;
        if reader.next().is_some() || batch.num_rows() != 1 || batch.num_columns() != 39 {
            return Err(AnalyticsError::InvalidInput);
        }
        validate_input_columns(&batch, expected_input)?;
        let resolution = match uint8(&batch, 12)? {
            1 => CalendarResolution::Exact,
            2 => CalendarResolution::ProvisionalWeekendOnly,
            _ => return Err(AnalyticsError::InvalidInput),
        };
        let cashflows = decode_cashflows(&batch)?;
        let measures = AnalyticsMeasures::new(
            decimal(&batch, 31)?,
            decimal(&batch, 32)?,
            decimal(&batch, 34)?,
            decimal(&batch, 35)?,
            decimal(&batch, 36)?,
            decimal(&batch, 37)?,
            decimal(&batch, 38)?,
        )
        .map_err(|_| AnalyticsError::InvalidInput)?;
        if measures.dirty_price() != decimal(&batch, 33)? {
            return Err(AnalyticsError::InvalidInput);
        }
        BondAnalyticsResult::new(expected_input.clone(), resolution, cashflows, measures)
            .map_err(|_| AnalyticsError::InvalidInput)
    }

    fn decode_facts(&self, bytes: &[u8]) -> Result<BondAnalyticsArtifactFacts, AnalyticsError> {
        let batch = read_single_batch(bytes)?;
        let headers_match = string(&batch, 0)? == ARTIFACT_SCHEMA_ID
            && string(&batch, 1)? == ARTIFACT_CODEC_ID
            && string(&batch, 2)? == ENGINE_ID
            && string(&batch, 3)? == ENGINE_VERSION
            && string(&batch, 4)? == ALGORITHM_ID
            && string(&batch, 5)? == CONVENTION_PROFILE
            && uint32(&batch, 7)? == ALGORITHM_VERSION
            && uint32(&batch, 8)? == ABI_VERSION
            && string(&batch, 15)? == MARKET_TIMEZONE;
        if !headers_match {
            return Err(AnalyticsError::InvalidInput);
        }
        let calendar_id = string(&batch, 6)?;
        if calendar_id.trim().is_empty() || calendar_id != calendar_id.trim() {
            return Err(AnalyticsError::InvalidInput);
        }
        Version::new(uint64(&batch, 9)?).map_err(|_| AnalyticsError::InvalidInput)?;
        ContentHash::from_bytes(binary(&batch, 10)?).map_err(|_| AnalyticsError::InvalidInput)?;
        let requirement = match uint8(&batch, 11)? {
            1 => CalendarRequirement::ReferenceReplay,
            2 => CalendarRequirement::ExactMarket,
            _ => return Err(AnalyticsError::InvalidInput),
        };
        let resolution = match uint8(&batch, 12)? {
            1 => CalendarResolution::Exact,
            2 => CalendarResolution::ProvisionalWeekendOnly,
            _ => return Err(AnalyticsError::InvalidInput),
        };
        if requirement == CalendarRequirement::ExactMarket
            && resolution != CalendarResolution::Exact
        {
            return Err(AnalyticsError::InvalidInput);
        }
        if date(&batch, 13)? > date(&batch, 14)? {
            return Err(AnalyticsError::InvalidInput);
        }
        let instant = chrono::DateTime::<Utc>::from_timestamp_micros(timestamp(&batch, 16)?)
            .ok_or(AnalyticsError::InvalidInput)?;
        let valuation_at = decode_market_time(instant)?;
        date(&batch, 17)?;
        match uint8(&batch, 18)? {
            1 => AnalyticsMode::YieldIn,
            2 => AnalyticsMode::PriceIn,
            _ => return Err(AnalyticsError::InvalidInput),
        };
        if !decimal(&batch, 19)?.is_positive() || !decimal(&batch, 29)?.is_positive() {
            return Err(AnalyticsError::InvalidInput);
        }
        let cashflows = decode_cashflows(&batch)?;
        if cashflows.is_empty()
            || cashflows.iter().enumerate().any(|(index, cashflow)| {
                cashflow.sequence() != u32::try_from(index + 1).unwrap_or(u32::MAX)
                    || (index > 0 && cashflows[index - 1].payment_date() > cashflow.payment_date())
            })
        {
            return Err(AnalyticsError::InvalidInput);
        }
        let measures = AnalyticsMeasures::new(
            decimal(&batch, 31)?,
            decimal(&batch, 32)?,
            decimal(&batch, 34)?,
            decimal(&batch, 35)?,
            decimal(&batch, 36)?,
            decimal(&batch, 37)?,
            decimal(&batch, 38)?,
        )
        .map_err(|_| AnalyticsError::InvalidInput)?;
        if measures.dirty_price() != decimal(&batch, 33)? {
            return Err(AnalyticsError::InvalidInput);
        }
        Ok(BondAnalyticsArtifactFacts::new(
            valuation_at,
            object_ref(&batch, 20, 21, 22)?,
            object_ref(&batch, 23, 24, 25)?,
            object_ref(&batch, 26, 27, 28)?,
            measures.dv01(),
        ))
    }
}

fn decode_market_time(instant: chrono::DateTime<Utc>) -> Result<MarketTime, AnalyticsError> {
    for delta in [-1_i64, 0, 1] {
        let Some(local_date) = instant
            .date_naive()
            .checked_add_signed(TimeDelta::days(delta))
        else {
            continue;
        };
        if let Ok(value) = MarketTime::new(instant, MARKET_TIMEZONE, local_date) {
            return Ok(value);
        }
    }
    Err(AnalyticsError::InvalidInput)
}

fn read_single_batch(bytes: &[u8]) -> Result<RecordBatch, AnalyticsError> {
    let mut reader =
        FileReader::try_new(Cursor::new(bytes), None).map_err(|_| AnalyticsError::InvalidInput)?;
    if reader.schema().as_ref() != &artifact_schema() {
        return Err(AnalyticsError::InvalidInput);
    }
    let batch = reader
        .next()
        .ok_or(AnalyticsError::InvalidInput)?
        .map_err(|_| AnalyticsError::InvalidInput)?;
    if reader.next().is_some() || batch.num_rows() != 1 || batch.num_columns() != 39 {
        return Err(AnalyticsError::InvalidInput);
    }
    Ok(batch)
}

fn object_ref(
    batch: &RecordBatch,
    id_column: usize,
    version_column: usize,
    hash_column: usize,
) -> Result<AnalyticsObjectRef, AnalyticsError> {
    let id = Ulid::new(string(batch, id_column)?).map_err(|_| AnalyticsError::InvalidInput)?;
    let version =
        Version::new(uint64(batch, version_column)?).map_err(|_| AnalyticsError::InvalidInput)?;
    let hash = ContentHash::from_bytes(binary(batch, hash_column)?)
        .map_err(|_| AnalyticsError::InvalidInput)?;
    Ok(AnalyticsObjectRef::new(VersionRef::new(id, version), hash))
}

fn artifact_schema() -> Schema {
    let cashflow_fields = cashflow_fields();
    let cashflow_item = Field::new("item", DataType::Struct(cashflow_fields), false);
    Schema::new(vec![
        text_field("schema_id"),
        text_field("codec_id"),
        text_field("engine_id"),
        text_field("engine_version"),
        text_field("algorithm_id"),
        text_field("convention_profile"),
        text_field("calendar_id"),
        Field::new("algorithm_version", DataType::UInt32, false),
        Field::new("abi_version", DataType::UInt32, false),
        Field::new("calendar_version", DataType::UInt64, false),
        binary_hash_field("calendar_content_hash"),
        Field::new("calendar_requirement", DataType::UInt8, false),
        Field::new("calendar_resolution", DataType::UInt8, false),
        date_field("calendar_coverage_start"),
        date_field("calendar_coverage_end"),
        text_field("market_timezone"),
        Field::new(
            "valuation_at",
            DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            false,
        ),
        date_field("settlement_date"),
        Field::new("input_mode", DataType::UInt8, false),
        decimal_field("input_value"),
        text_field("bond_id"),
        Field::new("bond_version", DataType::UInt64, false),
        binary_hash_field("bond_content_hash"),
        text_field("rule_pack_id"),
        Field::new("rule_pack_version", DataType::UInt64, false),
        binary_hash_field("rule_pack_content_hash"),
        text_field("snapshot_id"),
        Field::new("snapshot_version", DataType::UInt64, false),
        binary_hash_field("snapshot_content_hash"),
        decimal_field("face_amount"),
        Field::new("cashflows", DataType::List(Arc::new(cashflow_item)), false),
        decimal_field("accrued_interest"),
        decimal_field("clean_price"),
        decimal_field("dirty_price"),
        decimal_field("yield_to_maturity"),
        decimal_field("macaulay_duration"),
        decimal_field("modified_duration"),
        decimal_field("convexity"),
        decimal_field("dv01"),
    ])
}

fn cashflow_fields() -> Fields {
    Fields::from(vec![
        Field::new("sequence", DataType::UInt32, false),
        date_field("nominal_date"),
        date_field("payment_date"),
        decimal_field("coupon"),
        decimal_field("principal"),
        decimal_field("total"),
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

fn encode_batch(result: &BondAnalyticsResult) -> Result<RecordBatch, AnalyticsError> {
    let input = result.input();
    let measures = result.measures();
    let calendar = input.calendar();
    let columns: Vec<ArrayRef> = vec![
        text_array(ARTIFACT_SCHEMA_ID),
        text_array(ARTIFACT_CODEC_ID),
        text_array(ENGINE_ID),
        text_array(ENGINE_VERSION),
        text_array(ALGORITHM_ID),
        text_array(CONVENTION_PROFILE),
        text_array(calendar.id()),
        Arc::new(UInt32Array::from(vec![ALGORITHM_VERSION])),
        Arc::new(UInt32Array::from(vec![ABI_VERSION])),
        Arc::new(UInt64Array::from(vec![calendar.version().get()])),
        hash_array(calendar.content_hash())?,
        Arc::new(UInt8Array::from(vec![input.calendar_requirement() as u8])),
        Arc::new(UInt8Array::from(vec![result.calendar_resolution() as u8])),
        date_array(calendar.coverage_start())?,
        date_array(calendar.coverage_end())?,
        text_array(MARKET_TIMEZONE),
        Arc::new(
            TimestampMicrosecondArray::from(vec![utc_micros(input.valuation_at())])
                .with_timezone("UTC"),
        ),
        date_array(input.settlement_date())?,
        Arc::new(UInt8Array::from(vec![input.mode() as u8])),
        decimal_array(input.input_value())?,
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
        decimal_array(input.terms().face_amount())?,
        cashflow_array(result.cashflows())?,
        decimal_array(measures.accrued_interest())?,
        decimal_array(measures.clean_price())?,
        decimal_array(measures.dirty_price())?,
        decimal_array(measures.yield_to_maturity())?,
        decimal_array(measures.macaulay_duration())?,
        decimal_array(measures.modified_duration())?,
        decimal_array(measures.convexity())?,
        decimal_array(measures.dv01())?,
    ];
    RecordBatch::try_new(Arc::new(artifact_schema()), columns).map_err(|_| AnalyticsError::Internal)
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

fn cashflow_array(cashflows: &[DerivedCashflow]) -> Result<ArrayRef, AnalyticsError> {
    let sequence = Arc::new(UInt32Array::from_iter_values(
        cashflows.iter().map(DerivedCashflow::sequence),
    )) as ArrayRef;
    let nominal = Arc::new(Date32Array::from_iter_values(
        cashflows
            .iter()
            .map(|value| epoch_days(value.nominal_date()))
            .collect::<Result<Vec<_>, _>>()?,
    )) as ArrayRef;
    let payment = Arc::new(Date32Array::from_iter_values(
        cashflows
            .iter()
            .map(|value| epoch_days(value.payment_date()))
            .collect::<Result<Vec<_>, _>>()?,
    )) as ArrayRef;
    let coupon = decimal_values(cashflows.iter().map(DerivedCashflow::coupon))?;
    let principal = decimal_values(cashflows.iter().map(DerivedCashflow::principal))?;
    let total = decimal_values(cashflows.iter().map(DerivedCashflow::total))?;
    let values = StructArray::new(
        cashflow_fields(),
        vec![sequence, nominal, payment, coupon, principal, total],
        None,
    );
    let length = i32::try_from(cashflows.len()).map_err(|_| AnalyticsError::InvalidInput)?;
    let offsets = OffsetBuffer::new(ScalarBuffer::from(vec![0_i32, length]));
    let field = match artifact_schema().field(30).data_type() {
        DataType::List(field) => Arc::clone(field),
        _ => return Err(AnalyticsError::Internal),
    };
    Ok(Arc::new(ListArray::new(
        field,
        offsets,
        Arc::new(values),
        None,
    )))
}

fn decimal_values(values: impl Iterator<Item = FixedDecimal>) -> Result<ArrayRef, AnalyticsError> {
    Decimal128Array::from_iter_values(values.map(FixedDecimal::scaled))
        .with_precision_and_scale(DECIMAL_PRECISION, DECIMAL_SCALE_I8)
        .map(|array| Arc::new(array) as ArrayRef)
        .map_err(|_| AnalyticsError::InvalidInput)
}

fn validate_input_columns(
    batch: &RecordBatch,
    input: &BondAnalyticsInput,
) -> Result<(), AnalyticsError> {
    let calendar = input.calendar();
    let checks = [
        string(batch, 0)? == ARTIFACT_SCHEMA_ID,
        string(batch, 1)? == ARTIFACT_CODEC_ID,
        string(batch, 2)? == ENGINE_ID,
        string(batch, 3)? == ENGINE_VERSION,
        string(batch, 4)? == ALGORITHM_ID,
        string(batch, 5)? == CONVENTION_PROFILE,
        string(batch, 6)? == calendar.id(),
        uint32(batch, 7)? == ALGORITHM_VERSION,
        uint32(batch, 8)? == ABI_VERSION,
        uint64(batch, 9)? == calendar.version().get(),
        binary(batch, 10)? == calendar.content_hash().as_bytes(),
        uint8(batch, 11)? == input.calendar_requirement() as u8,
        date(batch, 13)? == calendar.coverage_start(),
        date(batch, 14)? == calendar.coverage_end(),
        string(batch, 15)? == MARKET_TIMEZONE,
        timestamp(batch, 16)? == utc_micros(input.valuation_at()),
        date(batch, 17)? == input.settlement_date(),
        uint8(batch, 18)? == input.mode() as u8,
        decimal(batch, 19)? == input.input_value(),
        string(batch, 20)? == input.bond().version_ref().id().as_str(),
        uint64(batch, 21)? == input.bond().version_ref().version().get(),
        binary(batch, 22)? == input.bond().content_hash().as_bytes(),
        string(batch, 23)? == input.rule_pack().version_ref().id().as_str(),
        uint64(batch, 24)? == input.rule_pack().version_ref().version().get(),
        binary(batch, 25)? == input.rule_pack().content_hash().as_bytes(),
        string(batch, 26)? == input.snapshot().version_ref().id().as_str(),
        uint64(batch, 27)? == input.snapshot().version_ref().version().get(),
        binary(batch, 28)? == input.snapshot().content_hash().as_bytes(),
        decimal(batch, 29)? == input.terms().face_amount(),
    ];
    if checks.into_iter().all(|value| value) {
        Ok(())
    } else {
        Err(AnalyticsError::InvalidInput)
    }
}

fn decode_cashflows(batch: &RecordBatch) -> Result<Vec<DerivedCashflow>, AnalyticsError> {
    let list = downcast::<ListArray>(batch.column(30))?;
    if list.is_null(0) {
        return Err(AnalyticsError::InvalidInput);
    }
    let values = list.value(0);
    let values = downcast::<StructArray>(&values)?;
    (0..values.len())
        .map(|index| {
            DerivedCashflow::new(
                primitive_value::<UInt32Type>(values.column(0), index)?,
                date_from_epoch_days(primitive_value::<Date32Type>(values.column(1), index)?)?,
                date_from_epoch_days(primitive_value::<Date32Type>(values.column(2), index)?)?,
                FixedDecimal::from_scaled(primitive_value::<Decimal128Type>(
                    values.column(3),
                    index,
                )?),
                FixedDecimal::from_scaled(primitive_value::<Decimal128Type>(
                    values.column(4),
                    index,
                )?),
                FixedDecimal::from_scaled(primitive_value::<Decimal128Type>(
                    values.column(5),
                    index,
                )?),
            )
            .map_err(|_| AnalyticsError::InvalidInput)
        })
        .collect()
}

fn string(batch: &RecordBatch, index: usize) -> Result<&str, AnalyticsError> {
    let array = downcast::<StringArray>(batch.column(index))?;
    non_null(array, 0)?;
    Ok(array.value(0))
}

fn uint8(batch: &RecordBatch, index: usize) -> Result<u8, AnalyticsError> {
    primitive_value::<UInt8Type>(batch.column(index), 0)
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
