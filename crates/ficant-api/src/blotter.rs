//! gRPC / gRPC-Web `DeskBlotterService` implementation, race-free `SubscribeEvents` barrier,
//! payload-size admission with reversal headroom, and deterministic lifecycle watchdog.

use std::collections::BTreeSet;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use ficant_application::ports::{
    ApplicationResult, BlotterClock, BlotterCommitPlan, BondAnalyticsEngine, CredentialVerifier,
    DeskBlotterPolicyProvider, FuturesDeliveryRuleParser, FuturesHedgeEngine, IdempotencyScopeKey,
    PollEventsOutcome, PrincipalRole, RegulatoryRulePackParser, StoredIdempotencyReceipt,
    VerifiedPrincipal,
};
use ficant_application::use_cases::blotter::{
    BlotterEvaluationEngines, BlotterEventEnvelope, ConfirmDraftEarmarkCommand,
    DeskBlotterOrchestrator, DeskBlotterWorkspaceState, ImportFullOpeningBaseCommand,
    IngestMarketQuotesInput, OrchestratedSweepLifecycleOutput, PreviewCoveredHedgeInput,
    SubmitDraftEarmarkCommand,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_contracts::ficant::blotter::v1 as pb;
use ficant_contracts::ficant::blotter::v1::desk_blotter_service_server::DeskBlotterService;
use ficant_domain::analytics::MARKET_TIMEZONE;
use ficant_domain::blotter::{
    BlotterIdGenerator, CancelCommittedCommand, ConfirmHoldCommand, CreateCommittedCommand,
    CreateHoldCommand, ExtendHoldCommand, IngestTradeFeedInput, ReleaseHoldCommand,
    ResolvePendingMatchInput, SetFeedControlCommand, SweepLifecycleCommand, TradeFeedRecordAction,
};
use ficant_domain::primitives::{ContentHash, Ulid};
use prost::Message;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio_stream::Stream;
use tokio_stream::wrappers::ReceiverStream;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status};

use crate::auth::extract_bearer_token;
use crate::blotter_codec::{
    hash_cancel_committed, hash_confirm_draft, hash_confirm_hold, hash_create_committed,
    hash_create_hold, hash_extend_hold, hash_import_opening_base, hash_ingest_market_quotes,
    hash_ingest_trade_feed, hash_release_hold, hash_resolve_pending_match, hash_set_feed_control,
    hash_submit_draft, hash_sweep_lifecycle, parse_baseline_delta, parse_bucket,
    parse_canonical_baseline_delta, parse_canonical_decimal, parse_canonical_market_quote_tick,
    parse_canonical_nlp_proposal, parse_canonical_quote_terms, parse_canonical_trade_fill_record,
    parse_fixed_decimal, parse_full_opening_position_base, parse_market_quote_tick,
    parse_market_time, parse_nlp_proposal, parse_non_empty, parse_opt_fixed_decimal,
    parse_preview_hedge_scope, parse_quote_terms, parse_resolution_action, parse_sha256,
    parse_side, parse_strict_market_rule_pack, parse_trade_fill_record, parse_ulid,
    to_pb_draft_record, to_pb_earmark_record, to_pb_event_envelope, to_pb_feed_status,
    to_pb_funding_summary, to_pb_market_time, to_pb_pending_match, to_pb_poll_events_batch,
    to_pb_position_view, to_pb_preview_covered_hedge_result, to_pb_risk_summary, to_pb_snapshot,
    to_pb_snapshot_fallback, to_pb_trade_fill_outcome,
};
use crate::to_error_detail;

/// Public default encoded byte limit for a single committed `BlotterEventEnvelope` (3 MiB).
pub const DEFAULT_MAX_EVENT_ENVELOPE_BYTES: usize = 3 * 1024 * 1024;
/// Reserved headroom inside `max_event_envelope_bytes` so accepted account state can always fit
/// a single required reversal event without truncation (512 KiB).
pub const DEFAULT_REVERSAL_HEADROOM_BYTES: usize = 512 * 1024;
/// Default page size used during `SubscribeEvents` catch-up replay.
pub const DEFAULT_SUBSCRIBE_REPLAY_PAGE_SIZE: u32 = 256;
/// Default outbound mpsc channel capacity for each `SubscribeEvents` stream.
pub const DEFAULT_SUBSCRIBE_STREAM_CHANNEL_CAPACITY: usize = 32;
/// Default interval between `SubscribeEvents` stream heartbeats.
pub const DEFAULT_STREAM_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);

/// Result of an atomic authorized write against [`BlotterStorePort`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlotterWriteOutcome {
    Committed {
        response_bytes: Vec<u8>,
        committed_seq: u64,
    },
    Replayed(StoredIdempotencyReceipt),
}

/// Atomic read-barrier handoff snapshot returned when registering a live event subscriber.
pub struct SubscribeBarrierSnapshot {
    pub head_seq: u64,
    pub oldest_retained_seq: u64,
    pub receiver: broadcast::Receiver<BlotterEventEnvelope>,
}

/// Pre-idempotency state-aware authorization callback executed under the single-writer lock.
pub type WriteAuthorizeFn =
    Box<dyn FnOnce(&DeskBlotterWorkspaceState) -> ApplicationResult<()> + Send>;

/// Pure synchronous state transition callback executed under the single-writer lock.
pub type WriteTransitionFn = Box<
    dyn FnOnce(&DeskBlotterWorkspaceState) -> ApplicationResult<(Vec<u8>, BlotterCommitPlan)>
        + Send,
>;

/// Object-safe L3 persistence + event-barrier port implemented by `ficant-server` (over
/// `SqliteBlotterStore`) and API integration tests.
pub trait BlotterStorePort: Send + Sync {
    /// Executes an atomic single-writer command with state-aware pre-idempotency authorization
    /// strictly under the store's exclusive writer lock.
    fn execute_write_authorized_bytes(
        &self,
        idempotency: Option<(&IdempotencyScopeKey, &ContentHash)>,
        authorize: WriteAuthorizeFn,
        transition: WriteTransitionFn,
    ) -> ApplicationResult<BlotterWriteOutcome>;

    /// Reads `(workspace_state, head_seq, oldest_retained_seq)` atomically under the read barrier.
    fn read_workspace_snapshot(&self) -> ApplicationResult<(DeskBlotterWorkspaceState, u64, u64)>;

    /// Registers a live subscriber receiver and reads `(head_seq, oldest_retained_seq)` atomically
    /// under the same single-writer lock that fires the post-commit event hook.
    fn subscribe_with_barrier(&self) -> ApplicationResult<SubscribeBarrierSnapshot>;

    /// Polls whole transactions from the event journal with account/RBAC projection.
    fn poll_events(
        &self,
        after_seq: u64,
        max_events: u32,
        account_filter: Option<&str>,
        allowed_accounts: Option<&BTreeSet<String>>,
    ) -> ApplicationResult<PollEventsOutcome>;
}

/// Optional deterministic hook invoked right after `subscribe_with_barrier` releases the writer
/// lock and before catch-up replay begins (used by tests to inject concurrent barrier commits).
pub type SubscribeBarrierTestHook = Arc<dyn Fn(u64) + Send + Sync>;

/// Runtime transport configuration for [`DeskBlotterGrpcService`].
#[derive(Clone)]
pub struct DeskBlotterServiceConfig {
    pub max_event_envelope_bytes: usize,
    pub reversal_headroom_bytes: usize,
    pub subscribe_replay_page_size: u32,
    pub subscribe_stream_channel_capacity: usize,
    pub stream_heartbeat_interval: Duration,
    pub on_subscribe_barrier_acquired: Option<SubscribeBarrierTestHook>,
}

impl Default for DeskBlotterServiceConfig {
    fn default() -> Self {
        Self {
            max_event_envelope_bytes: DEFAULT_MAX_EVENT_ENVELOPE_BYTES,
            reversal_headroom_bytes: DEFAULT_REVERSAL_HEADROOM_BYTES,
            subscribe_replay_page_size: DEFAULT_SUBSCRIBE_REPLAY_PAGE_SIZE,
            subscribe_stream_channel_capacity: DEFAULT_SUBSCRIBE_STREAM_CHANNEL_CAPACITY,
            stream_heartbeat_interval: DEFAULT_STREAM_HEARTBEAT_INTERVAL,
            on_subscribe_barrier_acquired: None,
        }
    }
}

impl DeskBlotterServiceConfig {
    /// Validates transport capacity limits (reversal headroom must stay strictly below max bytes).
    pub fn validate(&self) -> Result<(), String> {
        if self.max_event_envelope_bytes == 0 {
            return Err("max_event_envelope_bytes must be positive".to_owned());
        }
        if self.reversal_headroom_bytes >= self.max_event_envelope_bytes {
            return Err(
                "reversal_headroom_bytes must be strictly less than max_event_envelope_bytes"
                    .to_owned(),
            );
        }
        if self.subscribe_replay_page_size == 0 || self.subscribe_replay_page_size > 1_024 {
            return Err("subscribe_replay_page_size must be within 1..=1024".to_owned());
        }
        if self.subscribe_stream_channel_capacity == 0 {
            return Err("subscribe_stream_channel_capacity must be positive".to_owned());
        }
        if self.stream_heartbeat_interval.is_zero() {
            return Err("stream_heartbeat_interval must be positive".to_owned());
        }
        Ok(())
    }

    #[must_use]
    pub const fn admission_budget_bytes(&self, is_reversal_or_recovery: bool) -> usize {
        if is_reversal_or_recovery {
            self.max_event_envelope_bytes
        } else {
            self.max_event_envelope_bytes
                .saturating_sub(self.reversal_headroom_bytes)
        }
    }
}

/// Owned synchronous pricing and rule-parsing engines wired into [`DeskBlotterGrpcService`].
#[derive(Clone)]
pub struct OwnedBlotterEngines {
    pub bond_engine: Arc<dyn BondAnalyticsEngine>,
    pub hedge_engine: Arc<dyn FuturesHedgeEngine>,
    pub delivery_rule_parser: Arc<dyn FuturesDeliveryRuleParser>,
    pub regulatory_rule_parser: Arc<dyn RegulatoryRulePackParser>,
}

impl OwnedBlotterEngines {
    #[must_use]
    pub fn as_borrowed(&self) -> BlotterEvaluationEngines<'_> {
        BlotterEvaluationEngines {
            bond_engine: self.bond_engine.as_ref(),
            hedge_engine: self.hedge_engine.as_ref(),
            delivery_rule_parser: self.delivery_rule_parser.as_ref(),
            regulatory_rule_parser: self.regulatory_rule_parser.as_ref(),
        }
    }
}

/// Collision-free deterministic ULID generator seeded from `(timestamp_ms, head_seq)`.
///
/// Even when the injected `ManualBlotterClock` is frozen across WAL restarts, monotonic
/// persisted `head_seq` prevents any ULID reuse across transactions.
pub struct TxUlidGenerator {
    timestamp_ms: u64,
    head_seq: u64,
    local_counter: u64,
}

impl TxUlidGenerator {
    #[must_use]
    pub fn new(now: &ficant_domain::primitives::MarketTime, head_seq: u64) -> Self {
        let ts_ms = u64::try_from(now.instant().timestamp_millis().max(0)).unwrap_or(0);
        Self {
            timestamp_ms: ts_ms,
            head_seq,
            local_counter: 1,
        }
    }
}

impl BlotterIdGenerator for TxUlidGenerator {
    fn next_ulid(&mut self) -> Ulid {
        let rand_part = ((u128::from(self.head_seq) + 1) << 40) | u128::from(self.local_counter);
        self.local_counter = self.local_counter.saturating_add(1);
        let raw = ulid::Ulid::from_parts(self.timestamp_ms, rand_part);
        Ulid::new(raw.to_string()).expect("generated ULID must be canonical")
    }
}

// ============================================================================
// DeskBlotterGrpcService (Enabled)
// ============================================================================

pub struct DeskBlotterGrpcService {
    store: Arc<dyn BlotterStorePort>,
    policy_provider: Arc<dyn DeskBlotterPolicyProvider>,
    credentials: Arc<dyn CredentialVerifier>,
    clock: Arc<dyn BlotterClock>,
    engines: OwnedBlotterEngines,
    config: DeskBlotterServiceConfig,
}

impl DeskBlotterGrpcService {
    pub fn new(
        store: Arc<dyn BlotterStorePort>,
        policy_provider: Arc<dyn DeskBlotterPolicyProvider>,
        credentials: Arc<dyn CredentialVerifier>,
        clock: Arc<dyn BlotterClock>,
        engines: OwnedBlotterEngines,
        config: DeskBlotterServiceConfig,
    ) -> Result<Self, String> {
        config.validate()?;
        Ok(Self {
            store,
            policy_provider,
            credentials,
            clock,
            engines,
            config,
        })
    }

