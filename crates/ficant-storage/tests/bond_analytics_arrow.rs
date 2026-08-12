use std::io::Cursor;
use std::sync::{Arc, Mutex};

use arrow::array::{ArrayRef, RecordBatch, StringArray, UInt64Array};
use arrow::datatypes::{Field, Schema};
use arrow::ipc::MetadataVersion;
use arrow::ipc::reader::FileReader;
use arrow::ipc::writer::{FileWriter, IpcWriteOptions};
use async_trait::async_trait;
use chrono::{NaiveDate, TimeZone, Utc};
use ficant_application::ports::{
    AccessScope, ApplicationResult, ArtifactRepository, BeginBlobStage, BlobStore,
    BondAnalyticsArtifactCodec, BondAnalyticsEngine, IdempotencyKey, IntegrityEvent,
    IntegrityEventSink, IntegrityFailureReason, PublishArtifact, RequiredVerifiedBlobRead,
    SafeTraceContext, StagedBlobRef, VerifiedBlobPayload, VerifiedBlobReader, VerifiedBlobRef,
    VerifyBlobStage,
};
use ficant_application::{
    ApplicationError, ApplicationErrorCategory, PublishBondAnalytics, ReplayBondAnalytics,
};
use ficant_domain::analytics::{
    AnalyticsError, AnalyticsMode, AnalyticsObjectRef, BondAnalyticsInput, BondTerms,
    BusinessDayConvention, CalendarBinding, CalendarRequirement, CouponFrequency,
    DayCountConvention, FixedDecimal,
};
use ficant_domain::primitives::{ContentHash, MarketTime, OwnerRef, Ulid, Version, VersionRef};
use ficant_domain::research::Artifact;
use ficant_fixed_income_native::NativeBondAnalyticsEngine;
use ficant_storage::analytics_arrow::ArrowBondAnalyticsCodec;

#[test]
fn native_result_round_trips_through_deterministic_arrow_file() {
    let input = analytics_input(13_000_000_000);
    let result = NativeBondAnalyticsEngine
        .calculate(&input)
        .expect("frozen native fixture must calculate");
    assert_eq!(result.cashflows().len(), 2);
    assert_eq!(result.cashflows()[0].sequence(), 1);
    assert_eq!(result.cashflows()[1].sequence(), 2);

    let codec = ArrowBondAnalyticsCodec;
    let first = codec.encode(&result).expect("result must encode");
    let second = codec.encode(&result).expect("same result must re-encode");
    assert_eq!(
        hex(first.content_hash().as_bytes()),
        "0d74da243ddd828afd47dfc4e26fc9615b3e62525dc52b646ef1440f17959ef6"
    );
    assert_eq!(first.bytes(), second.bytes());
    assert_eq!(first.content_hash(), second.content_hash());
    assert_eq!(first.size(), second.size());

    let decoded = codec
        .decode(first.bytes(), &input)
        .expect("exact input must decode");
    assert_eq!(decoded, result);
    let facts = codec
        .decode_facts(first.bytes())
        .expect("verified payload must project self-describing facts");
    assert_eq!(facts.valuation_at(), input.valuation_at());
    assert_eq!(facts.bond(), input.bond());
    assert_eq!(facts.rule_pack(), input.rule_pack());
    assert_eq!(facts.snapshot(), input.snapshot());
    assert_eq!(facts.dv01(), result.measures().dv01());

    let drifted_input = analytics_input(14_000_000_000);
    assert!(codec.decode(first.bytes(), &drifted_input).is_err());
}

#[test]
fn facts_projection_fails_closed_on_schema_header_and_identity_tamper() {
    let input = analytics_input(13_000_000_000);
    let result = NativeBondAnalyticsEngine.calculate(&input).unwrap();
    let codec = ArrowBondAnalyticsCodec;
    let encoded = codec.encode(&result).unwrap();
    let batch = read_arrow_batch(encoded.bytes());

    let schema_drift = rename_field(&batch, 38, "untrusted_dv01");
    assert_eq!(
        codec.decode_facts(&write_arrow_batch(&schema_drift)),
        Err(AnalyticsError::InvalidInput)
    );

    let header_tamper = replace_column(
        &batch,
        2,
        Arc::new(StringArray::from(vec!["untrusted-engine"])) as ArrayRef,
    );
    assert_eq!(
        codec.decode_facts(&write_arrow_batch(&header_tamper)),
        Err(AnalyticsError::InvalidInput)
    );

    let invalid_bond_version =
        replace_column(&batch, 21, Arc::new(UInt64Array::from(vec![0])) as ArrayRef);
    assert_eq!(
        codec.decode_facts(&write_arrow_batch(&invalid_bond_version)),
        Err(AnalyticsError::InvalidInput)
    );

    let mut truncated = encoded.bytes().to_vec();
    truncated.truncate(truncated.len() / 2);
    assert_eq!(
        codec.decode_facts(&truncated),
        Err(AnalyticsError::InvalidInput)
    );
}

