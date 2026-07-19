use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{NaiveDate, TimeZone, Utc};
use ficant_application::ports::{
    AccessScope, ApplicationResult, ArtifactRepository, BeginBlobStage, BlobStore,
    CarryRollArtifactCodec, IdempotencyKey, IntegrityEvent, IntegrityEventSink,
    IntegrityFailureReason, PublishArtifact, RequiredVerifiedBlobRead, SafeTraceContext,
    StagedBlobRef, VerifiedBlobPayload, VerifiedBlobReader, VerifiedBlobRef, VerifyBlobStage,
};
use ficant_application::{
    ApplicationError, ApplicationErrorCategory, PublishCarryRoll, ReplayCarryRoll,
};
use ficant_domain::analytics::{
    AnalyticsObjectRef, BondTerms, BusinessDayConvention, CalendarBinding, CalendarRequirement,
    CouponFrequency, DayCountConvention, FixedDecimal,
};
use ficant_domain::curves::{
    CarryRollInput, YieldCurveBinding, YieldCurveInterpolation, YieldCurveNode,
};
use ficant_domain::primitives::{ContentHash, MarketTime, OwnerRef, Ulid, Version, VersionRef};
use ficant_domain::research::Artifact;
use ficant_fixed_income_native::NativeCarryRollEngine;
use ficant_storage::carry_arrow::ArrowCarryRollCodec;

#[test]
fn native_result_round_trips_through_deterministic_arrow_file() {
    let input = carry_input(0);
    let result = ficant_application::CalculateCarryRoll::new(&NativeCarryRollEngine)
        .execute(&input)
        .expect("frozen native fixture must calculate");
    let codec = ArrowCarryRollCodec;
    let first = codec.encode(&result).expect("result must encode");
    let second = codec.encode(&result).expect("same result must re-encode");
    assert_eq!(
        hex(first.content_hash().as_bytes()),
        "7a9bba978013e773c6a9a96b844864de04ef4831c2c805ace21b91da3e9a02b8"
    );
    assert_eq!(first.bytes(), second.bytes());
    assert_eq!(first.content_hash(), second.content_hash());
    assert_eq!(first.size(), second.size());
    assert_eq!(
        codec
            .decode(first.bytes(), &input)
            .expect("exact input must decode"),
        result
    );

    let drifted_curve_nodes = carry_input(1);
    assert_eq!(
        drifted_curve_nodes.curve().curve_snapshot(),
        input.curve().curve_snapshot(),
        "negative test deliberately reuses the same declared curve reference"
    );
    assert_ne!(drifted_curve_nodes.fingerprint(), input.fingerprint());
    assert!(codec.decode(first.bytes(), &drifted_curve_nodes).is_err());
}

#[tokio::test]
async fn publish_and_replay_use_verified_artifact_lifecycle() {
    let input = carry_input(0);
    let backend = MemoryBackend::default();
    let scope = AccessScope::new(
        input.owner().tenant_id().clone(),
        id('G'),
        vec![input.owner().owner_id().clone()],
    )
    .unwrap();
    let artifact = PublishCarryRoll::new(
        &NativeCarryRollEngine,
        &ArrowCarryRollCodec,
        &backend,
        &backend,
    )
    .execute(
        scope.clone(),
        id('H'),
        &input,
        IdempotencyKey::new("carry-roll-publish-1").unwrap(),
    )
    .await
    .expect("verified artifact must publish");
    let replay = ReplayCarryRoll::new(
        &NativeCarryRollEngine,
        &ArrowCarryRollCodec,
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
async fn append_failure_discards_private_stage_without_metadata() {
    let input = carry_input(0);
    let backend = MemoryBackend::default();
    backend.state.lock().unwrap().fail_append = true;
    let scope = AccessScope::new(
        input.owner().tenant_id().clone(),
        id('G'),
        vec![input.owner().owner_id().clone()],
    )
    .unwrap();
    let error = PublishCarryRoll::new(
        &NativeCarryRollEngine,
        &ArrowCarryRollCodec,
        &backend,
        &backend,
    )
    .execute(
        scope,
        id('H'),
        &input,
        IdempotencyKey::new("carry-roll-publish-failure").unwrap(),
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

fn carry_input(curve_shift: i128) -> CarryRollInput {
    let valuation_date = date(2026, 7, 19);
    let issue = date(2026, 1, 1);
    let version = Version::new(1).unwrap();
    let market_time = MarketTime::new(
        Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap(),
        "Asia/Shanghai",
        valuation_date,
    )
    .unwrap();
    let calendar = CalendarBinding::new(
        "phase2b-weekend-calendar-v1",
        version,
        ContentHash::digest(b"phase2b-weekend-calendar-v1"),
        issue,
        date(2031, 1, 10),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let terms = BondTerms::new(
        issue,
        date(2029, 1, 1),
        CouponFrequency::Annual,
        DayCountConvention::ActActBondIsma,
        BusinessDayConvention::Following,
        fixed(20_000_000_000),
        fixed(100_000_000_000_000),
    )
    .unwrap();
    let curve = YieldCurveBinding::new(
        object('F'),
        valuation_date,
        YieldCurveInterpolation::LinearYield,
        vec![
            YieldCurveNode::new(date(2027, 1, 1), fixed(12_500_000_000 + curve_shift)).unwrap(),
            YieldCurveNode::new(date(2027, 7, 20), fixed(17_500_000_000)).unwrap(),
            YieldCurveNode::new(date(2028, 1, 1), fixed(19_000_000_000)).unwrap(),
            YieldCurveNode::new(date(2029, 1, 1), fixed(22_500_000_000)).unwrap(),
            YieldCurveNode::new(date(2030, 7, 19), fixed(30_000_000_000)).unwrap(),
        ],
    )
    .unwrap();
    CarryRollInput::new(
        OwnerRef::new(id('A'), id('B')),
        object('C'),
        object('D'),
        object('E'),
        market_time,
        date(2026, 7, 20),
        date(2027, 1, 2),
        CalendarRequirement::ExactMarket,
        calendar,
        terms,
        curve,
    )
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

fn fixed(value: i128) -> FixedDecimal {
    FixedDecimal::from_scaled(value)
}

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut value, byte| {
        write!(value, "{byte:02x}").unwrap();
        value
    })
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
        assert_eq!(u64::try_from(bytes.len()).unwrap(), command.expected_size());
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