    fn orchestrator(&self) -> DeskBlotterOrchestrator<'_> {
        DeskBlotterOrchestrator::new(self.clock.as_ref(), self.engines.as_borrowed())
    }

    fn authenticate(&self, metadata: &MetadataMap) -> ApplicationResult<VerifiedPrincipal> {
        let token = extract_bearer_token(metadata)?;
        self.credentials.verify_bearer(&token, &self.clock.now())
    }

    fn execute_import_opening_base(
        &self,
        metadata: &MetadataMap,
        req: &pb::ImportOpeningBaseRequest,
    ) -> ApplicationResult<pb::ImportOpeningBaseResult> {
        let principal = self.authenticate(metadata)?;
        require_role(
            &principal,
            &[PrincipalRole::Trader, PrincipalRole::Operator],
        )?;
        let idempotency_key = parse_non_empty(&req.idempotency_key)?;
        let opening_base = parse_full_opening_position_base(req.opening_base.as_ref())?;
        let account_id = opening_base.core.account_id.clone();
        let source_id = opening_base.core.source_id.clone();
        let request_hash = hash_import_opening_base(&opening_base, req.expected_account_version);

        if !principal.can_access_account(&account_id) || !principal.can_access_source(&source_id) {
            return Err(forbidden());
        }

        let scope_key = IdempotencyScopeKey {
            actor_id: principal.actor_id.clone(),
            rpc_name: "ImportOpeningBase".to_owned(),
            scope_key: account_id.clone(),
            idempotency_key: idempotency_key.clone(),
        };
        let account_policy = self.policy_provider.account_policy(&account_id)?;
        let expected_ver = req.expected_account_version;
        let actor_id = principal.actor_id.clone();
        let clock = self.clock.clone();
        let engines = self.engines.clone();
        let config = self.config.clone();
        let receipt_key = scope_key.clone();
        let receipt_hash = request_hash.clone();

        let outcome = self.store.execute_write_authorized_bytes(
            Some((&scope_key, &request_hash)),
            Box::new(|_| Ok(())),
            Box::new(move |state| {
                let orch = DeskBlotterOrchestrator::new(clock.as_ref(), engines.as_borrowed());
                let (next_state, out, event) = orch.import_opening_base(
                    state,
                    &ImportFullOpeningBaseCommand {
                        actor_id,
                        idempotency_key,
                        opening_base,
                        expected_account_version: expected_ver,
                        policy: account_policy,
                    },
                )?;
                if let Some(ev) = event.as_ref()
                    && to_pb_event_envelope(ev).encoded_len() > config.admission_budget_bytes(false)
                {
                    return Err(ApplicationError::blotter_payload_too_large());
                }
                let result = pb::ImportOpeningBaseResult {
                    committed_seq: out.committed_seq,
                    replayed: out.replayed,
                    snapshot: Some(to_pb_snapshot(&out.snapshot)),
                };
                let response_bytes = result.encode_to_vec();
                let plan = BlotterCommitPlan {
                    next_state,
                    event,
                    idempotency_receipt: Some(StoredIdempotencyReceipt {
                        key: receipt_key,
                        request_hash: receipt_hash,
                        committed_seq: out.committed_seq,
                        response_payload: response_bytes.clone(),
                    }),
                };
                Ok((response_bytes, plan))
            }),
        )?;

        decode_or_replay::<pb::ImportOpeningBaseResult>(outcome, |r| r.replayed = true)
    }

    #[allow(clippy::too_many_lines)]
    fn execute_ingest_market_quotes(
        &self,
        metadata: &MetadataMap,
        req: &pb::IngestMarketQuotesRequest,
    ) -> ApplicationResult<pb::IngestMarketQuotesResult> {
        let principal = self.authenticate(metadata)?;
        require_role(
            &principal,
            &[
                PrincipalRole::FeedAdapter,
                PrincipalRole::Operator,
                PrincipalRole::Trader,
            ],
        )?;
        let idempotency_key = parse_non_empty(&req.idempotency_key)?;
        let account_id = parse_non_empty(&req.account_id)?;
        let source_id = parse_non_empty(&req.source_id)?;

        if !principal.can_access_account(&account_id) || !principal.can_access_source(&source_id) {
            return Err(forbidden());
        }

        // Pre-parse ticks preserving every DecimalValue's UnitRef for canonical request hashing
        // and extracting touched instrument codes before state-dependent authorization.
        let mut parsed_for_hash = Vec::with_capacity(req.ticks.len());
        let mut touched_instruments = BTreeSet::new();
        for t in &req.ticks {
            let canonical_tick = parse_canonical_market_quote_tick(t)?;
            if canonical_tick.tick.source_id != source_id {
                return Err(validation_failed());
            }
            touched_instruments.insert(canonical_tick.tick.instrument_code.clone());
            parsed_for_hash.push(canonical_tick);
        }

        let request_hash = hash_ingest_market_quotes(&account_id, &source_id, &parsed_for_hash);
        let scope_key = IdempotencyScopeKey {
            actor_id: principal.actor_id.clone(),
            rpc_name: "IngestMarketQuotes".to_owned(),
            scope_key: account_id.clone(),
            idempotency_key,
        };

        let auth_principal = principal.clone();
        let auth_source = source_id.clone();
        let auth_instruments = touched_instruments;
        let raw_ticks = req.ticks.clone();
        let clock = self.clock.clone();
        let engines = self.engines.clone();
        let config = self.config.clone();
        let receipt_key = scope_key.clone();
        let receipt_hash = request_hash.clone();

        let outcome = self.store.execute_write_authorized_bytes(
            Some((&scope_key, &request_hash)),
            Box::new(move |state| {
                // State-dependent batch authorization under the exclusive writer mutex:
                // EVERY account whose active quote source for any touched instrument is `source_id`
                // MUST be authorized for the caller, strictly BEFORE idempotency receipt lookup!
                for (acct_id, acct) in &state.domain.accounts {
                    let Some(meta) = state.accounts_meta.get(acct_id) else {
                        continue;
                    };
                    let affected = auth_instruments.iter().any(|inst| {
                        acct.positions.contains_key(inst)
                            && meta.policy.active_quote_source_for(inst) == auth_source
                    });
                    if affected && !auth_principal.can_access_account(acct_id) {
                        return Err(forbidden());
                    }
                }
                Ok(())
            }),
            Box::new(move |state| {
                let meta = state.accounts_meta.get(&account_id).ok_or_else(not_found)?;
                let mut ticks = Vec::with_capacity(raw_ticks.len());
                for raw in &raw_ticks {
                    ticks.push(parse_market_quote_tick(raw, &meta.units)?);
                }
                let orch = DeskBlotterOrchestrator::new(clock.as_ref(), engines.as_borrowed());
                let (next_state, out, event) = orch.ingest_market_quotes(
                    state,
                    &IngestMarketQuotesInput {
                        account_id: account_id.clone(),
                        source_id,
                        ticks,
                    },
                )?;
                if let Some(ev) = event.as_ref()
                    && to_pb_event_envelope(ev).encoded_len() > config.admission_budget_bytes(false)
                {
                    return Err(ApplicationError::blotter_payload_too_large());
                }
                let result = pb::IngestMarketQuotesResult {
                    committed_seq: out.committed_seq,
                    replayed: false,
                    applied_tick_count: out.applied_tick_count,
                    skipped_stale_tick_count: out.skipped_stale_tick_count,
                    risk_summary: Some(to_pb_risk_summary(&out.risk_summary, &meta.units)),
                };
                let response_bytes = result.encode_to_vec();
                let plan = BlotterCommitPlan {
                    next_state,
                    event,
                    idempotency_receipt: Some(StoredIdempotencyReceipt {
                        key: receipt_key,
                        request_hash: receipt_hash,
                        committed_seq: out.committed_seq,
                        response_payload: response_bytes.clone(),
                    }),
                };
                Ok((response_bytes, plan))
            }),
        )?;

        decode_or_replay::<pb::IngestMarketQuotesResult>(outcome, |r| r.replayed = true)
    }

    fn execute_create_hold(
        &self,
        metadata: &MetadataMap,
        req: &pb::CreateHoldRequest,
    ) -> ApplicationResult<pb::CreateHoldResult> {
        let principal = self.authenticate(metadata)?;
        require_role(&principal, &[PrincipalRole::Trader])?;
        let idempotency_key = parse_non_empty(&req.idempotency_key)?;
        let account_id = parse_non_empty(&req.account_id)?;
        if !principal.can_access_account(&account_id) {
            return Err(forbidden());
        }
        let instrument_code = parse_non_empty(&req.instrument_code)?;
        let side = parse_side(req.side)?;
        let bucket = parse_bucket(req.settlement_bucket)?;
        let notional_canonical =
            parse_canonical_decimal(req.notional.as_ref().ok_or_else(validation_failed)?)?;
        let quote_canonical = req
            .quote_terms
            .as_ref()
            .map(|qt| parse_canonical_quote_terms(Some(qt)))
            .transpose()?;
        let request_hash = hash_create_hold(
            &account_id,
            req.expected_position_version,
            &instrument_code,
            side,
            bucket,
            &notional_canonical,
            quote_canonical.as_ref(),
            req.ttl_seconds,
            &req.note,
        );
        let scope_key = IdempotencyScopeKey {
            actor_id: principal.actor_id.clone(),
            rpc_name: "CreateHold".to_owned(),
            scope_key: account_id.clone(),
            idempotency_key,
        };

        let raw_notional = req.notional.clone();
        let raw_quote = req.quote_terms.clone();
        let expected_pos_ver = req.expected_position_version;
        let ttl_seconds = req.ttl_seconds;
        let note = req.note.clone();
        let trader_id = principal.actor_id;
        let clock = self.clock.clone();
        let engines = self.engines.clone();
        let config = self.config.clone();
        let receipt_key = scope_key.clone();
        let receipt_hash = request_hash.clone();

        let outcome = self.store.execute_write_authorized_bytes(
            Some((&scope_key, &request_hash)),
            Box::new(|_| Ok(())),
            Box::new(move |state| {
                let meta = state.accounts_meta.get(&account_id).ok_or_else(not_found)?;
                let notional =
                    parse_fixed_decimal(raw_notional.as_ref(), &meta.units.currency_amount)?;
                let quote_terms = raw_quote
                    .as_ref()
                    .map(|qt| parse_quote_terms(Some(qt), &meta.units))
                    .transpose()?;
                let mut id_gen = TxUlidGenerator::new(&clock.now(), state.domain.head_seq);
                let earmark_id = id_gen.next_ulid();
                let orch = DeskBlotterOrchestrator::new(clock.as_ref(), engines.as_borrowed());
                let (next_state, out, event) = orch.create_hold(
                    state,
                    &CreateHoldCommand {
                        account_id: account_id.clone(),
                        earmark_id,
                        expected_position_version: expected_pos_ver,
                        instrument_code,
                        side,
                        settlement_bucket: bucket,
                        notional,
                        quote_terms,
                        ttl_seconds,
                        note,
                        trader_id,
                    },
                )?;
                if to_pb_event_envelope(&event).encoded_len() > config.admission_budget_bytes(false)
                {
                    return Err(ApplicationError::blotter_payload_too_large());
                }
                let result = pb::CreateHoldResult {
                    committed_seq: out.committed_seq,
                    replayed: false,
                    earmark: Some(to_pb_earmark_record(&out.earmark, &meta.units)),
                    position: Some(to_pb_position_view(&out.position, &meta.units)),
                };
                let response_bytes = result.encode_to_vec();
                let plan = BlotterCommitPlan {
                    next_state,
                    event: Some(event),
                    idempotency_receipt: Some(StoredIdempotencyReceipt {
                        key: receipt_key,
                        request_hash: receipt_hash,
                        committed_seq: out.committed_seq,
                        response_payload: response_bytes.clone(),
                    }),
                };
                Ok((response_bytes, plan))
            }),
        )?;

        decode_or_replay::<pb::CreateHoldResult>(outcome, |r| r.replayed = true)
    }

    fn execute_extend_hold(
        &self,
        metadata: &MetadataMap,
        req: &pb::ExtendHoldRequest,
    ) -> ApplicationResult<pb::ExtendHoldResult> {
        let principal = self.authenticate(metadata)?;
        require_role(&principal, &[PrincipalRole::Trader])?;
        let idempotency_key = parse_non_empty(&req.idempotency_key)?;
        let account_id = parse_non_empty(&req.account_id)?;
        if !principal.can_access_account(&account_id) {
            return Err(forbidden());
        }
        let earmark_id = parse_ulid(req.earmark_id.as_ref())?;
        let request_hash = hash_extend_hold(
            &account_id,
            &earmark_id,
            req.expected_earmark_revision,
            req.ttl_seconds,
        );
        let scope_key = IdempotencyScopeKey {
            actor_id: principal.actor_id,
            rpc_name: "ExtendHold".to_owned(),
            scope_key: account_id.clone(),
            idempotency_key,
        };
        let expected_rev = req.expected_earmark_revision;
        let ttl_seconds = req.ttl_seconds;
        let clock = self.clock.clone();
        let engines = self.engines.clone();
        let config = self.config.clone();
        let receipt_key = scope_key.clone();
        let receipt_hash = request_hash.clone();

        let outcome = self.store.execute_write_authorized_bytes(
            Some((&scope_key, &request_hash)),
            Box::new(|_| Ok(())),
            Box::new(move |state| {
                let meta = state.accounts_meta.get(&account_id).ok_or_else(not_found)?;
                let orch = DeskBlotterOrchestrator::new(clock.as_ref(), engines.as_borrowed());
                let (next_state, out, event) = orch.extend_hold(
                    state,
                    &ExtendHoldCommand {
                        account_id: account_id.clone(),
                        earmark_id,
                        expected_earmark_revision: expected_rev,
                        ttl_seconds,
                    },
                )?;
                if to_pb_event_envelope(&event).encoded_len() > config.admission_budget_bytes(false)
                {
                    return Err(ApplicationError::blotter_payload_too_large());
                }
                let result = pb::ExtendHoldResult {
                    committed_seq: out.committed_seq,
                    replayed: false,
                    earmark: Some(to_pb_earmark_record(&out.earmark, &meta.units)),
                };
                let response_bytes = result.encode_to_vec();
                let plan = BlotterCommitPlan {
                    next_state,
                    event: Some(event),
                    idempotency_receipt: Some(StoredIdempotencyReceipt {
                        key: receipt_key,
                        request_hash: receipt_hash,
                        committed_seq: out.committed_seq,
                        response_payload: response_bytes.clone(),
                    }),
                };
                Ok((response_bytes, plan))
            }),
        )?;

        decode_or_replay::<pb::ExtendHoldResult>(outcome, |r| r.replayed = true)
    }

    fn execute_confirm_hold(
        &self,
        metadata: &MetadataMap,
        req: &pb::ConfirmHoldRequest,
    ) -> ApplicationResult<pb::ConfirmHoldResult> {
        let principal = self.authenticate(metadata)?;
        require_role(&principal, &[PrincipalRole::Trader])?;
        let idempotency_key = parse_non_empty(&req.idempotency_key)?;
        let account_id = parse_non_empty(&req.account_id)?;
        if !principal.can_access_account(&account_id) {
            return Err(forbidden());
        }
        let earmark_id = parse_ulid(req.earmark_id.as_ref())?;
        let quote_canonical = parse_canonical_quote_terms(req.quote_terms.as_ref())?;
        let request_hash = hash_confirm_hold(
            &account_id,
            &earmark_id,
            req.expected_earmark_revision,
            req.expected_position_version,
            &quote_canonical,
        );
        let scope_key = IdempotencyScopeKey {
            actor_id: principal.actor_id.clone(),
            rpc_name: "ConfirmHold".to_owned(),
            scope_key: account_id.clone(),
            idempotency_key,
        };
        let expected_rev = req.expected_earmark_revision;
        let expected_pos = req.expected_position_version;
        let raw_quote = req.quote_terms.clone();
        let trader_id = principal.actor_id;
        let clock = self.clock.clone();
        let engines = self.engines.clone();
        let config = self.config.clone();
        let receipt_key = scope_key.clone();
        let receipt_hash = request_hash.clone();

        let outcome = self.store.execute_write_authorized_bytes(
            Some((&scope_key, &request_hash)),
            Box::new(|_| Ok(())),
            Box::new(move |state| {
                let meta = state.accounts_meta.get(&account_id).ok_or_else(not_found)?;
                let quote_terms = parse_quote_terms(raw_quote.as_ref(), &meta.units)?;
                let orch = DeskBlotterOrchestrator::new(clock.as_ref(), engines.as_borrowed());
                let (next_state, out, event) = orch.confirm_hold(
                    state,
                    &ConfirmHoldCommand {
                        account_id: account_id.clone(),
                        earmark_id,
                        expected_earmark_revision: expected_rev,
                        expected_position_version: expected_pos,
                        quote_terms,
                        exact_dirty_price_input: None,
                        trader_id,
                    },
                )?;
                let is_expiry_release = out.reconfirmation_error.is_some();
                if to_pb_event_envelope(&event).encoded_len()
                    > config.admission_budget_bytes(is_expiry_release)
                {
                    return Err(ApplicationError::blotter_payload_too_large());
                }
                let result = pb::ConfirmHoldResult {
                    committed_seq: out.committed_seq,
                    replayed: false,
                    earmark: Some(to_pb_earmark_record(&out.earmark, &meta.units)),
                    position: Some(to_pb_position_view(&out.position, &meta.units)),
                    risk_summary: Some(to_pb_risk_summary(&out.risk_summary, &meta.units)),
                    funding_summary: Some(to_pb_funding_summary(&out.funding_summary, &meta.units)),
                    reconfirmation_error: out.reconfirmation_error.as_ref().map(to_error_detail),
                };
                let response_bytes = result.encode_to_vec();
                let plan = BlotterCommitPlan {
                    next_state,
                    event: Some(event),
                    idempotency_receipt: Some(StoredIdempotencyReceipt {
                        key: receipt_key,
                        request_hash: receipt_hash,
                        committed_seq: out.committed_seq,
                        response_payload: response_bytes.clone(),
                    }),
                };
                Ok((response_bytes, plan))
            }),
        )?;

        decode_or_replay::<pb::ConfirmHoldResult>(outcome, |r| r.replayed = true)
    }

    fn execute_release_hold(
        &self,
        metadata: &MetadataMap,
        req: &pb::ReleaseHoldRequest,
    ) -> ApplicationResult<pb::ReleaseHoldResult> {
        let principal = self.authenticate(metadata)?;
        require_role(&principal, &[PrincipalRole::Trader])?;
        let idempotency_key = parse_non_empty(&req.idempotency_key)?;
        let account_id = parse_non_empty(&req.account_id)?;
        if !principal.can_access_account(&account_id) {
            return Err(forbidden());
        }
        let earmark_id = parse_ulid(req.earmark_id.as_ref())?;
        let request_hash = hash_release_hold(
            &account_id,
            &earmark_id,
            req.expected_earmark_revision,
            &req.reason,
        );
        let scope_key = IdempotencyScopeKey {
            actor_id: principal.actor_id.clone(),
            rpc_name: "ReleaseHold".to_owned(),
            scope_key: account_id.clone(),
            idempotency_key,
        };
        let expected_rev = req.expected_earmark_revision;
        let reason = req.reason.clone();
        let actor_id = principal.actor_id;
        let clock = self.clock.clone();
        let engines = self.engines.clone();
        let config = self.config.clone();
        let receipt_key = scope_key.clone();
        let receipt_hash = request_hash.clone();

        let outcome = self.store.execute_write_authorized_bytes(
            Some((&scope_key, &request_hash)),
            Box::new(|_| Ok(())),
            Box::new(move |state| {
                let meta = state.accounts_meta.get(&account_id).ok_or_else(not_found)?;
                let orch = DeskBlotterOrchestrator::new(clock.as_ref(), engines.as_borrowed());
                let (next_state, out, event) = orch.release_hold(
                    state,
                    &ReleaseHoldCommand {
                        account_id: account_id.clone(),
                        earmark_id,
                        expected_earmark_revision: expected_rev,
                        reason,
                        actor_id,
                    },
                )?;
                if to_pb_event_envelope(&event).encoded_len() > config.admission_budget_bytes(true)
                {
                    return Err(ApplicationError::blotter_payload_too_large());
                }
                let result = pb::ReleaseHoldResult {
                    committed_seq: out.committed_seq,
                    replayed: false,
                    earmark: Some(to_pb_earmark_record(&out.earmark, &meta.units)),
                    position: Some(to_pb_position_view(&out.position, &meta.units)),
                };
                let response_bytes = result.encode_to_vec();
                let plan = BlotterCommitPlan {
                    next_state,
                    event: Some(event),
                    idempotency_receipt: Some(StoredIdempotencyReceipt {
                        key: receipt_key,
                        request_hash: receipt_hash,
                        committed_seq: out.committed_seq,
                        response_payload: response_bytes.clone(),
                    }),
                };
                Ok((response_bytes, plan))
            }),
        )?;

        decode_or_replay::<pb::ReleaseHoldResult>(outcome, |r| r.replayed = true)
    }

    fn execute_create_committed(
        &self,
        metadata: &MetadataMap,
        req: &pb::CreateCommittedRequest,
    ) -> ApplicationResult<pb::CreateCommittedResult> {
        let principal = self.authenticate(metadata)?;
        require_role(&principal, &[PrincipalRole::Trader])?;
        let idempotency_key = parse_non_empty(&req.idempotency_key)?;
        let account_id = parse_non_empty(&req.account_id)?;
        if !principal.can_access_account(&account_id) {
            return Err(forbidden());
        }
        let instrument_code = parse_non_empty(&req.instrument_code)?;
        let side = parse_side(req.side)?;
        let bucket = parse_bucket(req.settlement_bucket)?;
        let notional_canonical =
            parse_canonical_decimal(req.notional.as_ref().ok_or_else(validation_failed)?)?;
        let quote_canonical = parse_canonical_quote_terms(req.quote_terms.as_ref())?;
        let request_hash = hash_create_committed(
            &account_id,
            req.expected_position_version,
            &instrument_code,
            side,
            bucket,
            &notional_canonical,
            &quote_canonical,
            &req.note,
        );
        let scope_key = IdempotencyScopeKey {
            actor_id: principal.actor_id.clone(),
            rpc_name: "CreateCommitted".to_owned(),
            scope_key: account_id.clone(),
            idempotency_key,
        };
        let raw_notional = req.notional.clone();
        let raw_quote = req.quote_terms.clone();
        let expected_pos = req.expected_position_version;
        let note = req.note.clone();
        let trader_id = principal.actor_id;
        let clock = self.clock.clone();
        let engines = self.engines.clone();
        let config = self.config.clone();
        let receipt_key = scope_key.clone();
        let receipt_hash = request_hash.clone();

        let outcome = self.store.execute_write_authorized_bytes(
            Some((&scope_key, &request_hash)),
            Box::new(|_| Ok(())),
            Box::new(move |state| {
                let meta = state.accounts_meta.get(&account_id).ok_or_else(not_found)?;
                let notional =
                    parse_fixed_decimal(raw_notional.as_ref(), &meta.units.currency_amount)?;
                let quote_terms = parse_quote_terms(raw_quote.as_ref(), &meta.units)?;
                let mut id_gen = TxUlidGenerator::new(&clock.now(), state.domain.head_seq);
                let earmark_id = id_gen.next_ulid();
                let orch = DeskBlotterOrchestrator::new(clock.as_ref(), engines.as_borrowed());
                let (next_state, out, event) = orch.create_committed(
                    state,
                    &CreateCommittedCommand {
                        account_id: account_id.clone(),
                        earmark_id,
                        expected_position_version: expected_pos,
                        instrument_code,
                        side,
                        settlement_bucket: bucket,
                        notional,
                        quote_terms,
                        exact_dirty_price_input: None,
                        note,
                        trader_id,
                    },
                )?;
                if to_pb_event_envelope(&event).encoded_len() > config.admission_budget_bytes(false)
                {
                    return Err(ApplicationError::blotter_payload_too_large());
                }
                let result = pb::CreateCommittedResult {
                    committed_seq: out.committed_seq,
                    replayed: false,
                    earmark: Some(to_pb_earmark_record(&out.earmark, &meta.units)),
                    position: Some(to_pb_position_view(&out.position, &meta.units)),
                    risk_summary: Some(to_pb_risk_summary(&out.risk_summary, &meta.units)),
                    funding_summary: Some(to_pb_funding_summary(&out.funding_summary, &meta.units)),
                };
                let response_bytes = result.encode_to_vec();
                let plan = BlotterCommitPlan {
                    next_state,
                    event: Some(event),
                    idempotency_receipt: Some(StoredIdempotencyReceipt {
                        key: receipt_key,
                        request_hash: receipt_hash,
                        committed_seq: out.committed_seq,
                        response_payload: response_bytes.clone(),
                    }),
                };
                Ok((response_bytes, plan))
            }),
        )?;

        decode_or_replay::<pb::CreateCommittedResult>(outcome, |r| r.replayed = true)
    }

    fn execute_cancel_committed(
        &self,
        metadata: &MetadataMap,
        req: &pb::CancelCommittedRequest,
    ) -> ApplicationResult<pb::CancelCommittedResult> {
        let principal = self.authenticate(metadata)?;
        require_role(
            &principal,
            &[PrincipalRole::Trader, PrincipalRole::Operator],
        )?;
        let idempotency_key = parse_non_empty(&req.idempotency_key)?;
        let account_id = parse_non_empty(&req.account_id)?;
        if !principal.can_access_account(&account_id) {
            return Err(forbidden());
        }
        let earmark_id = parse_ulid(req.earmark_id.as_ref())?;
        let reason = parse_non_empty(&req.reason)?;
        let request_hash = hash_cancel_committed(
            &account_id,
            &earmark_id,
            req.expected_earmark_revision,
            &reason,
        );
        let scope_key = IdempotencyScopeKey {
            actor_id: principal.actor_id.clone(),
            rpc_name: "CancelCommitted".to_owned(),
            scope_key: account_id.clone(),
            idempotency_key,
        };
        let expected_rev = req.expected_earmark_revision;
        let actor_id = principal.actor_id;
        let clock = self.clock.clone();
        let engines = self.engines.clone();
        let config = self.config.clone();
        let receipt_key = scope_key.clone();
        let receipt_hash = request_hash.clone();

        let outcome = self.store.execute_write_authorized_bytes(
            Some((&scope_key, &request_hash)),
            Box::new(|_| Ok(())),
            Box::new(move |state| {
                let meta = state.accounts_meta.get(&account_id).ok_or_else(not_found)?;
                let orch = DeskBlotterOrchestrator::new(clock.as_ref(), engines.as_borrowed());
                let (next_state, out, event) = orch.cancel_committed(
                    state,
                    &CancelCommittedCommand {
                        account_id: account_id.clone(),
                        earmark_id,
                        expected_earmark_revision: expected_rev,
                        reason,
                        actor_id,
                    },
                )?;
                if to_pb_event_envelope(&event).encoded_len() > config.admission_budget_bytes(true)
                {
                    return Err(ApplicationError::blotter_payload_too_large());
                }
                let result = pb::CancelCommittedResult {
                    committed_seq: out.committed_seq,
                    replayed: false,
                    earmark: Some(to_pb_earmark_record(&out.earmark, &meta.units)),
                    position: Some(to_pb_position_view(&out.position, &meta.units)),
                    risk_summary: Some(to_pb_risk_summary(&out.risk_summary, &meta.units)),
                    funding_summary: Some(to_pb_funding_summary(&out.funding_summary, &meta.units)),
                };
                let response_bytes = result.encode_to_vec();
                let plan = BlotterCommitPlan {
                    next_state,
                    event: Some(event),
                    idempotency_receipt: Some(StoredIdempotencyReceipt {
                        key: receipt_key,
                        request_hash: receipt_hash,
                        committed_seq: out.committed_seq,
                        response_payload: response_bytes.clone(),
                    }),
                };
                Ok((response_bytes, plan))
            }),
        )?;

        decode_or_replay::<pb::CancelCommittedResult>(outcome, |r| r.replayed = true)
    }

    #[allow(clippy::too_many_lines)]
    fn execute_submit_draft(
        &self,
        metadata: &MetadataMap,
        req: &pb::SubmitDraftEarmarkRequest,
    ) -> ApplicationResult<pb::SubmitDraftEarmarkResult> {
        let principal = self.authenticate(metadata)?;
        require_role(
            &principal,
            &[PrincipalRole::NlpParser, PrincipalRole::Trader],
        )?;
        let idempotency_key = parse_non_empty(&req.idempotency_key)?;
        let account_id = parse_non_empty(&req.account_id)?;
        if !principal.can_access_account(&account_id) {
            return Err(forbidden());
        }

        let hash_proposal = parse_canonical_nlp_proposal(req.proposal.as_ref())?;
        if hash_proposal.account_id != account_id {
            return Err(validation_failed());
        }
        let request_hash = hash_submit_draft(&account_id, &hash_proposal);
        let scope_key = IdempotencyScopeKey {
            actor_id: principal.actor_id,
            rpc_name: "SubmitDraftEarmark".to_owned(),
            scope_key: account_id.clone(),
            idempotency_key,
        };
        let raw_proposal_cloned = req.proposal.clone();
        let clock = self.clock.clone();
        let engines = self.engines.clone();
        let config = self.config.clone();
        let receipt_key = scope_key.clone();
        let receipt_hash = request_hash.clone();

        let outcome = self.store.execute_write_authorized_bytes(
            Some((&scope_key, &request_hash)),
            Box::new(|_| Ok(())),
            Box::new(move |state| {
                let meta = state.accounts_meta.get(&account_id).ok_or_else(not_found)?;
                let proposal = parse_nlp_proposal(raw_proposal_cloned.as_ref(), &meta.units)?;
                let mut id_gen = TxUlidGenerator::new(&clock.now(), state.domain.head_seq);
                let draft_id = id_gen.next_ulid();
                let orch = DeskBlotterOrchestrator::new(clock.as_ref(), engines.as_borrowed());
                let (next_state, out, event) = orch.submit_draft_earmark(
                    state,
                    &SubmitDraftEarmarkCommand {
                        account_id: account_id.clone(),
                        draft_id,
                        proposal,
                    },
                )?;
                if to_pb_event_envelope(&event).encoded_len() > config.admission_budget_bytes(false)
                {
                    return Err(ApplicationError::blotter_payload_too_large());
                }
                let result = pb::SubmitDraftEarmarkResult {
                    committed_seq: out.committed_seq,
                    replayed: false,
                    draft: Some(to_pb_draft_record(&out.draft, &meta.units)),
                };
                let response_bytes = result.encode_to_vec();
                let plan = BlotterCommitPlan {
                    next_state,
                    event: Some(event),
                    idempotency_receipt: Some(StoredIdempotencyReceipt {
                        key: receipt_key,
                        request_hash: receipt_hash,
                        committed_seq: out.committed_seq,
                        response_payload: response_bytes.clone(),
                    }),
                };
                Ok((response_bytes, plan))
            }),
        )?;

        decode_or_replay::<pb::SubmitDraftEarmarkResult>(outcome, |r| r.replayed = true)
    }

    fn execute_confirm_draft(
        &self,
        metadata: &MetadataMap,
        req: &pb::ConfirmDraftEarmarkRequest,
    ) -> ApplicationResult<pb::ConfirmDraftEarmarkResult> {
        let principal = self.authenticate(metadata)?;
        require_role(&principal, &[PrincipalRole::Trader])?;
        let idempotency_key = parse_non_empty(&req.idempotency_key)?;
        let account_id = parse_non_empty(&req.account_id)?;
        if !principal.can_access_account(&account_id) {
            return Err(forbidden());
        }
        let draft_id = parse_ulid(req.draft_id.as_ref())?;
        let token = parse_sha256(req.confirmation_token.as_ref())?;
        let request_hash = hash_confirm_draft(
            &account_id,
            &draft_id,
            req.expected_draft_revision,
            req.expected_position_version,
            &token,
            req.acknowledge_uncovered_dv01,
        );
        let scope_key = IdempotencyScopeKey {
            actor_id: principal.actor_id.clone(),
            rpc_name: "ConfirmDraftEarmark".to_owned(),
            scope_key: account_id.clone(),
            idempotency_key,
        };
        let expected_draft_rev = req.expected_draft_revision;
        let expected_pos_ver = req.expected_position_version;
        let ack_uncovered = req.acknowledge_uncovered_dv01;
        let trader_id = principal.actor_id;
        let clock = self.clock.clone();
        let engines = self.engines.clone();
        let config = self.config.clone();
        let receipt_key = scope_key.clone();
        let receipt_hash = request_hash.clone();

        let outcome = self.store.execute_write_authorized_bytes(
            Some((&scope_key, &request_hash)),
            Box::new(|_| Ok(())),
            Box::new(move |state| {
                let meta = state.accounts_meta.get(&account_id).ok_or_else(not_found)?;
                let mut id_gen = TxUlidGenerator::new(&clock.now(), state.domain.head_seq);
                let earmark_id = id_gen.next_ulid();
                let orch = DeskBlotterOrchestrator::new(clock.as_ref(), engines.as_borrowed());
                let (next_state, out, event) = orch.confirm_draft_earmark(
                    state,
                    &ConfirmDraftEarmarkCommand {
                        account_id: account_id.clone(),
                        draft_id,
                        earmark_id,
                        expected_draft_revision: expected_draft_rev,
                        expected_position_version: expected_pos_ver,
                        confirmation_token: token,
                        acknowledge_uncovered_dv01: ack_uncovered,
                        trader_id,
                    },
                )?;
                if to_pb_event_envelope(&event).encoded_len() > config.admission_budget_bytes(false)
                {
                    return Err(ApplicationError::blotter_payload_too_large());
                }
                let result = pb::ConfirmDraftEarmarkResult {
                    committed_seq: out.committed_seq,
                    replayed: false,
                    draft: Some(to_pb_draft_record(&out.draft, &meta.units)),
                    earmark: Some(to_pb_earmark_record(&out.earmark, &meta.units)),
                    position: Some(to_pb_position_view(&out.position, &meta.units)),
                    risk_summary: Some(to_pb_risk_summary(&out.risk_summary, &meta.units)),
                    funding_summary: Some(to_pb_funding_summary(&out.funding_summary, &meta.units)),
                };
                let response_bytes = result.encode_to_vec();
                let plan = BlotterCommitPlan {
                    next_state,
                    event: Some(event),
                    idempotency_receipt: Some(StoredIdempotencyReceipt {
                        key: receipt_key,
                        request_hash: receipt_hash,
                        committed_seq: out.committed_seq,
                        response_payload: response_bytes.clone(),
                    }),
                };
                Ok((response_bytes, plan))
            }),
        )?;

        decode_or_replay::<pb::ConfirmDraftEarmarkResult>(outcome, |r| r.replayed = true)
    }

    #[allow(clippy::too_many_lines)]
    fn execute_ingest_trade_feed(
        &self,
        metadata: &MetadataMap,
        req: &pb::IngestTradeFeedRequest,
    ) -> ApplicationResult<pb::IngestTradeFeedResult> {
        let principal = self.authenticate(metadata)?;
        require_role(
            &principal,
            &[PrincipalRole::FeedAdapter, PrincipalRole::Operator],
        )?;
        let idempotency_key = parse_non_empty(&req.idempotency_key)?;
        let source_id = parse_non_empty(&req.source_id)?;
        if !principal.can_access_source(&source_id) {
            return Err(forbidden());
        }

        let heartbeat_at = req
            .heartbeat_at
            .as_ref()
            .map(|mt| parse_market_time(Some(mt)))
            .transpose()?;

        let mut parsed_for_hash = Vec::with_capacity(req.fills.len());
        let mut is_reversal_or_amend = false;
        for raw in &req.fills {
            let canonical_fill = parse_canonical_trade_fill_record(raw)?;
            if canonical_fill.record.source_id != source_id {
                return Err(validation_failed());
            }
            if !principal.can_access_account(&canonical_fill.record.account_id) {
                return Err(forbidden());
            }
            if matches!(
                canonical_fill.record.action,
                TradeFeedRecordAction::CancelReversal | TradeFeedRecordAction::Amend
            ) {
                is_reversal_or_amend = true;
            }
            parsed_for_hash.push(canonical_fill);
        }

        let request_hash =
            hash_ingest_trade_feed(&source_id, &parsed_for_hash, heartbeat_at.as_ref());
        let scope_key = IdempotencyScopeKey {
            actor_id: principal.actor_id.clone(),
            rpc_name: "IngestTradeFeed".to_owned(),
            scope_key: source_id.clone(),
            idempotency_key,
        };

        let auth_principal = principal.clone();
        let auth_source = source_id.clone();
        let raw_fills = req.fills.clone();
        let actor_id = principal.actor_id;
        let clock = self.clock.clone();
        let engines = self.engines.clone();
        let config = self.config.clone();
        let receipt_key = scope_key.clone();
        let receipt_hash = request_hash.clone();

        let outcome = self.store.execute_write_authorized_bytes(
            Some((&scope_key, &request_hash)),
            Box::new(move |state| {
                // If unreleased buffered packets exist on this source, verify the caller is also
                // authorized for any account whose buffered packet could be released by this call.
                if let Some(feed) = state.domain.feeds.get(&auth_source) {
                    let has_unreleased = feed.source_batches.values().any(|b| !b.released);
                    if has_unreleased {
                        for packet in feed.raw_packets.values() {
                            if !auth_principal.can_access_account(&packet.record.account_id) {
                                return Err(forbidden());
                            }
                        }
                    }
                }
                Ok(())
            }),
            Box::new(move |state| {
                let mut fills = Vec::with_capacity(raw_fills.len());
                for raw in &raw_fills {
                    let acct_units = state.accounts_meta.get(&raw.account_id).map(|m| &m.units);
                    fills.push(parse_trade_fill_record(raw, acct_units)?);
                }
                let mut id_gen = TxUlidGenerator::new(&clock.now(), state.domain.head_seq);
                let orch = DeskBlotterOrchestrator::new(clock.as_ref(), engines.as_borrowed());
                let (next_state, out, event) = orch.ingest_trade_feed(
                    state,
                    &IngestTradeFeedInput {
                        source_id,
                        fills,
                        heartbeat_at,
                        actor_id,
                    },
                    &mut id_gen,
                )?;
                let has_reversal_alloc = is_reversal_or_amend
                    || out
                        .changed_allocations
                        .iter()
                        .any(|a| a.reversed_by_revision.is_some());
                if let Some(ev) = event.as_ref()
                    && to_pb_event_envelope(ev).encoded_len()
                        > config.admission_budget_bytes(has_reversal_alloc)
                {
                    return Err(ApplicationError::blotter_payload_too_large());
                }
                let result = pb::IngestTradeFeedResult {
                    committed_seq: out.committed_seq,
                    replayed: false,
                    item_outcomes: out
                        .item_outcomes
                        .iter()
                        .map(to_pb_trade_fill_outcome)
                        .collect(),
                    feed_status: Some(to_pb_feed_status(&out.feed_status)),
                };
                let response_bytes = result.encode_to_vec();
                let plan = BlotterCommitPlan {
                    next_state,
                    event,
                    idempotency_receipt: Some(StoredIdempotencyReceipt {
                        key: receipt_key,
                        request_hash: receipt_hash,
                        committed_seq: out.committed_seq,
                        response_payload: response_bytes.clone(),
                    }),
                };
                Ok((response_bytes, plan))
            }),
        )?;

        decode_or_replay::<pb::IngestTradeFeedResult>(outcome, |r| r.replayed = true)
    }

    #[allow(clippy::too_many_lines)]
    fn execute_resolve_pending_match(
        &self,
        metadata: &MetadataMap,
        req: &pb::ResolvePendingMatchRequest,
    ) -> ApplicationResult<pb::ResolvePendingMatchResult> {
        let principal = self.authenticate(metadata)?;
        require_role(
            &principal,
            &[PrincipalRole::Trader, PrincipalRole::Operator],
        )?;
        let idempotency_key = parse_non_empty(&req.idempotency_key)?;
        let account_id = parse_non_empty(&req.account_id)?;
        if !principal.can_access_account(&account_id) {
            return Err(forbidden());
        }
        let pending_match_id = parse_ulid(req.pending_match_id.as_ref())?;
        let action = parse_resolution_action(req.action)?;
        let target_earmark_id = req
            .target_earmark_id
            .as_ref()
            .map(|u| parse_ulid(Some(u)))
            .transpose()?;
        let offset_canonical = req
            .offset_notional
            .as_ref()
            .map(parse_canonical_decimal)
            .transpose()?;
        let baseline_canonical = req
            .baseline_delta
            .as_ref()
            .map(|d| parse_canonical_baseline_delta(Some(d)))
            .transpose()?;

        let request_hash = hash_resolve_pending_match(
            &account_id,
            &pending_match_id,
            req.expected_pending_revision,
            req.expected_position_version,
            action,
            req.expected_earmark_revision,
            target_earmark_id.as_ref(),
            offset_canonical.as_ref(),
            &req.reason,
            baseline_canonical.as_ref(),
        );
        let scope_key = IdempotencyScopeKey {
            actor_id: principal.actor_id.clone(),
            rpc_name: "ResolvePendingMatch".to_owned(),
            scope_key: account_id.clone(),
            idempotency_key,
        };
        let expected_pending_rev = req.expected_pending_revision;
        let expected_pos_ver = req.expected_position_version;
        let expected_earmark_rev = req.expected_earmark_revision;
        let raw_offset = req.offset_notional.clone();
        let raw_baseline = req.baseline_delta.clone();
        let reason = req.reason.clone();
        let actor_id = principal.actor_id;
        let clock = self.clock.clone();
        let engines = self.engines.clone();
        let config = self.config.clone();
        let receipt_key = scope_key.clone();
        let receipt_hash = request_hash.clone();

        let outcome = self.store.execute_write_authorized_bytes(
            Some((&scope_key, &request_hash)),
            Box::new(|_| Ok(())),
            Box::new(move |state| {
                let meta = state.accounts_meta.get(&account_id).ok_or_else(not_found)?;
                let offset_notional =
                    parse_opt_fixed_decimal(raw_offset.as_ref(), &meta.units.currency_amount)?;
                let baseline_delta = raw_baseline
                    .as_ref()
                    .map(|d| parse_baseline_delta(Some(d), &meta.units))
                    .transpose()?;
                let mut id_gen = TxUlidGenerator::new(&clock.now(), state.domain.head_seq);
                let orch = DeskBlotterOrchestrator::new(clock.as_ref(), engines.as_borrowed());
                let (next_state, out, event) = orch.resolve_pending_match(
                    state,
                    &ResolvePendingMatchInput {
                        account_id: account_id.clone(),
                        pending_match_id,
                        expected_pending_revision: expected_pending_rev,
                        expected_position_version: expected_pos_ver,
                        action,
                        expected_earmark_revision: expected_earmark_rev,
                        target_earmark_id,
                        offset_notional,
                        reason,
                        baseline_delta,
                        actor_id,
                    },
                    &mut id_gen,
                )?;
                if to_pb_event_envelope(&event).encoded_len() > config.admission_budget_bytes(true)
                {
                    return Err(ApplicationError::blotter_payload_too_large());
                }
                let result = pb::ResolvePendingMatchResult {
                    committed_seq: out.committed_seq,
                    replayed: false,
                    pending_match: Some(to_pb_pending_match(&out.pending_match, &meta.units)),
                    updated_earmark: out
                        .updated_earmark
                        .as_ref()
                        .map(|e| to_pb_earmark_record(e, &meta.units)),
                    position: Some(to_pb_position_view(&out.position, &meta.units)),
                    risk_summary: Some(to_pb_risk_summary(&out.risk_summary, &meta.units)),
                    funding_summary: Some(to_pb_funding_summary(&out.funding_summary, &meta.units)),
                };
                let response_bytes = result.encode_to_vec();
                let plan = BlotterCommitPlan {
                    next_state,
                    event: Some(event),
                    idempotency_receipt: Some(StoredIdempotencyReceipt {
                        key: receipt_key,
                        request_hash: receipt_hash,
                        committed_seq: out.committed_seq,
                        response_payload: response_bytes.clone(),
                    }),
                };
                Ok((response_bytes, plan))
            }),
        )?;

        decode_or_replay::<pb::ResolvePendingMatchResult>(outcome, |r| r.replayed = true)
    }

    fn execute_sweep_lifecycle(
        &self,
        metadata: &MetadataMap,
        req: &pb::SweepLifecycleRequest,
    ) -> ApplicationResult<pb::SweepLifecycleResult> {
        let principal = self.authenticate(metadata)?;
        require_role(
            &principal,
            &[PrincipalRole::Trader, PrincipalRole::Operator],
        )?;
        let idempotency_key = parse_non_empty(&req.idempotency_key)?;
        let account_filter = req.account_id.as_deref().map(parse_non_empty).transpose()?;
        let source_filter = req.source_id.as_deref().map(parse_non_empty).transpose()?;

        if let Some(ref acct) = account_filter
            && !principal.can_access_account(acct)
        {
            return Err(forbidden());
        }
        if let Some(ref src) = source_filter
            && !principal.can_access_source(src)
        {
            return Err(forbidden());
        }

        let request_hash =
            hash_sweep_lifecycle(account_filter.as_deref(), source_filter.as_deref());
        let canonical_scope = format!(
            "acct={}|src={}",
            account_filter.as_deref().unwrap_or("*"),
            source_filter.as_deref().unwrap_or("*")
        );
        let scope_key = IdempotencyScopeKey {
            actor_id: principal.actor_id.clone(),
            rpc_name: "SweepLifecycle".to_owned(),
            scope_key: canonical_scope,
            idempotency_key,
        };
        let auth_principal = principal;
        let auth_acct = account_filter.clone();
        let auth_src = source_filter.clone();
        let clock = self.clock.clone();
        let engines = self.engines.clone();
        let config = self.config.clone();
        let receipt_key = scope_key.clone();
        let receipt_hash = request_hash.clone();

        let outcome = self.store.execute_write_authorized_bytes(
            Some((&scope_key, &request_hash)),
            Box::new(move |state| {
                if auth_acct.is_none() {
                    for acct_id in state.domain.accounts.keys() {
                        if !auth_principal.can_access_account(acct_id) {
                            return Err(forbidden());
                        }
                    }
                }
                if auth_src.is_none() {
                    for src_id in state.domain.feeds.keys() {
                        if !auth_principal.can_access_source(src_id) {
                            return Err(forbidden());
                        }
                    }
                }
                Ok(())
            }),
            Box::new(move |state| {
                let orch = DeskBlotterOrchestrator::new(clock.as_ref(), engines.as_borrowed());
                let (next_state, out, event) = orch.sweep_lifecycle(
                    state,
                    &SweepLifecycleCommand {
                        account_id: account_filter,
                        source_id: source_filter,
                    },
                )?;
                if let Some(ev) = event.as_ref()
                    && to_pb_event_envelope(ev).encoded_len() > config.admission_budget_bytes(true)
                {
                    return Err(ApplicationError::blotter_payload_too_large());
                }
                let result = pb::SweepLifecycleResult {
                    head_seq: out.head_seq,
                    replayed: false,
                    expired_hold_count: out.expired_hold_count,
                    newly_alerted_committed_count: out.newly_alerted_committed_count,
                    newly_stale_feed_count: out.newly_stale_feed_count,
                };
                let response_bytes = result.encode_to_vec();
                let plan = BlotterCommitPlan {
                    next_state,
                    event,
                    idempotency_receipt: Some(StoredIdempotencyReceipt {
                        key: receipt_key,
                        request_hash: receipt_hash,
                        committed_seq: out.head_seq,
                        response_payload: response_bytes.clone(),
                    }),
                };
                Ok((response_bytes, plan))
            }),
        )?;

        decode_or_replay::<pb::SweepLifecycleResult>(outcome, |r| r.replayed = true)
    }

    fn execute_set_feed_control(
        &self,
        metadata: &MetadataMap,
        req: &pb::SetFeedControlRequest,
    ) -> ApplicationResult<pb::SetFeedControlResult> {
        let principal = self.authenticate(metadata)?;
        if req.force_clear_gap_pause {
            require_role(&principal, &[PrincipalRole::Operator])?;
        } else {
            require_role(
                &principal,
                &[PrincipalRole::Trader, PrincipalRole::Operator],
            )?;
        }
        let idempotency_key = parse_non_empty(&req.idempotency_key)?;
        let source_id = parse_non_empty(&req.source_id)?;
        if !principal.can_access_source(&source_id) {
            return Err(forbidden());
        }
        let request_hash = hash_set_feed_control(
            &source_id,
            req.expected_feed_status_revision,
            req.paused_by_operator,
            req.force_clear_gap_pause,
            &req.reason,
        );
        let scope_key = IdempotencyScopeKey {
            actor_id: principal.actor_id.clone(),
            rpc_name: "SetFeedControl".to_owned(),
            scope_key: source_id.clone(),
            idempotency_key,
        };
        let auth_principal = principal;
        let auth_source = source_id.clone();
        let expected_rev = req.expected_feed_status_revision;
        let paused_by_op = req.paused_by_operator;
        let force_clear = req.force_clear_gap_pause;
        let reason = req.reason.clone();
        let clock = self.clock.clone();
        let engines = self.engines.clone();
        let config = self.config.clone();
        let receipt_key = scope_key.clone();
        let receipt_hash = request_hash.clone();

        let outcome = self.store.execute_write_authorized_bytes(
            Some((&scope_key, &request_hash)),
            Box::new(move |state| {
                // State-dependent authorization under the exclusive writer mutex:
                // SetFeedControl mutates `earmarks_paused` and commits snapshot replacements for
                // EVERY account bound to `source_id`, so the caller must hold access to ALL such
                // accounts strictly BEFORE idempotency receipt lookup.
                for acct in state.domain.accounts.values() {
                    if acct.opening_base.source_id == auth_source
                        && !auth_principal.can_access_account(&acct.account_id)
                    {
                        return Err(forbidden());
                    }
                }
                Ok(())
            }),
            Box::new(move |state| {
                let orch = DeskBlotterOrchestrator::new(clock.as_ref(), engines.as_borrowed());
                let (next_state, out, event) = orch.set_feed_control(
                    state,
                    &SetFeedControlCommand {
                        source_id,
                        expected_feed_status_revision: expected_rev,
                        paused_by_operator: paused_by_op,
                        force_clear_gap_pause: force_clear,
                        reason,
                    },
                )?;
                if to_pb_event_envelope(&event).encoded_len() > config.admission_budget_bytes(true)
                {
                    return Err(ApplicationError::blotter_payload_too_large());
                }
                let result = pb::SetFeedControlResult {
                    committed_seq: out.committed_seq,
                    replayed: false,
                    feed_status: Some(to_pb_feed_status(&out.feed_status)),
                };
                let response_bytes = result.encode_to_vec();
                let plan = BlotterCommitPlan {
                    next_state,
                    event: Some(event),
                    idempotency_receipt: Some(StoredIdempotencyReceipt {
                        key: receipt_key,
                        request_hash: receipt_hash,
                        committed_seq: out.committed_seq,
                        response_payload: response_bytes.clone(),
                    }),
                };
                Ok((response_bytes, plan))
            }),
        )?;

        decode_or_replay::<pb::SetFeedControlResult>(outcome, |r| r.replayed = true)
    }

    fn execute_preview_covered_hedge(
        &self,
        metadata: &MetadataMap,
        req: &pb::PreviewCoveredHedgeRequest,
    ) -> ApplicationResult<pb::PreviewCoveredHedgeResult> {
        let principal = self.authenticate(metadata)?;
        require_role(
            &principal,
            &[PrincipalRole::Trader, PrincipalRole::Operator],
        )?;
        let account_id = parse_non_empty(&req.account_id)?;
        if !principal.can_access_account(&account_id) {
            return Err(forbidden());
        }
        let scope = parse_preview_hedge_scope(req.scope)?;
        let product_code = parse_non_empty(&req.product_code)?;
        let contract_code = parse_non_empty(&req.contract_code)?;
        let delivery_rule_pack = parse_strict_market_rule_pack(req.delivery_rule_pack.as_ref())?;
        let valuation_at = req
            .valuation_at
            .as_ref()
            .map(|mt| {
                let parsed = parse_market_time(Some(mt))?;
                if parsed.market_timezone() != MARKET_TIMEZONE {
                    return Err(validation_failed());
                }
                Ok(parsed)
            })
            .transpose()?;

        let (state, _, _) = self.store.read_workspace_snapshot()?;
        let meta = state.accounts_meta.get(&account_id).ok_or_else(not_found)?;
        let ctd_dv01_per_100 =
            parse_fixed_decimal(req.ctd_dv01_per_100.as_ref(), &meta.units.dv01_per_100)?;
        let conversion_factor =
            parse_fixed_decimal(req.conversion_factor.as_ref(), &meta.units.dimensionless)?;

        let out = self.orchestrator().preview_covered_hedge(
            &state,
            &PreviewCoveredHedgeInput {
                account_id,
                scope,
                product_code,
                contract_code,
                ctd_dv01_per_100,
                conversion_factor,
                delivery_rule_pack,
                valuation_at,
                expected_snapshot_seq: req.expected_snapshot_seq,
            },
        )?;
        Ok(to_pb_preview_covered_hedge_result(&out, &meta.units))
    }

    fn execute_get_account_snapshot(
        &self,
        metadata: &MetadataMap,
        req: &pb::GetAccountSnapshotRequest,
    ) -> ApplicationResult<pb::GetAccountSnapshotResult> {
        let principal = self.authenticate(metadata)?;
        require_role(
            &principal,
            &[PrincipalRole::Trader, PrincipalRole::Operator],
        )?;
        let account_id = parse_non_empty(&req.account_id)?;
        if !principal.can_access_account(&account_id) {
            return Err(forbidden());
        }
        let (state, _, _) = self.store.read_workspace_snapshot()?;
        let snap = self
            .orchestrator()
            .get_account_snapshot(&state, &account_id)?;
        Ok(pb::GetAccountSnapshotResult {
            snapshot: Some(to_pb_snapshot(&snap)),
        })
    }

    fn execute_poll_events(
        &self,
        metadata: &MetadataMap,
        req: &pb::PollEventsRequest,
    ) -> ApplicationResult<pb::PollEventsResult> {
        let principal = self.authenticate(metadata)?;
        require_role(
            &principal,
            &[PrincipalRole::Trader, PrincipalRole::Operator],
        )?;
        let account_filter = req.account_id.as_deref().map(parse_non_empty).transpose()?;
        if let Some(ref acct) = account_filter
            && !principal.can_access_account(acct)
        {
            return Err(forbidden());
        }
        let outcome = self.store.poll_events(
            req.after_seq,
            req.max_events,
            account_filter.as_deref(),
            Some(&principal.allowed_accounts),
        )?;
        let pb_outcome = match outcome {
            PollEventsOutcome::Batch(batch) => {
                pb::poll_events_result::Outcome::Batch(to_pb_poll_events_batch(&batch))
            }
            PollEventsOutcome::Fallback(fb) => {
                pb::poll_events_result::Outcome::SnapshotFallback(to_pb_snapshot_fallback(&fb))
            }
        };
        Ok(pb::PollEventsResult {
            outcome: Some(pb_outcome),
        })
    }
}