#[tokio::test]
async fn publish_and_replay_use_verified_artifact_lifecycle() {
    let input = analytics_input(13_000_000_000);
    let backend = MemoryBackend::default();
    let scope = AccessScope::new(
        input.owner().tenant_id().clone(),
        id('G'),
        vec![input.owner().owner_id().clone()],
    )
    .unwrap();
    let codec = ArrowBondAnalyticsCodec;
    let publish = PublishBondAnalytics::new(&NativeBondAnalyticsEngine, &codec, &backend, &backend);
    let artifact = publish
        .execute(
            scope.clone(),
            id('H'),
            &input,
            IdempotencyKey::new("analytics-publish-1").unwrap(),
        )
        .await
        .expect("verified artifact must publish");
    assert_eq!(artifact.id(), &id('H'));

    let replay = ReplayBondAnalytics::new(
        &NativeBondAnalyticsEngine,
        &codec,
        &backend,
        &backend,
        &backend,
    )
    .execute(
        &scope,
        id('H'),
        &input,
        SafeTraceContext::new("0123456789abcdef0123456789abcdef").unwrap(),
    )
    .await
    .expect("verified payload must replay byte-for-byte");
    assert_eq!(replay.artifact(), &artifact);
    assert_eq!(replay.stored(), replay.recalculated());
    assert_eq!(backend.state.lock().unwrap().integrity_events, 0);
}

#[tokio::test]
async fn failed_append_discards_private_stage() {
    let input = analytics_input(13_000_000_000);
    let backend = MemoryBackend::default();
    backend.state.lock().unwrap().fail_append = true;
    let scope = AccessScope::new(
        input.owner().tenant_id().clone(),
        id('G'),
        vec![input.owner().owner_id().clone()],
    )
    .unwrap();
    let error = PublishBondAnalytics::new(
        &NativeBondAnalyticsEngine,
        &ArrowBondAnalyticsCodec,
        &backend,
        &backend,
    )
    .execute(
        scope,
        id('H'),
        &input,
        IdempotencyKey::new("analytics-publish-failure").unwrap(),
    )
    .await
    .expect_err("append failure must stop publication");
    assert_eq!(
        error.category(),
        ApplicationErrorCategory::StorageUnavailable
    );
    let state = backend.state.lock().unwrap();
    assert!(state.artifact.is_none());
    assert!(state.staged.is_none());
    assert_eq!(state.discard_count, 1);
}

fn analytics_input(input_value: i128) -> BondAnalyticsInput {
    let version = Version::new(1).unwrap();
    let owner = OwnerRef::new(id('A'), id('B'));
    let reference = |suffix, seed| {
        AnalyticsObjectRef::new(
            VersionRef::new(id(suffix), version),
            ContentHash::from_bytes(&[seed; 32]).unwrap(),
        )
    };
    let valuation_at = Utc.with_ymd_and_hms(2026, 7, 13, 0, 0, 0).unwrap();
    let valuation_at = MarketTime::new(
        valuation_at,
        "Asia/Shanghai",
        NaiveDate::from_ymd_opt(2026, 7, 13).unwrap(),
    )
    .unwrap();
    let calendar = CalendarBinding::new(
        "cgb-reference-calendar-v1",
        version,
        ContentHash::from_bytes(&[4; 32]).unwrap(),
        NaiveDate::from_ymd_opt(1970, 1, 1).unwrap(),
        NaiveDate::from_ymd_opt(1970, 1, 1).unwrap(),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let terms = BondTerms::new(
        NaiveDate::from_ymd_opt(2026, 5, 15).unwrap(),
        NaiveDate::from_ymd_opt(2028, 5, 15).unwrap(),
        CouponFrequency::Annual,
        DayCountConvention::ActActBondIsma,
        BusinessDayConvention::Following,
        FixedDecimal::from_scaled(13_000_000_000),
        FixedDecimal::from_scaled(100_000_000_000_000),
    )
    .unwrap();
    BondAnalyticsInput::new(
        owner,
        reference('C', 1),
        reference('D', 2),
        reference('E', 3),
        valuation_at,
        NaiveDate::from_ymd_opt(2026, 7, 14).unwrap(),
        CalendarRequirement::ReferenceReplay,
        calendar,
        terms,
        AnalyticsMode::YieldIn,
        FixedDecimal::from_scaled(input_value),
    )
    .unwrap()
}

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut value, byte| {
        write!(value, "{byte:02x}").unwrap();
        value
    })
}