#[tonic::async_trait]
impl DeskBlotterService for DeskBlotterGrpcService {
    type SubscribeEventsStream =
        Pin<Box<dyn Stream<Item = Result<pb::SubscribeEventsResponse, Status>> + Send>>;

    async fn import_opening_base(
        &self,
        request: Request<pb::ImportOpeningBaseRequest>,
    ) -> Result<Response<pb::ImportOpeningBaseResponse>, Status> {
        let result = match self.execute_import_opening_base(request.metadata(), request.get_ref()) {
            Ok(v) => pb::import_opening_base_response::Result::Value(v),
            Err(e) => pb::import_opening_base_response::Result::Error(to_error_detail(&e)),
        };
        Ok(Response::new(pb::ImportOpeningBaseResponse {
            result: Some(result),
        }))
    }

    async fn ingest_market_quotes(
        &self,
        request: Request<pb::IngestMarketQuotesRequest>,
    ) -> Result<Response<pb::IngestMarketQuotesResponse>, Status> {
        let result = match self.execute_ingest_market_quotes(request.metadata(), request.get_ref())
        {
            Ok(v) => pb::ingest_market_quotes_response::Result::Value(v),
            Err(e) => pb::ingest_market_quotes_response::Result::Error(to_error_detail(&e)),
        };
        Ok(Response::new(pb::IngestMarketQuotesResponse {
            result: Some(result),
        }))
    }

    async fn create_hold(
        &self,
        request: Request<pb::CreateHoldRequest>,
    ) -> Result<Response<pb::CreateHoldResponse>, Status> {
        let result = match self.execute_create_hold(request.metadata(), request.get_ref()) {
            Ok(v) => pb::create_hold_response::Result::Value(v),
            Err(e) => pb::create_hold_response::Result::Error(to_error_detail(&e)),
        };
        Ok(Response::new(pb::CreateHoldResponse {
            result: Some(result),
        }))
    }

    async fn extend_hold(
        &self,
        request: Request<pb::ExtendHoldRequest>,
    ) -> Result<Response<pb::ExtendHoldResponse>, Status> {
        let result = match self.execute_extend_hold(request.metadata(), request.get_ref()) {
            Ok(v) => pb::extend_hold_response::Result::Value(v),
            Err(e) => pb::extend_hold_response::Result::Error(to_error_detail(&e)),
        };
        Ok(Response::new(pb::ExtendHoldResponse {
            result: Some(result),
        }))
    }

    async fn confirm_hold(
        &self,
        request: Request<pb::ConfirmHoldRequest>,
    ) -> Result<Response<pb::ConfirmHoldResponse>, Status> {
        let result = match self.execute_confirm_hold(request.metadata(), request.get_ref()) {
            Ok(v) => pb::confirm_hold_response::Result::Value(v),
            Err(e) => pb::confirm_hold_response::Result::Error(to_error_detail(&e)),
        };
        Ok(Response::new(pb::ConfirmHoldResponse {
            result: Some(result),
        }))
    }