fn read_arrow_batch(bytes: &[u8]) -> RecordBatch {
    let mut reader = FileReader::try_new(Cursor::new(bytes), None).unwrap();
    let batch = reader.next().unwrap().unwrap();
    assert!(reader.next().is_none());
    batch
}

fn write_arrow_batch(batch: &RecordBatch) -> Vec<u8> {
    let options = IpcWriteOptions::try_new(64, false, MetadataVersion::V5).unwrap();
    let mut bytes = Vec::new();
    {
        let mut writer =
            FileWriter::try_new_with_options(&mut bytes, batch.schema().as_ref(), options).unwrap();
        writer.write(batch).unwrap();
        writer.finish().unwrap();
    }
    bytes
}

fn replace_column(batch: &RecordBatch, index: usize, value: ArrayRef) -> RecordBatch {
    let mut columns = batch.columns().to_vec();
    columns[index] = value;
    RecordBatch::try_new(batch.schema(), columns).unwrap()
}

fn rename_field(batch: &RecordBatch, index: usize, name: &str) -> RecordBatch {
    let current = batch.schema();
    let mut fields = current
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect::<Vec<_>>();
    let field = &fields[index];
    fields[index] = Field::new(name, field.data_type().clone(), field.is_nullable());
    let schema = Arc::new(Schema::new_with_metadata(
        fields,
        current.metadata().clone(),
    ));
    RecordBatch::try_new(schema, batch.columns().to_vec()).unwrap()
}

#[derive(Default)]
struct MemoryBackend {
    state: Arc<Mutex<MemoryState>>,
}

#[derive(Default)]
struct MemoryState {
    staged: Option<Vec<u8>>,
    promoted: Option<Vec<u8>>,
    artifact: Option<Artifact>,
    fail_append: bool,
    discard_count: usize,
    integrity_events: usize,
}

#[async_trait]
impl BlobStore for MemoryBackend {
    async fn begin_stage(&self, command: BeginBlobStage) -> ApplicationResult<StagedBlobRef> {
        self.state.lock().unwrap().staged = Some(Vec::new());
        Ok(StagedBlobRef::new(id('F'), command.owner().clone()))
    }

    async fn append_chunk(
        &self,
        _scope: &AccessScope,
        _staged: &StagedBlobRef,
        chunk: Vec<u8>,
    ) -> ApplicationResult<()> {
        let mut state = self.state.lock().unwrap();
        if state.fail_append {
            return Err(ApplicationError::new(
                ApplicationErrorCategory::StorageUnavailable,
                true,
            ));
        }
        state.staged.as_mut().unwrap().extend(chunk);
        Ok(())
    }

    async fn verify_and_promote(
        &self,
        command: VerifyBlobStage,
    ) -> ApplicationResult<VerifiedBlobRef> {
        let mut state = self.state.lock().unwrap();
        let bytes = state.staged.take().unwrap();
        assert_eq!(ContentHash::digest(&bytes), *command.expected_hash());
        assert_eq!(bytes.len() as u64, command.expected_size());
        state.promoted = Some(bytes);
        VerifiedBlobRef::new(command.expected_hash().clone(), command.expected_size())
    }

    async fn discard_stage(
        &self,
        _scope: &AccessScope,
        _staged: &StagedBlobRef,
    ) -> ApplicationResult<()> {
        let mut state = self.state.lock().unwrap();
        state.staged = None;
        state.discard_count += 1;
        Ok(())
    }
}

#[async_trait]
impl ArtifactRepository for MemoryBackend {
    async fn publish_verified_blob(&self, command: PublishArtifact) -> ApplicationResult<Artifact> {
        let artifact = command.artifact().clone();
        self.state.lock().unwrap().artifact = Some(artifact.clone());
        Ok(artifact)
    }

    async fn get_metadata(
        &self,
        _scope: &AccessScope,
        artifact_id: Ulid,
    ) -> ApplicationResult<Option<Artifact>> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .artifact
            .clone()
            .filter(|artifact| artifact.id() == &artifact_id))
    }
}

#[async_trait]
impl VerifiedBlobReader for MemoryBackend {
    async fn read_required(
        &self,
        request: &RequiredVerifiedBlobRead,
        sink: &dyn IntegrityEventSink,
    ) -> ApplicationResult<VerifiedBlobPayload> {
        let bytes = self.state.lock().unwrap().promoted.clone();
        match bytes {
            Some(bytes) => request.verify_bytes(sink, bytes).await,
            None => Err(request
                .fail_integrity(sink, IntegrityFailureReason::Missing)
                .await),
        }
    }
}

#[async_trait]
impl IntegrityEventSink for MemoryBackend {
    async fn emit(&self, _event: IntegrityEvent) -> ApplicationResult<()> {
        self.state.lock().unwrap().integrity_events += 1;
        Ok(())
    }
}