    async fn release_hold(
        &self,
        request: Request<pb::ReleaseHoldRequest>,
    ) -> Result<Response<pb::ReleaseHoldResponse>, Status> {
        let result = match self.execute_release_hold(request.metadata(), request.get_ref()) {
            Ok(v) => pb::release_hold_response::Result::Value(v),
            Err(e) => pb::release_hold_response::Result::Error(to_error_detail(&e)),
        };
        Ok(Response::new(pb::ReleaseHoldResponse {
            result: Some(result),
        }))
    }

    async fn create_committed(
        &self,
        request: Request<pb::CreateCommittedRequest>,
    ) -> Result<Response<pb::CreateCommittedResponse>, Status> {
        let result = match self.execute_create_committed(request.metadata(), request.get_ref()) {
            Ok(v) => pb::create_committed_response::Result::Value(v),
            Err(e) => pb::create_committed_response::Result::Error(to_error_detail(&e)),
        };
        Ok(Response::new(pb::CreateCommittedResponse {
            result: Some(result),
        }))
    }

    async fn cancel_committed(
        &self,
        request: Request<pb::CancelCommittedRequest>,
    ) -> Result<Response<pb::CancelCommittedResponse>, Status> {
        let result = match self.execute_cancel_committed(request.metadata(), request.get_ref()) {
            Ok(v) => pb::cancel_committed_response::Result::Value(v),
            Err(e) => pb::cancel_committed_response::Result::Error(to_error_detail(&e)),
        };
        Ok(Response::new(pb::CancelCommittedResponse {
            result: Some(result),
        }))
    }

    async fn submit_draft_earmark(
        &self,
        request: Request<pb::SubmitDraftEarmarkRequest>,
    ) -> Result<Response<pb::SubmitDraftEarmarkResponse>, Status> {
        let result = match self.execute_submit_draft(request.metadata(), request.get_ref()) {
            Ok(v) => pb::submit_draft_earmark_response::Result::Value(v),
            Err(e) => pb::submit_draft_earmark_response::Result::Error(to_error_detail(&e)),
        };
        Ok(Response::new(pb::SubmitDraftEarmarkResponse {
            result: Some(result),
        }))
    }

    async fn confirm_draft_earmark(
        &self,
        request: Request<pb::ConfirmDraftEarmarkRequest>,
    ) -> Result<Response<pb::ConfirmDraftEarmarkResponse>, Status> {
        let result = match self.execute_confirm_draft(request.metadata(), request.get_ref()) {
            Ok(v) => pb::confirm_draft_earmark_response::Result::Value(v),
            Err(e) => pb::confirm_draft_earmark_response::Result::Error(to_error_detail(&e)),
        };
        Ok(Response::new(pb::ConfirmDraftEarmarkResponse {
            result: Some(result),
        }))
    }

    async fn ingest_trade_feed(
        &self,
        request: Request<pb::IngestTradeFeedRequest>,
    ) -> Result<Response<pb::IngestTradeFeedResponse>, Status> {
        let result = match self.execute_ingest_trade_feed(request.metadata(), request.get_ref()) {
            Ok(v) => pb::ingest_trade_feed_response::Result::Value(v),
            Err(e) => pb::ingest_trade_feed_response::Result::Error(to_error_detail(&e)),
        };
        Ok(Response::new(pb::IngestTradeFeedResponse {
            result: Some(result),
        }))
    }

    async fn resolve_pending_match(
        &self,
        request: Request<pb::ResolvePendingMatchRequest>,
    ) -> Result<Response<pb::ResolvePendingMatchResponse>, Status> {
        let result = match self.execute_resolve_pending_match(request.metadata(), request.get_ref())
        {
            Ok(v) => pb::resolve_pending_match_response::Result::Value(v),
            Err(e) => pb::resolve_pending_match_response::Result::Error(to_error_detail(&e)),
        };
        Ok(Response::new(pb::ResolvePendingMatchResponse {
            result: Some(result),
        }))
    }

    async fn sweep_lifecycle(
        &self,
        request: Request<pb::SweepLifecycleRequest>,
    ) -> Result<Response<pb::SweepLifecycleResponse>, Status> {
        let result = match self.execute_sweep_lifecycle(request.metadata(), request.get_ref()) {
            Ok(v) => pb::sweep_lifecycle_response::Result::Value(v),
            Err(e) => pb::sweep_lifecycle_response::Result::Error(to_error_detail(&e)),
        };
        Ok(Response::new(pb::SweepLifecycleResponse {
            result: Some(result),
        }))
    }

    async fn set_feed_control(
        &self,
        request: Request<pb::SetFeedControlRequest>,
    ) -> Result<Response<pb::SetFeedControlResponse>, Status> {
        let result = match self.execute_set_feed_control(request.metadata(), request.get_ref()) {
            Ok(v) => pb::set_feed_control_response::Result::Value(v),
            Err(e) => pb::set_feed_control_response::Result::Error(to_error_detail(&e)),
        };
        Ok(Response::new(pb::SetFeedControlResponse {
            result: Some(result),
        }))
    }

    async fn preview_covered_hedge(
        &self,
        request: Request<pb::PreviewCoveredHedgeRequest>,
    ) -> Result<Response<pb::PreviewCoveredHedgeResponse>, Status> {
        let result = match self.execute_preview_covered_hedge(request.metadata(), request.get_ref())
        {
            Ok(v) => pb::preview_covered_hedge_response::Result::Value(v),
            Err(e) => pb::preview_covered_hedge_response::Result::Error(to_error_detail(&e)),
        };
        Ok(Response::new(pb::PreviewCoveredHedgeResponse {
            result: Some(result),
        }))
    }

    async fn get_account_snapshot(
        &self,
        request: Request<pb::GetAccountSnapshotRequest>,
    ) -> Result<Response<pb::GetAccountSnapshotResponse>, Status> {
        let result = match self.execute_get_account_snapshot(request.metadata(), request.get_ref())
        {
            Ok(v) => pb::get_account_snapshot_response::Result::Value(v),
            Err(e) => pb::get_account_snapshot_response::Result::Error(to_error_detail(&e)),
        };
        Ok(Response::new(pb::GetAccountSnapshotResponse {
            result: Some(result),
        }))
    }

    async fn poll_events(
        &self,
        request: Request<pb::PollEventsRequest>,
    ) -> Result<Response<pb::PollEventsResponse>, Status> {
        let result = match self.execute_poll_events(request.metadata(), request.get_ref()) {
            Ok(v) => pb::poll_events_response::Result::Value(v),
            Err(e) => pb::poll_events_response::Result::Error(to_error_detail(&e)),
        };
        Ok(Response::new(pb::PollEventsResponse {
            result: Some(result),
        }))
    }

    #[allow(clippy::too_many_lines)]
    async fn subscribe_events(
        &self,
        request: Request<pb::SubscribeEventsRequest>,
    ) -> Result<Response<Self::SubscribeEventsStream>, Status> {
        let (tx, rx) = mpsc::channel(self.config.subscribe_stream_channel_capacity);
        let store = self.store.clone();
        let credentials = self.credentials.clone();
        let clock = self.clock.clone();
        let config = self.config.clone();
        let metadata = request.metadata().clone();
        let req = request.into_inner();

        tokio::spawn(async move {
            let token = match extract_bearer_token(&metadata) {
                Ok(t) => t,
                Err(err) => {
                    send_sub_error(&tx, &err).await;
                    return;
                }
            };
            let initial_principal = match credentials.verify_bearer(&token, &clock.now()) {
                Ok(p) => p,
                Err(err) => {
                    send_sub_error(&tx, &err).await;
                    return;
                }
            };
            if let Err(err) = require_role(
                &initial_principal,
                &[PrincipalRole::Trader, PrincipalRole::Operator],
            ) {
                send_sub_error(&tx, &err).await;
                return;
            }
            let account_filter = match req.account_id.as_deref().map(parse_non_empty).transpose() {
                Ok(f) => f,
                Err(err) => {
                    send_sub_error(&tx, &err).await;
                    return;
                }
            };
            if let Some(ref acct) = account_filter
                && !initial_principal.can_access_account(acct)
            {
                send_sub_error(&tx, &forbidden()).await;
                return;
            }

            // Atomic handoff: capture (head_seq, oldest_retained_seq, live broadcast receiver)
            // under the exact same single-writer mutex that fires the post-commit event hook.
            let barrier = match store.subscribe_with_barrier() {
                Ok(b) => b,
                Err(err) => {
                    send_sub_error(&tx, &err).await;
                    return;
                }
            };
            if let Some(hook) = &config.on_subscribe_barrier_acquired {
                hook(barrier.head_seq);
            }

            if req.after_seq > barrier.head_seq {
                send_sub_error(&tx, &validation_failed()).await;
                return;
            }
            if barrier.head_seq > 0 && req.after_seq.saturating_add(1) < barrier.oldest_retained_seq
            {
                send_sub_fallback(
                    &tx,
                    &ficant_application::use_cases::blotter::SnapshotFallbackRequired {
                        requested_after_seq: req.after_seq,
                        oldest_retained_seq: barrier.oldest_retained_seq,
                        current_head_seq: barrier.head_seq,
                        reason: "requested cursor precedes retained event journal window"
                            .to_owned(),
                    },
                )
                .await;
                return;
            }

            let mut cursor = req.after_seq;
            let mut live_rx = barrier.receiver;

            // Phase 1: Lock-free catch-up replay from `after_seq` up to `barrier.head_seq`.
            if !replay_until_target(
                store.as_ref(),
                credentials.as_ref(),
                clock.as_ref(),
                &token,
                account_filter.as_deref(),
                config.subscribe_replay_page_size,
                &mut cursor,
                barrier.head_seq,
                &tx,
            )
            .await
            {
                return;
            }

            // Emit initial heartbeat strictly AFTER all catch-up events up to `cursor` have been sent.
            let init_head = store
                .read_workspace_snapshot()
                .map_or(cursor, |(_, h, _)| h.max(cursor));
            if tx
                .send(Ok(pb::SubscribeEventsResponse {
                    payload: Some(pb::subscribe_events_response::Payload::Heartbeat(
                        pb::StreamHeartbeat {
                            head_seq: init_head,
                            server_time: Some(to_pb_market_time(&clock.now())),
                            scanned_through_seq: cursor,
                        },
                    )),
                }))
                .await
                .is_err()
            {
                return;
            }

            // Phase 2: Live stream delivery with boundary deduplication, lag catch-up, retention
            // fallback, and continuous authorization re-checks.
            let mut hb_interval = tokio::time::interval(config.stream_heartbeat_interval);
            hb_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // Consume the immediate first tick since we just sent the post-replay heartbeat.
            let _ = hb_interval.tick().await;

            loop {
                tokio::select! {
                    recv = live_rx.recv() => {
                        match recv {
                            Ok(mut event) => {
                                if event.seq <= cursor {
                                    // Boundary duplicate already delivered in replay phase.
                                    continue;
                                }
                                let live_principal = match verify_live_subscriber(
                                    credentials.as_ref(),
                                    clock.as_ref(),
                                    &token,
                                    account_filter.as_deref(),
                                ) {
                                    Ok(p) => p,
                                    Err(err) => {
                                        send_sub_error(&tx, &err).await;
                                        return;
                                    }
                                };
                                event.changes.retain(|c| {
                                    let acct_ok = account_filter
                                        .as_deref()
                                        .is_none_or(|f| c.account_id == f);
                                    acct_ok && live_principal.can_access_account(&c.account_id)
                                });
                                let seq = event.seq;
                                if !event.changes.is_empty()
                                    && tx
                                        .send(Ok(pb::SubscribeEventsResponse {
                                            payload: Some(
                                                pb::subscribe_events_response::Payload::Event(
                                                    to_pb_event_envelope(&event),
                                                ),
                                            ),
                                        }))
                                        .await
                                        .is_err()
                                {
                                    return;
                                }
                                cursor = seq;
                            }
                            Err(broadcast::error::RecvError::Lagged(_)) => {
                                // Slow consumer fell behind broadcast buffer: re-establish atomic
                                // barrier, replay missed events from `cursor`, or emit
                                // SnapshotFallbackRequired if pruned beyond retention window.
                                let next_barrier = match store.subscribe_with_barrier() {
                                    Ok(b) => b,
                                    Err(err) => {
                                        send_sub_error(&tx, &err).await;
                                        return;
                                    }
                                };
                                if !replay_until_target(
                                    store.as_ref(),
                                    credentials.as_ref(),
                                    clock.as_ref(),
                                    &token,
                                    account_filter.as_deref(),
                                    config.subscribe_replay_page_size,
                                    &mut cursor,
                                    next_barrier.head_seq,
                                    &tx,
                                )
                                .await
                                {
                                    return;
                                }
                                live_rx = next_barrier.receiver;
                            }
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                    _ = hb_interval.tick() => {
                        if let Err(err) = verify_live_subscriber(
                            credentials.as_ref(),
                            clock.as_ref(),
                            &token,
                            account_filter.as_deref(),
                        ) {
                            send_sub_error(&tx, &err).await;
                            return;
                        }
                        let head_seq = store
                            .read_workspace_snapshot()
                            .map_or(cursor, |(_, h, _)| h.max(cursor));
                        if tx
                            .send(Ok(pb::SubscribeEventsResponse {
                                payload: Some(
                                    pb::subscribe_events_response::Payload::Heartbeat(
                                        pb::StreamHeartbeat {
                                            head_seq,
                                            server_time: Some(to_pb_market_time(&clock.now())),
                                            scanned_through_seq: cursor,
                                        },
                                    ),
                                ),
                            }))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                }
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }
}

#[allow(clippy::too_many_arguments)]
async fn replay_until_target(
    store: &dyn BlotterStorePort,
    credentials: &dyn CredentialVerifier,
    clock: &dyn BlotterClock,
    token: &str,
    account_filter: Option<&str>,
    page_size: u32,
    cursor: &mut u64,
    target_head: u64,
    tx: &mpsc::Sender<Result<pb::SubscribeEventsResponse, Status>>,
) -> bool {
    while *cursor < target_head {
        let live_principal = match verify_live_subscriber(credentials, clock, token, account_filter)
        {
            Ok(p) => p,
            Err(err) => {
                send_sub_error(tx, &err).await;
                return false;
            }
        };
        let poll = match store.poll_events(
            *cursor,
            page_size,
            account_filter,
            Some(&live_principal.allowed_accounts),
        ) {
            Ok(o) => o,
            Err(err) => {
                send_sub_error(tx, &err).await;
                return false;
            }
        };
        match poll {
            PollEventsOutcome::Fallback(fb) => {
                send_sub_fallback(tx, &fb).await;
                return false;
            }
            PollEventsOutcome::Batch(batch) => {
                for ev in batch.events {
                    if ev.seq <= *cursor {
                        continue;
                    }
                    if let Err(err) =
                        verify_live_subscriber(credentials, clock, token, account_filter)
                    {
                        send_sub_error(tx, &err).await;
                        return false;
                    }
                    let ev_seq = ev.seq;
                    if tx
                        .send(Ok(pb::SubscribeEventsResponse {
                            payload: Some(pb::subscribe_events_response::Payload::Event(
                                to_pb_event_envelope(&ev),
                            )),
                        }))
                        .await
                        .is_err()
                    {
                        return false;
                    }
                    *cursor = ev_seq;
                }
                *cursor = (*cursor).max(batch.scanned_through_seq);
                if !batch.has_more {
                    break;
                }
            }
        }
    }
    true
}

fn verify_live_subscriber(
    credentials: &dyn CredentialVerifier,
    clock: &dyn BlotterClock,
    token: &str,
    account_filter: Option<&str>,
) -> ApplicationResult<VerifiedPrincipal> {
    let principal = credentials.verify_bearer(token, &clock.now())?;
    require_role(
        &principal,
        &[PrincipalRole::Trader, PrincipalRole::Operator],
    )?;
    if let Some(acct) = account_filter
        && !principal.can_access_account(acct)
    {
        return Err(forbidden());
    }
    Ok(principal)
}

async fn send_sub_error(
    tx: &mpsc::Sender<Result<pb::SubscribeEventsResponse, Status>>,
    err: &ApplicationError,
) {
    let _ = tx
        .send(Ok(pb::SubscribeEventsResponse {
            payload: Some(pb::subscribe_events_response::Payload::Error(
                to_error_detail(err),
            )),
        }))
        .await;
}

async fn send_sub_fallback(
    tx: &mpsc::Sender<Result<pb::SubscribeEventsResponse, Status>>,
    fb: &ficant_application::use_cases::blotter::SnapshotFallbackRequired,
) {
    let _ = tx
        .send(Ok(pb::SubscribeEventsResponse {
            payload: Some(pb::subscribe_events_response::Payload::SnapshotFallback(
                to_pb_snapshot_fallback(fb),
            )),
        }))
        .await;
}

// ============================================================================
// DisabledDeskBlotterGrpcService (BP-1 Safe Default Mode)
// ============================================================================

/// Safe default `DeskBlotterService` mounted when no blotter storage is configured.
///
/// Leaves `RatesAnalyticsService` and `--health-check` fully operational without creating any
/// files on disk, while returning structured `STORAGE_UNAVAILABLE` errors on all blotter RPCs.
#[derive(Clone, Copy, Debug, Default)]
pub struct DisabledDeskBlotterGrpcService;

#[tonic::async_trait]
impl DeskBlotterService for DisabledDeskBlotterGrpcService {
    type SubscribeEventsStream =
        Pin<Box<dyn Stream<Item = Result<pb::SubscribeEventsResponse, Status>> + Send>>;

    async fn import_opening_base(
        &self,
        _request: Request<pb::ImportOpeningBaseRequest>,
    ) -> Result<Response<pb::ImportOpeningBaseResponse>, Status> {
        Ok(Response::new(pb::ImportOpeningBaseResponse {
            result: Some(pb::import_opening_base_response::Result::Error(
                to_error_detail(&storage_unavailable()),
            )),
        }))
    }

    async fn ingest_market_quotes(
        &self,
        _request: Request<pb::IngestMarketQuotesRequest>,
    ) -> Result<Response<pb::IngestMarketQuotesResponse>, Status> {
        Ok(Response::new(pb::IngestMarketQuotesResponse {
            result: Some(pb::ingest_market_quotes_response::Result::Error(
                to_error_detail(&storage_unavailable()),
            )),
        }))
    }

    async fn create_hold(
        &self,
        _request: Request<pb::CreateHoldRequest>,
    ) -> Result<Response<pb::CreateHoldResponse>, Status> {
        Ok(Response::new(pb::CreateHoldResponse {
            result: Some(pb::create_hold_response::Result::Error(to_error_detail(
                &storage_unavailable(),
            ))),
        }))
    }

    async fn extend_hold(
        &self,
        _request: Request<pb::ExtendHoldRequest>,
    ) -> Result<Response<pb::ExtendHoldResponse>, Status> {
        Ok(Response::new(pb::ExtendHoldResponse {
            result: Some(pb::extend_hold_response::Result::Error(to_error_detail(
                &storage_unavailable(),
            ))),
        }))
    }

    async fn confirm_hold(
        &self,
        _request: Request<pb::ConfirmHoldRequest>,
    ) -> Result<Response<pb::ConfirmHoldResponse>, Status> {
        Ok(Response::new(pb::ConfirmHoldResponse {
            result: Some(pb::confirm_hold_response::Result::Error(to_error_detail(
                &storage_unavailable(),
            ))),
        }))
    }

    async fn release_hold(
        &self,
        _request: Request<pb::ReleaseHoldRequest>,
    ) -> Result<Response<pb::ReleaseHoldResponse>, Status> {
        Ok(Response::new(pb::ReleaseHoldResponse {
            result: Some(pb::release_hold_response::Result::Error(to_error_detail(
                &storage_unavailable(),
            ))),
        }))
    }

    async fn create_committed(
        &self,
        _request: Request<pb::CreateCommittedRequest>,
    ) -> Result<Response<pb::CreateCommittedResponse>, Status> {
        Ok(Response::new(pb::CreateCommittedResponse {
            result: Some(pb::create_committed_response::Result::Error(
                to_error_detail(&storage_unavailable()),
            )),
        }))
    }

    async fn cancel_committed(
        &self,
        _request: Request<pb::CancelCommittedRequest>,
    ) -> Result<Response<pb::CancelCommittedResponse>, Status> {
        Ok(Response::new(pb::CancelCommittedResponse {
            result: Some(pb::cancel_committed_response::Result::Error(
                to_error_detail(&storage_unavailable()),
            )),
        }))
    }

    async fn submit_draft_earmark(
        &self,
        _request: Request<pb::SubmitDraftEarmarkRequest>,
    ) -> Result<Response<pb::SubmitDraftEarmarkResponse>, Status> {
        Ok(Response::new(pb::SubmitDraftEarmarkResponse {
            result: Some(pb::submit_draft_earmark_response::Result::Error(
                to_error_detail(&storage_unavailable()),
            )),
        }))
    }

    async fn confirm_draft_earmark(
        &self,
        _request: Request<pb::ConfirmDraftEarmarkRequest>,
    ) -> Result<Response<pb::ConfirmDraftEarmarkResponse>, Status> {
        Ok(Response::new(pb::ConfirmDraftEarmarkResponse {
            result: Some(pb::confirm_draft_earmark_response::Result::Error(
                to_error_detail(&storage_unavailable()),
            )),
        }))
    }

    async fn ingest_trade_feed(
        &self,
        _request: Request<pb::IngestTradeFeedRequest>,
    ) -> Result<Response<pb::IngestTradeFeedResponse>, Status> {
        Ok(Response::new(pb::IngestTradeFeedResponse {
            result: Some(pb::ingest_trade_feed_response::Result::Error(
                to_error_detail(&storage_unavailable()),
            )),
        }))
    }

    async fn resolve_pending_match(
        &self,
        _request: Request<pb::ResolvePendingMatchRequest>,
    ) -> Result<Response<pb::ResolvePendingMatchResponse>, Status> {
        Ok(Response::new(pb::ResolvePendingMatchResponse {
            result: Some(pb::resolve_pending_match_response::Result::Error(
                to_error_detail(&storage_unavailable()),
            )),
        }))
    }

    async fn sweep_lifecycle(
        &self,
        _request: Request<pb::SweepLifecycleRequest>,
    ) -> Result<Response<pb::SweepLifecycleResponse>, Status> {
        Ok(Response::new(pb::SweepLifecycleResponse {
            result: Some(pb::sweep_lifecycle_response::Result::Error(
                to_error_detail(&storage_unavailable()),
            )),
        }))
    }

    async fn set_feed_control(
        &self,
        _request: Request<pb::SetFeedControlRequest>,
    ) -> Result<Response<pb::SetFeedControlResponse>, Status> {
        Ok(Response::new(pb::SetFeedControlResponse {
            result: Some(pb::set_feed_control_response::Result::Error(
                to_error_detail(&storage_unavailable()),
            )),
        }))
    }

    async fn preview_covered_hedge(
        &self,
        _request: Request<pb::PreviewCoveredHedgeRequest>,
    ) -> Result<Response<pb::PreviewCoveredHedgeResponse>, Status> {
        Ok(Response::new(pb::PreviewCoveredHedgeResponse {
            result: Some(pb::preview_covered_hedge_response::Result::Error(
                to_error_detail(&storage_unavailable()),
            )),
        }))
    }

    async fn get_account_snapshot(
        &self,
        _request: Request<pb::GetAccountSnapshotRequest>,
    ) -> Result<Response<pb::GetAccountSnapshotResponse>, Status> {
        Ok(Response::new(pb::GetAccountSnapshotResponse {
            result: Some(pb::get_account_snapshot_response::Result::Error(
                to_error_detail(&storage_unavailable()),
            )),
        }))
    }

    async fn poll_events(
        &self,
        _request: Request<pb::PollEventsRequest>,
    ) -> Result<Response<pb::PollEventsResponse>, Status> {
        Ok(Response::new(pb::PollEventsResponse {
            result: Some(pb::poll_events_response::Result::Error(to_error_detail(
                &storage_unavailable(),
            ))),
        }))
    }

    async fn subscribe_events(
        &self,
        _request: Request<pb::SubscribeEventsRequest>,
    ) -> Result<Response<Self::SubscribeEventsStream>, Status> {
        let (tx, rx) = mpsc::channel(1);
        let _ = tx
            .send(Ok(pb::SubscribeEventsResponse {
                payload: Some(pb::subscribe_events_response::Payload::Error(
                    to_error_detail(&storage_unavailable()),
                )),
            }))
            .await;
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }
}

// ============================================================================
// Deterministic Hold / Alert / Staleness Watchdog
// ============================================================================

/// Controller handle for triggering deterministic sweeps on the background [`BlotterWatchdog`]
/// with zero `sleep` waits.
#[derive(Clone)]
pub struct WatchdogController {
    trigger_tx: mpsc::Sender<oneshot::Sender<ApplicationResult<OrchestratedSweepLifecycleOutput>>>,
    shutdown_tx: watch::Sender<bool>,
}

impl WatchdogController {
    /// Triggers one full lifecycle sweep on the watchdog task and awaits its exact result.
    pub async fn trigger_sweep_and_wait(
        &self,
    ) -> ApplicationResult<OrchestratedSweepLifecycleOutput> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.trigger_tx
            .send(resp_tx)
            .await
            .map_err(|_| storage_unavailable())?;
        resp_rx.await.map_err(|_| storage_unavailable())?
    }

    /// Signals the background watchdog task to shut down cleanly.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

/// Spawns the lifecycle watchdog task (hold expiration, committed alerts, feed staleness).
///
/// When `periodic_interval` is `Some(dur)`, the watchdog sweeps both periodically AND whenever
/// [`WatchdogController::trigger_sweep_and_wait`] is called. When `None` (deterministic test
/// mode), sweeps run strictly on manual trigger signals with zero wall-clock timers.
#[must_use]
pub fn spawn_blotter_watchdog(
    store: Arc<dyn BlotterStorePort>,
    clock: Arc<dyn BlotterClock>,
    engines: OwnedBlotterEngines,
    periodic_interval: Option<Duration>,
) -> WatchdogController {
    let (trigger_tx, mut trigger_rx) =
        mpsc::channel::<oneshot::Sender<ApplicationResult<OrchestratedSweepLifecycleOutput>>>(32);
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

    tokio::spawn(async move {
        let mut interval = periodic_interval.map(|dur| {
            let mut iv = tokio::time::interval(dur);
            iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            iv
        });
        if let Some(iv) = interval.as_mut() {
            // Skip immediate startup tick so explicit test clocks control first sweep cleanly.
            let _ = iv.tick().await;
        }

        loop {
            tokio::select! {
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        break;
                    }
                }
                maybe_reply = trigger_rx.recv() => {
                    let Some(reply) = maybe_reply else {
                        break;
                    };
                    let res = run_watchdog_sweep_once(store.as_ref(), &clock, &engines);
                    let _ = reply.send(res);
                }
                () = async {
                    if let Some(iv) = interval.as_mut() {
                        iv.tick().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    let _ = run_watchdog_sweep_once(store.as_ref(), &clock, &engines);
                }
            }
        }
    });

    WatchdogController {
        trigger_tx,
        shutdown_tx,
    }
}

fn run_watchdog_sweep_once(
    store: &dyn BlotterStorePort,
    clock: &Arc<dyn BlotterClock>,
    engines: &OwnedBlotterEngines,
) -> ApplicationResult<OrchestratedSweepLifecycleOutput> {
    let clock_c = clock.clone();
    let engines_c = engines.clone();
    let outcome = store.execute_write_authorized_bytes(
        None,
        Box::new(|_| Ok(())),
        Box::new(move |state| {
            let orch = DeskBlotterOrchestrator::new(clock_c.as_ref(), engines_c.as_borrowed());
            let (next_state, out, event) =
                orch.sweep_lifecycle(state, &SweepLifecycleCommand::default())?;
            let pb_out = pb::SweepLifecycleResult {
                head_seq: out.head_seq,
                replayed: false,
                expired_hold_count: out.expired_hold_count,
                newly_alerted_committed_count: out.newly_alerted_committed_count,
                newly_stale_feed_count: out.newly_stale_feed_count,
            };
            let bytes = pb_out.encode_to_vec();
            let plan = BlotterCommitPlan {
                next_state,
                event,
                idempotency_receipt: None,
            };
            Ok((bytes, plan))
        }),
    )?;
    let (bytes, _) = match outcome {
        BlotterWriteOutcome::Committed {
            response_bytes,
            committed_seq,
        } => (response_bytes, committed_seq),
        BlotterWriteOutcome::Replayed(r) => (r.response_payload, r.committed_seq),
    };
    let decoded =
        pb::SweepLifecycleResult::decode(bytes.as_slice()).map_err(|_| storage_unavailable())?;
    Ok(OrchestratedSweepLifecycleOutput {
        head_seq: decoded.head_seq,
        mutated: decoded.expired_hold_count > 0
            || decoded.newly_alerted_committed_count > 0
            || decoded.newly_stale_feed_count > 0,
        expired_hold_count: decoded.expired_hold_count,
        newly_alerted_committed_count: decoded.newly_alerted_committed_count,
        newly_stale_feed_count: decoded.newly_stale_feed_count,
    })
}

// ============================================================================
// Helpers
// ============================================================================

fn decode_or_replay<M: Message + Default>(
    outcome: BlotterWriteOutcome,
    mark_replayed: impl FnOnce(&mut M),
) -> ApplicationResult<M> {
    match outcome {
        BlotterWriteOutcome::Committed { response_bytes, .. } => {
            M::decode(response_bytes.as_slice()).map_err(|_| storage_unavailable())
        }
        BlotterWriteOutcome::Replayed(receipt) => {
            let mut decoded = M::decode(receipt.response_payload.as_slice())
                .map_err(|_| storage_unavailable())?;
            mark_replayed(&mut decoded);
            Ok(decoded)
        }
    }
}

fn require_role(principal: &VerifiedPrincipal, allowed: &[PrincipalRole]) -> ApplicationResult<()> {
    if allowed.contains(&principal.role) {
        Ok(())
    } else {
        Err(forbidden())
    }
}

const fn forbidden() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::Forbidden, false)
}

const fn not_found() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::NotFound, false)
}

const fn validation_failed() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}

const fn storage_unavailable() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::StorageUnavailable, true)
}
