"""Verify generated Python `ficant.blotter.v1` contracts against the shared Buf descriptor image."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
import shutil
import subprocess
import sys

from google.protobuf import descriptor_pb2

REPO_ROOT = Path(__file__).resolve().parents[2]
GENERATED_ROOT = (
    REPO_ROOT
    / "python"
    / "node-contracts"
    / "src"
    / "ficant_contracts"
    / "generated"
)
DESCRIPTOR_PATH = REPO_ROOT / "crates" / "ficant-contracts" / "tests" / "ficant_descriptor.bin"

if str(GENERATED_ROOT) not in sys.path:
    sys.path.insert(0, str(GENERATED_ROOT))

from ficant.blotter.v1 import model_pb2, service_pb2, service_pb2_grpc, spi_pb2, spi_pb2_grpc  # noqa: E402
from ficant.core.v1.common_pb2 import DecimalValue, Ulid, UnitRef  # noqa: E402
from ficant.core.v1.error_pb2 import ERROR_CODE_VALIDATION_FAILED, ErrorDetail  # noqa: E402

MODEL_PROTO = "ficant/blotter/v1/model.proto"
SPI_PROTO = "ficant/blotter/v1/spi.proto"
SERVICE_PROTO = "ficant/blotter/v1/service.proto"

FDP = descriptor_pb2.FieldDescriptorProto

D = ".ficant.core.v1.DecimalValue"
U = ".ficant.core.v1.Ulid"
H = ".ficant.core.v1.Sha256"
Z = ".ficant.core.v1.MarketTime"
E = ".ficant.core.v1.ErrorDetail"
BOND = ".ficant.market.v1.Bond"
CALENDAR = ".ficant.market.v1.Calendar"
RULE_PACK = ".ficant.market.v1.MarketRulePack"
PRICE_SRC = ".ficant.market.v1.PriceSourceType"
CAL_REQ = ".ficant.rates.v1.CalendarRequirement"
UNITS = ".ficant.rates.v1.AnalysisUnits"
TAX_BASIS = ".ficant.rates.v1.BondTaxBasis"
HEDGE_MEAS = ".ficant.rates.v1.FuturesHedgeMeasures"

E_BUCKET = ".ficant.blotter.v1.SettlementBucket"
E_SIDE = ".ficant.blotter.v1.TradeSide"
E_EARMARK_STATE = ".ficant.blotter.v1.EarmarkState"
E_PENDING_STATE = ".ficant.blotter.v1.PendingMatchState"
E_PENDING_REASON = ".ficant.blotter.v1.PendingMatchReason"
E_RESOLVE_ACTION = ".ficant.blotter.v1.PendingMatchResolutionAction"
E_FEED_ACTION = ".ficant.blotter.v1.TradeFeedRecordAction"
E_ELIGIBILITY = ".ficant.blotter.v1.RateCoverageEligibility"
E_COVERAGE = ".ficant.blotter.v1.CoverageStatus"
E_UNCOVERED = ".ficant.blotter.v1.UncoveredReason"
E_CAL_RES = ".ficant.blotter.v1.CalendarResolutionMode"
E_FUNDING_PREC = ".ficant.blotter.v1.FundingPrecision"
E_HEDGE_SCOPE = ".ficant.blotter.v1.PreviewHedgeScope"
E_DRAFT_TARGET = ".ficant.blotter.v1.DraftTargetState"
E_DRAFT_STATUS = ".ficant.blotter.v1.DraftEarmarkStatus"
E_FEED_HEALTH = ".ficant.blotter.v1.FeedHealthState"
E_OUTCOME_KIND = ".ficant.blotter.v1.TradeFillOutcomeKind"

M_QUOTE_TERMS = ".ficant.blotter.v1.QuoteTerms"
M_CATALOG_ENTRY = ".ficant.blotter.v1.InstrumentCatalogEntry"
M_OPENING_BAL = ".ficant.blotter.v1.OpeningInstrumentBalance"
M_BASE_EVIDENCE = ".ficant.blotter.v1.BaseEvidence"
M_OPENING_BASE = ".ficant.blotter.v1.OpeningPositionBase"
M_BASELINE_DELTA = ".ficant.blotter.v1.BaselineDelta"
M_BUCKET_VIEW = ".ficant.blotter.v1.BucketBalanceView"
M_POS_VIEW = ".ficant.blotter.v1.InstrumentPositionView"
M_EARMARK = ".ficant.blotter.v1.EarmarkRecord"
M_ALLOC_VIEW = ".ficant.blotter.v1.ReconcileAllocationView"
M_PENDING = ".ficant.blotter.v1.PendingMatchRecord"
M_ANCHOR = ".ficant.blotter.v1.SourceTextAnchor"
M_RULE_LINEAGE = ".ficant.blotter.v1.RuleLineage"
M_DRAFT = ".ficant.blotter.v1.DraftEarmarkRecord"
M_RISK_SUM = ".ficant.blotter.v1.RiskAndCoverageSummary"
M_FUNDING_SUM = ".ficant.blotter.v1.FundingGapSummary"
M_SEQ_RANGE = ".ficant.blotter.v1.SeqRange"
M_FEED_STATUS = ".ficant.blotter.v1.FeedStatusView"
M_SNAPSHOT = ".ficant.blotter.v1.AccountBlotterSnapshot"
M_ACCT_CHANGE = ".ficant.blotter.v1.AccountChange"
M_ENVELOPE = ".ficant.blotter.v1.BlotterEventEnvelope"
M_FALLBACK = ".ficant.blotter.v1.SnapshotFallbackRequired"
M_STREAM_HB = ".ficant.blotter.v1.StreamHeartbeat"
M_POLL_BATCH = ".ficant.blotter.v1.PollEventsBatch"
M_FILL = ".ficant.blotter.v1.TradeFillRecord"
M_QUOTE_TICK = ".ficant.blotter.v1.MarketQuoteTick"
M_NLP_PROPOSAL = ".ficant.blotter.v1.NlpEarmarkProposal"
M_FILL_OUTCOME = ".ficant.blotter.v1.TradeFillIngestOutcome"
M_PULL_FEED_RES = ".ficant.blotter.v1.PullTradeFeedBatchResult"
M_QUOTE_CURSOR = ".ficant.blotter.v1.QuoteCursor"
M_PULL_QUOTES_RES = ".ficant.blotter.v1.PullMarketQuotesResult"
M_FEED_HB = ".ficant.blotter.v1.FeedHeartbeat"
M_QUOTE_HB = ".ficant.blotter.v1.QuoteHeartbeat"


@dataclass(frozen=True)
class FieldSpec:
    name: str
    tag: int
    field_type: int
    type_name: str = ""
    repeated: bool = False
    proto3_optional: bool = False
    oneof: str | None = None


def f_str(name: str, tag: int) -> FieldSpec:
    return FieldSpec(name, tag, FDP.TYPE_STRING)


def f_opt_str(name: str, tag: int) -> FieldSpec:
    return FieldSpec(name, tag, FDP.TYPE_STRING, proto3_optional=True)


def f_rep_str(name: str, tag: int) -> FieldSpec:
    return FieldSpec(name, tag, FDP.TYPE_STRING, repeated=True)


def f_u64(name: str, tag: int) -> FieldSpec:
    return FieldSpec(name, tag, FDP.TYPE_UINT64)


def f_opt_u64(name: str, tag: int) -> FieldSpec:
    return FieldSpec(name, tag, FDP.TYPE_UINT64, proto3_optional=True)


def f_u32(name: str, tag: int) -> FieldSpec:
    return FieldSpec(name, tag, FDP.TYPE_UINT32)


def f_bool(name: str, tag: int) -> FieldSpec:
    return FieldSpec(name, tag, FDP.TYPE_BOOL)


def f_msg(name: str, tag: int, type_name: str) -> FieldSpec:
    return FieldSpec(name, tag, FDP.TYPE_MESSAGE, type_name=type_name)


def f_rep_msg(name: str, tag: int, type_name: str) -> FieldSpec:
    return FieldSpec(name, tag, FDP.TYPE_MESSAGE, type_name=type_name, repeated=True)


def f_oneof_msg(name: str, tag: int, type_name: str, oneof: str) -> FieldSpec:
    return FieldSpec(name, tag, FDP.TYPE_MESSAGE, type_name=type_name, oneof=oneof)


def f_enum(name: str, tag: int, type_name: str) -> FieldSpec:
    return FieldSpec(name, tag, FDP.TYPE_ENUM, type_name=type_name)


def _load_descriptor_set() -> descriptor_pb2.FileDescriptorSet:
    fds = descriptor_pb2.FileDescriptorSet()
    fds.ParseFromString(DESCRIPTOR_PATH.read_bytes())
    return fds


def _blotter_files(
    fds: descriptor_pb2.FileDescriptorSet,
) -> dict[str, descriptor_pb2.FileDescriptorProto]:
    return {f.name: f for f in fds.file if f.package == "ficant.blotter.v1"}


def _unary_response_fields(value_type: str) -> list[FieldSpec]:
    return [
        f_oneof_msg("value", 1, value_type, "result"),
        f_oneof_msg("error", 2, E, "result"),
    ]


EXPECTED_MESSAGES: dict[str, tuple[str, list[FieldSpec], set[int]]] = {
    # model.proto (24 messages)
    "QuoteTerms": (
        MODEL_PROTO,
        [
            f_oneof_msg("clean_price_per_100", 1, D, "price_or_yield"),
            f_oneof_msg("yield_to_maturity", 2, D, "price_or_yield"),
            f_msg("settlement_amount", 3, D),
        ],
        set(),
    ),
    "InstrumentCatalogEntry": (
        MODEL_PROTO,
        [
            f_str("instrument_code", 1),
            f_msg("bond", 2, BOND),
            f_enum("eligibility_class", 3, E_ELIGIBILITY),
            f_enum("calendar_requirement", 4, CAL_REQ),
        ],
        set(),
    ),
    "OpeningInstrumentBalance": (
        MODEL_PROTO,
        [
            f_str("instrument_code", 1),
            f_msg("opening_economic_notional", 2, D),
            f_msg("opening_available_notional", 3, D),
            f_msg("opening_net_t0_notional", 4, D),
            f_msg("opening_net_t1_notional", 5, D),
        ],
        set(),
    ),
    "BaseEvidence": (
        MODEL_PROTO,
        [
            f_str("source_id", 1),
            f_str("external_trade_id", 2),
            f_u64("original_feed_seq", 3),
            f_msg("original_executed_at", 4, Z),
            f_msg("old_notional", 5, D),
            f_msg("old_signed_cash", 6, D),
            f_enum("settlement_bucket", 7, E_BUCKET),
            f_str("evidence_ref", 8),
            f_msg("evidence_hash", 9, H),
            f_str("instrument_code", 10),
            f_enum("side", 11, E_SIDE),
            f_u64("original_revision", 12),
        ],
        set(),
    ),
    "OpeningPositionBase": (
        MODEL_PROTO,
        [
            f_str("account_id", 1),
            f_msg("base_id", 2, U),
            f_str("trading_date", 3),
            f_str("next_settlement_date", 4),
            f_msg("effective_from", 5, Z),
            f_msg("watermark_time", 6, Z),
            f_opt_u64("watermark_source_seq", 7),
            f_str("source_id", 8),
            f_msg("opening_cash_t0", 9, D),
            f_msg("opening_net_cash_delta_t1", 10, D),
            f_rep_msg("balances", 11, M_OPENING_BAL),
            f_rep_msg("instruments", 12, M_CATALOG_ENTRY),
            f_msg("calendar", 13, CALENDAR),
            f_enum("default_calendar_requirement", 14, CAL_REQ),
            f_msg("units", 15, UNITS),
            f_rep_msg("baseline_evidence", 16, M_BASE_EVIDENCE),
            f_u64("source_start_after_feed_seq", 17),
        ],
        set(),
    ),
    "BaselineDelta": (
        MODEL_PROTO,
        [
            f_msg("delta_notional", 1, D),
            f_msg("delta_cash", 2, D),
            f_enum("settlement_bucket", 3, E_BUCKET),
            f_msg("evidence", 4, M_BASE_EVIDENCE),
            f_str("reason", 5),
        ],
        set(),
    ),
    "BucketBalanceView": (
        MODEL_PROTO,
        [
            f_enum("settlement_bucket", 1, E_BUCKET),
            f_str("settlement_date", 2),
            f_msg("available_notional", 3, D),
            f_msg("hold_buy_notional", 4, D),
            f_msg("hold_sell_notional", 5, D),
            f_msg("committed_buy_unreconciled_notional", 6, D),
            f_msg("committed_sell_unreconciled_notional", 7, D),
            f_msg("reconciled_buy_notional", 8, D),
            f_msg("reconciled_sell_notional", 9, D),
            f_msg("pending_match_buy_unresolved_notional", 10, D),
            f_msg("pending_match_sell_unresolved_notional", 11, D),
            f_bool("sell_ambiguity_locked", 12),
        ],
        set(),
    ),
    "InstrumentPositionView": (
        MODEL_PROTO,
        [
            f_str("account_id", 1),
            f_str("instrument_code", 2),
            f_u64("position_version", 3),
            f_msg("economic_net_notional", 4, D),
            f_msg("available_t0_notional", 5, D),
            f_msg("available_t1_notional", 6, D),
            f_msg("bucket_t0", 7, M_BUCKET_VIEW),
            f_msg("bucket_t1", 8, M_BUCKET_VIEW),
            f_enum("coverage_status", 9, E_COVERAGE),
            f_enum("tax_basis", 10, TAX_BASIS),
            f_msg("pre_tax_dv01", 11, D),
            f_msg("valuation_clean_price_per_100", 12, D),
            f_msg("valuation_yield_to_maturity", 13, D),
            f_enum("calendar_resolution", 14, E_CAL_RES),
            f_enum("uncovered_reason", 15, E_UNCOVERED),
            f_str("uncovered_detail", 16),
            f_str("valuation_settlement_date", 17),
            f_str("active_quote_source_id", 18),
            f_opt_u64("active_quote_seq", 19),
        ],
        set(),
    ),
    "EarmarkRecord": (
        MODEL_PROTO,
        [
            f_msg("earmark_id", 1, U),
            f_u64("earmark_revision", 2),
            f_str("account_id", 3),
            f_str("instrument_code", 4),
            f_enum("side", 5, E_SIDE),
            f_enum("settlement_bucket", 6, E_BUCKET),
            f_str("settlement_date", 7),
            f_msg("notional", 8, D),
            f_msg("unreconciled_notional", 9, D),
            f_msg("quote_terms", 10, M_QUOTE_TERMS),
            f_msg("locked_settlement_cash", 11, D),
            f_msg("unreconciled_settlement_cash", 12, D),
            f_enum("funding_precision", 13, E_FUNDING_PREC),
            f_enum("state", 14, E_EARMARK_STATE),
            f_str("created_by", 15),
            f_msg("created_at", 16, Z),
            f_msg("expires_at", 17, Z),
            f_msg("committed_at", 18, Z),
            f_bool("overdue_alert", 19),
            f_bool("pre_close_alert", 20),
            f_bool("reopened_by_reversal", 21),
            f_str("note", 22),
            f_u64("created_seq", 23),
            f_u64("updated_seq", 24),
            f_opt_str("confirmed_by_trader_id", 25),
            f_opt_str("cancel_actor_id", 26),
            f_opt_str("cancel_reason", 27),
        ],
        set(),
    ),
    "ReconcileAllocationView": (
        MODEL_PROTO,
        [
            f_msg("allocation_id", 1, U),
            f_str("source_id", 2),
            f_str("external_trade_id", 3),
            f_u64("source_revision", 4),
            f_msg("earmark_id", 5, U),
            f_msg("pending_match_id", 6, U),
            f_msg("allocated_notional", 7, D),
            f_msg("allocated_fill_cash", 8, D),
            f_opt_u64("reversed_by_revision", 9),
            f_msg("released_earmark_cash", 10, D),
            f_msg("baseline_delta", 11, M_BASELINE_DELTA),
            f_str("actor_id", 12),
        ],
        set(),
    ),
    "PendingMatchRecord": (
        MODEL_PROTO,
        [
            f_msg("pending_match_id", 1, U),
            f_u64("pending_revision", 2),
            f_str("account_id", 3),
            f_str("instrument_code", 4),
            f_enum("side", 5, E_SIDE),
            f_enum("settlement_bucket", 6, E_BUCKET),
            f_str("settlement_date", 7),
            f_str("source_id", 8),
            f_str("external_trade_id", 9),
            f_u64("source_revision", 10),
            f_msg("original_notional", 11, D),
            f_msg("unresolved_notional", 12, D),
            f_msg("prior_baseline_notional", 13, D),
            f_msg("quote_terms", 14, M_QUOTE_TERMS),
            f_msg("executed_at", 15, Z),
            f_enum("state", 16, E_PENDING_STATE),
            f_enum("reason", 17, E_PENDING_REASON),
            f_rep_msg("candidate_earmark_ids", 18, U),
            f_u64("created_seq", 19),
            f_u64("updated_seq", 20),
            f_msg("unresolved_signed_cash", 21, D),
            f_msg("evidence", 22, M_BASE_EVIDENCE),
            f_msg("applied_delta", 23, M_BASELINE_DELTA),
            f_opt_str("resolution_actor_id", 24),
            f_opt_str("resolution_reason", 25),
        ],
        set(),
    ),
    "SourceTextAnchor": (
        MODEL_PROTO,
        [
            f_str("field_name", 1),
            f_u32("byte_start", 2),
            f_u32("byte_end", 3),
            f_str("span_text", 4),
        ],
        set(),
    ),
    "RuleLineage": (
        MODEL_PROTO,
        [
            f_msg("pack_id", 1, U),
            f_u64("version", 2),
            f_str("source", 3),
            f_msg("content_hash", 4, H),
            f_msg("parsed_constraints_hash", 5, H),
        ],
        set(),
    ),
    "DraftEarmarkRecord": (
        MODEL_PROTO,
        [
            f_msg("draft_id", 1, U),
            f_u64("draft_revision", 2),
            f_str("account_id", 3),
            f_str("instrument_code", 4),
            f_enum("side", 5, E_SIDE),
            f_enum("settlement_bucket", 6, E_BUCKET),
            f_msg("notional", 7, D),
            f_msg("quote_terms", 8, M_QUOTE_TERMS),
            f_enum("target_state", 9, E_DRAFT_TARGET),
            f_enum("status", 10, E_DRAFT_STATUS),
            f_str("raw_source_text", 11),
            f_rep_msg("anchors", 12, M_ANCHOR),
            f_msg("confidence_score", 13, D),
            f_str("parser_source_id", 14),
            f_enum("preview_coverage_status", 15, E_COVERAGE),
            f_msg("preview_delta_pre_tax_dv01", 16, D),
            f_enum("preview_uncovered_reason", 17, E_UNCOVERED),
            f_msg("preview_delta_uncovered_notional", 18, D),
            f_msg("preview_delta_cash_t0", 19, D),
            f_msg("preview_delta_cash_t1", 20, D),
            f_u64("preview_at_seq", 21),
            f_u64("preview_quote_seq", 22),
            f_msg("confirmation_token", 23, H),
            f_msg("rule_lineage", 24, M_RULE_LINEAGE),
            f_str("preview_quote_source_id", 25),
            f_msg("context_hash", 26, H),
            f_opt_str("confirmed_by_trader_id", 27),
            f_str("valuation_settlement_date", 28),
        ],
        set(),
    ),
    "RiskAndCoverageSummary": (
        MODEL_PROTO,
        [
            f_u32("covered_instrument_count", 1),
            f_u32("uncovered_instrument_count", 2),
            f_enum("covered_tax_basis", 3, TAX_BASIS),
            f_msg("covered_net_pre_tax_dv01", 4, D),
            f_msg("covered_gross_notional", 5, D),
            f_msg("uncovered_gross_notional", 6, D),
            f_msg("uncovered_net_notional", 7, D),
            f_bool("whole_account_hedge_allowed", 8),
        ],
        set(),
    ),
    "FundingGapSummary": (
        MODEL_PROTO,
        [
            f_msg("opening_cash_t0", 1, D),
            f_msg("opening_net_cash_delta_t1", 2, D),
            f_msg("net_settlement_flow_t0", 3, D),
            f_msg("net_settlement_flow_t1", 4, D),
            f_msg("projected_cash_t0", 5, D),
            f_msg("projected_cash_t1", 6, D),
            f_msg("funding_gap_t0", 7, D),
            f_msg("funding_gap_t1", 8, D),
            f_u32("clean_price_estimated_funding_count", 9),
            f_u32("unpriced_funding_earmark_count", 10),
            f_bool("funding_is_complete_exact", 11),
            f_msg("pending_match_cash_t0", 12, D),
            f_msg("pending_match_cash_t1", 13, D),
            f_u32("pending_match_count", 14),
            f_u32("pending_unknown_cash_count", 15),
            f_bool("feed_incomplete", 16),
        ],
        set(),
    ),
    "SeqRange": (MODEL_PROTO, [f_u64("first", 1), f_u64("last", 2)], set()),
    "FeedStatusView": (
        MODEL_PROTO,
        [
            f_str("source_id", 1),
            f_u64("feed_status_revision", 2),
            f_enum("health_state", 3, E_FEED_HEALTH),
            f_bool("earmarks_paused", 4),
            f_bool("paused_by_gap", 5),
            f_bool("paused_by_staleness", 6),
            f_bool("paused_by_operator", 7),
            f_msg("last_heartbeat_at", 8, Z),
            f_u64("contiguous_feed_seq", 9),
            f_u64("max_seen_feed_seq", 10),
            f_rep_msg("missing_feed_ranges", 11, M_SEQ_RANGE),
            f_u32("open_pending_match_count", 12),
            f_str("status_reason", 13),
        ],
        set(),
    ),
    "AccountBlotterSnapshot": (
        MODEL_PROTO,
        [
            f_u64("snapshot_seq", 1),
            f_str("account_id", 2),
            f_u64("account_version", 3),
            f_msg("opening_base", 4, M_OPENING_BASE),
            f_msg("risk_summary", 5, M_RISK_SUM),
            f_msg("funding_summary", 6, M_FUNDING_SUM),
            f_rep_msg("feed_statuses", 7, M_FEED_STATUS),
            f_rep_msg("positions", 8, M_POS_VIEW),
            f_rep_msg("active_earmarks", 9, M_EARMARK),
            f_rep_msg("open_pending_matches", 10, M_PENDING),
            f_rep_msg("pending_drafts", 11, M_DRAFT),
            f_rep_str("allowed_hedge_product_codes", 12),
        ],
        set(),
    ),
    "AccountChange": (
        MODEL_PROTO,
        [
            f_str("account_id", 1),
            f_msg("replace_snapshot", 2, M_SNAPSHOT),
            f_rep_msg("changed_allocations", 3, M_ALLOC_VIEW),
        ],
        set(),
    ),
    "BlotterEventEnvelope": (
        MODEL_PROTO,
        [
            f_u64("seq", 1),
            f_msg("occurred_at", 2, Z),
            f_rep_msg("changes", 3, M_ACCT_CHANGE),
            f_str("event_type", 4),
        ],
        set(),
    ),
    "SnapshotFallbackRequired": (
        MODEL_PROTO,
        [
            f_u64("requested_after_seq", 1),
            f_u64("oldest_retained_seq", 2),
            f_u64("current_head_seq", 3),
            f_str("reason", 4),
        ],
        set(),
    ),
    "StreamHeartbeat": (
        MODEL_PROTO,
        [
            f_u64("head_seq", 1),
            f_msg("server_time", 2, Z),
            f_u64("scanned_through_seq", 3),
        ],
        set(),
    ),
    "PollEventsBatch": (
        MODEL_PROTO,
        [
            f_rep_msg("events", 1, M_ENVELOPE),
            f_u64("scanned_through_seq", 2),
            f_u64("current_head_seq", 3),
            f_bool("has_more", 4),
        ],
        set(),
    ),
    # spi.proto (23 messages)
    "TradeFillRecord": (
        SPI_PROTO,
        [
            f_str("source_id", 1),
            f_str("external_trade_id", 2),
            f_u64("source_revision", 3),
            f_u64("feed_seq", 4),
            f_enum("action", 5, E_FEED_ACTION),
            f_str("account_id", 6),
            f_str("instrument_code", 7),
            f_enum("side", 8, E_SIDE),
            f_str("settlement_date", 9),
            f_msg("notional", 10, D),
            f_msg("quote_terms", 11, M_QUOTE_TERMS),
            f_msg("executed_at", 12, Z),
            f_u64("batch_first_feed_seq", 13),
            f_u64("batch_last_feed_seq", 14),
        ],
        set(),
    ),
    "MarketQuoteTick": (
        SPI_PROTO,
        [
            f_str("source_id", 1),
            f_u64("quote_seq", 2),
            f_str("instrument_code", 3),
            f_enum("source_type", 4, PRICE_SRC),
            f_msg("bid_clean_price", 5, D),
            f_msg("ask_clean_price", 6, D),
            f_msg("mid_clean_price", 7, D),
            f_msg("mid_yield_to_maturity", 8, D),
            f_msg("observed_at", 9, Z),
            f_msg("received_at", 10, Z),
        ],
        set(),
    ),
    "NlpEarmarkProposal": (
        SPI_PROTO,
        [
            f_str("parser_source_id", 1),
            f_str("model_version", 2),
            f_str("raw_source_text", 3),
            f_rep_msg("anchors", 4, M_ANCHOR),
            f_msg("confidence_score", 5, D),
            f_str("account_id", 6),
            f_str("instrument_code", 7),
            f_enum("side", 8, E_SIDE),
            f_enum("settlement_bucket", 9, E_BUCKET),
            f_msg("notional", 10, D),
            f_msg("quote_terms", 11, M_QUOTE_TERMS),
            f_enum("suggested_target_state", 12, E_DRAFT_TARGET),
        ],
        set(),
    ),
    "TradeFillIngestOutcome": (
        SPI_PROTO,
        [
            f_str("source_id", 1),
            f_str("external_trade_id", 2),
            f_u64("source_revision", 3),
            f_u64("feed_seq", 4),
            f_enum("kind", 5, E_OUTCOME_KIND),
            f_msg("pending_match_id", 6, U),
            f_rep_msg("allocation_ids", 7, U),
            f_opt_u64("effective_revision", 8),
        ],
        set(),
    ),
    "PullTradeFeedBatchResult": (
        SPI_PROTO,
        [
            f_rep_msg("fills", 1, M_FILL),
            f_u64("scanned_through_feed_seq", 2),
            f_bool("has_more", 3),
        ],
        set(),
    ),
    "QuoteCursor": (
        SPI_PROTO,
        [
            f_str("source_id", 1),
            f_str("instrument_code", 2),
            f_u64("after_quote_seq", 3),
        ],
        set(),
    ),
    "PullMarketQuotesResult": (
        SPI_PROTO,
        [
            f_rep_msg("ticks", 1, M_QUOTE_TICK),
            f_msg("next_cursor", 2, M_QUOTE_CURSOR),
            f_bool("has_more", 3),
        ],
        set(),
    ),
    "FeedHeartbeat": (
        SPI_PROTO,
        [
            f_str("source_id", 1),
            f_u64("contiguous_feed_seq", 2),
            f_msg("server_time", 3, Z),
        ],
        set(),
    ),
    "QuoteHeartbeat": (
        SPI_PROTO,
        [f_msg("cursor", 1, M_QUOTE_CURSOR), f_msg("server_time", 2, Z)],
        set(),
    ),
    "FetchOpeningPositionBaseRequest": (
        SPI_PROTO,
        [f_str("request_id", 1), f_str("account_id", 2), f_str("trading_date", 3)],
        set(),
    ),
    "FetchOpeningPositionBaseResponse": (SPI_PROTO, _unary_response_fields(M_OPENING_BASE), set()),
    "PullTradeFeedBatchRequest": (
        SPI_PROTO,
        [f_str("source_id", 1), f_u64("after_feed_seq", 2), f_u32("max_records", 3)],
        set(),
    ),
    "PullTradeFeedBatchResponse": (SPI_PROTO, _unary_response_fields(M_PULL_FEED_RES), set()),
    "StreamTradeFeedRequest": (
        SPI_PROTO,
        [f_str("source_id", 1), f_u64("after_feed_seq", 2)],
        set(),
    ),
    "StreamTradeFeedResponse": (
        SPI_PROTO,
        [
            f_oneof_msg("fill", 1, M_FILL, "payload"),
            f_oneof_msg("heartbeat", 2, M_FEED_HB, "payload"),
            f_oneof_msg("error", 3, E, "payload"),
        ],
        set(),
    ),
    "FetchTradeRevisionRequest": (
        SPI_PROTO,
        [f_str("source_id", 1), f_str("external_trade_id", 2), f_u64("source_revision", 3)],
        set(),
    ),
    "FetchTradeRevisionResponse": (SPI_PROTO, _unary_response_fields(M_FILL), set()),
    "PullMarketQuotesRequest": (
        SPI_PROTO,
        [f_msg("cursor", 1, M_QUOTE_CURSOR), f_u32("max_ticks", 2)],
        set(),
    ),
    "PullMarketQuotesResponse": (SPI_PROTO, _unary_response_fields(M_PULL_QUOTES_RES), set()),
    "StreamMarketQuotesRequest": (SPI_PROTO, [f_msg("cursor", 1, M_QUOTE_CURSOR)], set()),
    "StreamMarketQuotesResponse": (
        SPI_PROTO,
        [
            f_oneof_msg("tick", 1, M_QUOTE_TICK, "payload"),
            f_oneof_msg("heartbeat", 2, M_QUOTE_HB, "payload"),
            f_oneof_msg("error", 3, E, "payload"),
        ],
        set(),
    ),
    "ParseNlpIntentRequest": (
        SPI_PROTO,
        [f_str("request_id", 1), f_str("account_id", 2), f_str("raw_source_text", 3)],
        set(),
    ),
    "ParseNlpIntentResponse": (SPI_PROTO, _unary_response_fields(M_NLP_PROPOSAL), set()),
    # service.proto (53 messages)
    "ImportOpeningBaseRequest": (
        SERVICE_PROTO,
        [
            f_str("idempotency_key", 1),
            f_msg("opening_base", 2, M_OPENING_BASE),
            f_u64("expected_account_version", 3),
        ],
        {4, 5},
    ),
    "ImportOpeningBaseResult": (
        SERVICE_PROTO,
        [f_u64("committed_seq", 1), f_bool("replayed", 2), f_msg("snapshot", 3, M_SNAPSHOT)],
        set(),
    ),
    "ImportOpeningBaseResponse": (
        SERVICE_PROTO,
        _unary_response_fields(".ficant.blotter.v1.ImportOpeningBaseResult"),
        set(),
    ),
    "IngestMarketQuotesRequest": (
        SERVICE_PROTO,
        [
            f_str("idempotency_key", 1),
            f_str("account_id", 2),
            f_rep_msg("ticks", 3, M_QUOTE_TICK),
            f_str("source_id", 4),
        ],
        set(),
    ),
    "IngestMarketQuotesResult": (
        SERVICE_PROTO,
        [
            f_u64("committed_seq", 1),
            f_bool("replayed", 2),
            f_u32("applied_tick_count", 3),
            f_u32("skipped_stale_tick_count", 4),
            f_msg("risk_summary", 5, M_RISK_SUM),
        ],
        set(),
    ),
    "IngestMarketQuotesResponse": (
        SERVICE_PROTO,
        _unary_response_fields(".ficant.blotter.v1.IngestMarketQuotesResult"),
        set(),
    ),
    "CreateHoldRequest": (
        SERVICE_PROTO,
        [
            f_str("idempotency_key", 1),
            f_str("account_id", 2),
            f_u64("expected_position_version", 3),
            f_str("instrument_code", 4),
            f_enum("side", 5, E_SIDE),
            f_enum("settlement_bucket", 6, E_BUCKET),
            f_msg("notional", 7, D),
            f_msg("quote_terms", 8, M_QUOTE_TERMS),
            f_u32("ttl_seconds", 9),
            f_str("note", 10),
        ],
        set(),
    ),
    "CreateHoldResult": (
        SERVICE_PROTO,
        [
            f_u64("committed_seq", 1),
            f_bool("replayed", 2),
            f_msg("earmark", 3, M_EARMARK),
            f_msg("position", 4, M_POS_VIEW),
        ],
        set(),
    ),
    "CreateHoldResponse": (
        SERVICE_PROTO,
        _unary_response_fields(".ficant.blotter.v1.CreateHoldResult"),
        set(),
    ),
    "ExtendHoldRequest": (
        SERVICE_PROTO,
        [
            f_str("idempotency_key", 1),
            f_str("account_id", 2),
            f_msg("earmark_id", 3, U),
            f_u64("expected_earmark_revision", 4),
            f_u32("ttl_seconds", 5),
        ],
        set(),
    ),
    "ExtendHoldResult": (
        SERVICE_PROTO,
        [f_u64("committed_seq", 1), f_bool("replayed", 2), f_msg("earmark", 3, M_EARMARK)],
        set(),
    ),
    "ExtendHoldResponse": (
        SERVICE_PROTO,
        _unary_response_fields(".ficant.blotter.v1.ExtendHoldResult"),
        set(),
    ),
    "ConfirmHoldRequest": (
        SERVICE_PROTO,
        [
            f_str("idempotency_key", 1),
            f_str("account_id", 2),
            f_msg("earmark_id", 3, U),
            f_u64("expected_earmark_revision", 4),
            f_u64("expected_position_version", 5),
            f_msg("quote_terms", 6, M_QUOTE_TERMS),
        ],
        set(),
    ),
    "ConfirmHoldResult": (
        SERVICE_PROTO,
        [
            f_u64("committed_seq", 1),
            f_bool("replayed", 2),
            f_msg("earmark", 3, M_EARMARK),
            f_msg("position", 4, M_POS_VIEW),
            f_msg("risk_summary", 5, M_RISK_SUM),
            f_msg("funding_summary", 6, M_FUNDING_SUM),
            f_msg("reconfirmation_error", 7, E),
        ],
        set(),
    ),
    "ConfirmHoldResponse": (
        SERVICE_PROTO,
        _unary_response_fields(".ficant.blotter.v1.ConfirmHoldResult"),
        set(),
    ),
    "ReleaseHoldRequest": (
        SERVICE_PROTO,
        [
            f_str("idempotency_key", 1),
            f_str("account_id", 2),
            f_msg("earmark_id", 3, U),
            f_u64("expected_earmark_revision", 4),
            f_str("reason", 5),
        ],
        set(),
    ),
    "ReleaseHoldResult": (
        SERVICE_PROTO,
        [
            f_u64("committed_seq", 1),
            f_bool("replayed", 2),
            f_msg("earmark", 3, M_EARMARK),
            f_msg("position", 4, M_POS_VIEW),
        ],
        set(),
    ),
    "ReleaseHoldResponse": (
        SERVICE_PROTO,
        _unary_response_fields(".ficant.blotter.v1.ReleaseHoldResult"),
        set(),
    ),
    "CreateCommittedRequest": (
        SERVICE_PROTO,
        [
            f_str("idempotency_key", 1),
            f_str("account_id", 2),
            f_u64("expected_position_version", 3),
            f_str("instrument_code", 4),
            f_enum("side", 5, E_SIDE),
            f_enum("settlement_bucket", 6, E_BUCKET),
            f_msg("notional", 7, D),
            f_msg("quote_terms", 8, M_QUOTE_TERMS),
            f_str("note", 9),
        ],
        set(),
    ),
    "CreateCommittedResult": (
        SERVICE_PROTO,
        [
            f_u64("committed_seq", 1),
            f_bool("replayed", 2),
            f_msg("earmark", 3, M_EARMARK),
            f_msg("position", 4, M_POS_VIEW),
            f_msg("risk_summary", 5, M_RISK_SUM),
            f_msg("funding_summary", 6, M_FUNDING_SUM),
        ],
        set(),
    ),
    "CreateCommittedResponse": (
        SERVICE_PROTO,
        _unary_response_fields(".ficant.blotter.v1.CreateCommittedResult"),
        set(),
    ),
    "CancelCommittedRequest": (
        SERVICE_PROTO,
        [
            f_str("idempotency_key", 1),
            f_str("account_id", 2),
            f_msg("earmark_id", 3, U),
            f_u64("expected_earmark_revision", 4),
            f_str("reason", 5),
        ],
        set(),
    ),
    "CancelCommittedResult": (
        SERVICE_PROTO,
        [
            f_u64("committed_seq", 1),
            f_bool("replayed", 2),
            f_msg("earmark", 3, M_EARMARK),
            f_msg("position", 4, M_POS_VIEW),
            f_msg("risk_summary", 5, M_RISK_SUM),
            f_msg("funding_summary", 6, M_FUNDING_SUM),
        ],
        set(),
    ),
    "CancelCommittedResponse": (
        SERVICE_PROTO,
        _unary_response_fields(".ficant.blotter.v1.CancelCommittedResult"),
        set(),
    ),
    "SubmitDraftEarmarkRequest": (
        SERVICE_PROTO,
        [
            f_str("idempotency_key", 1),
            f_str("account_id", 2),
            f_msg("proposal", 3, M_NLP_PROPOSAL),
        ],
        set(),
    ),
    "SubmitDraftEarmarkResult": (
        SERVICE_PROTO,
        [f_u64("committed_seq", 1), f_bool("replayed", 2), f_msg("draft", 3, M_DRAFT)],
        set(),
    ),
    "SubmitDraftEarmarkResponse": (
        SERVICE_PROTO,
        _unary_response_fields(".ficant.blotter.v1.SubmitDraftEarmarkResult"),
        set(),
    ),
    "ConfirmDraftEarmarkRequest": (
        SERVICE_PROTO,
        [
            f_str("idempotency_key", 1),
            f_str("account_id", 2),
            f_msg("draft_id", 3, U),
            f_u64("expected_draft_revision", 4),
            f_u64("expected_position_version", 5),
            f_msg("confirmation_token", 6, H),
            f_bool("acknowledge_uncovered_dv01", 7),
        ],
        set(),
    ),
    "ConfirmDraftEarmarkResult": (
        SERVICE_PROTO,
        [
            f_u64("committed_seq", 1),
            f_bool("replayed", 2),
            f_msg("draft", 3, M_DRAFT),
            f_msg("earmark", 4, M_EARMARK),
            f_msg("position", 5, M_POS_VIEW),
            f_msg("risk_summary", 6, M_RISK_SUM),
            f_msg("funding_summary", 7, M_FUNDING_SUM),
        ],
        set(),
    ),
    "ConfirmDraftEarmarkResponse": (
        SERVICE_PROTO,
        _unary_response_fields(".ficant.blotter.v1.ConfirmDraftEarmarkResult"),
        set(),
    ),
    "IngestTradeFeedRequest": (
        SERVICE_PROTO,
        [
            f_str("idempotency_key", 1),
            f_str("source_id", 2),
            f_rep_msg("fills", 3, M_FILL),
            f_msg("heartbeat_at", 4, Z),
        ],
        set(),
    ),
    "IngestTradeFeedResult": (
        SERVICE_PROTO,
        [
            f_u64("committed_seq", 1),
            f_bool("replayed", 2),
            f_rep_msg("item_outcomes", 3, M_FILL_OUTCOME),
            f_msg("feed_status", 4, M_FEED_STATUS),
        ],
        set(),
    ),
    "IngestTradeFeedResponse": (
        SERVICE_PROTO,
        _unary_response_fields(".ficant.blotter.v1.IngestTradeFeedResult"),
        set(),
    ),
    "ResolvePendingMatchRequest": (
        SERVICE_PROTO,
        [
            f_str("idempotency_key", 1),
            f_str("account_id", 2),
            f_msg("pending_match_id", 3, U),
            f_u64("expected_pending_revision", 4),
            f_u64("expected_position_version", 5),
            f_enum("action", 6, E_RESOLVE_ACTION),
            f_opt_u64("expected_earmark_revision", 7),
            f_msg("target_earmark_id", 8, U),
            f_msg("offset_notional", 9, D),
            f_str("reason", 10),
            f_msg("baseline_delta", 11, M_BASELINE_DELTA),
        ],
        set(),
    ),
    "ResolvePendingMatchResult": (
        SERVICE_PROTO,
        [
            f_u64("committed_seq", 1),
            f_bool("replayed", 2),
            f_msg("pending_match", 3, M_PENDING),
            f_msg("updated_earmark", 4, M_EARMARK),
            f_msg("position", 5, M_POS_VIEW),
            f_msg("risk_summary", 6, M_RISK_SUM),
            f_msg("funding_summary", 7, M_FUNDING_SUM),
        ],
        set(),
    ),
    "ResolvePendingMatchResponse": (
        SERVICE_PROTO,
        _unary_response_fields(".ficant.blotter.v1.ResolvePendingMatchResult"),
        set(),
    ),
    "SweepLifecycleRequest": (
        SERVICE_PROTO,
        [
            f_str("idempotency_key", 1),
            f_opt_str("account_id", 2),
            f_opt_str("source_id", 3),
        ],
        set(),
    ),
    "SweepLifecycleResult": (
        SERVICE_PROTO,
        [
            f_u64("head_seq", 1),
            f_bool("replayed", 2),
            f_u32("expired_hold_count", 3),
            f_u32("newly_alerted_committed_count", 4),
            f_u32("newly_stale_feed_count", 5),
        ],
        set(),
    ),
    "SweepLifecycleResponse": (
        SERVICE_PROTO,
        _unary_response_fields(".ficant.blotter.v1.SweepLifecycleResult"),
        set(),
    ),
    "SetFeedControlRequest": (
        SERVICE_PROTO,
        [
            f_str("idempotency_key", 1),
            f_str("source_id", 2),
            f_u64("expected_feed_status_revision", 3),
            f_bool("paused_by_operator", 4),
            f_bool("force_clear_gap_pause", 5),
            f_str("reason", 6),
        ],
        set(),
    ),
    "SetFeedControlResult": (
        SERVICE_PROTO,
        [f_u64("committed_seq", 1), f_bool("replayed", 2), f_msg("feed_status", 3, M_FEED_STATUS)],
        set(),
    ),
    "SetFeedControlResponse": (
        SERVICE_PROTO,
        _unary_response_fields(".ficant.blotter.v1.SetFeedControlResult"),
        set(),
    ),
    "PreviewCoveredHedgeRequest": (
        SERVICE_PROTO,
        [
            f_str("account_id", 1),
            f_enum("scope", 2, E_HEDGE_SCOPE),
            f_str("product_code", 3),
            f_str("contract_code", 4),
            f_msg("ctd_dv01_per_100", 5, D),
            f_msg("conversion_factor", 6, D),
            f_msg("delivery_rule_pack", 7, RULE_PACK),
            f_msg("valuation_at", 8, Z),
            f_opt_u64("expected_snapshot_seq", 9),
        ],
        set(),
    ),
    "PreviewCoveredHedgeResult": (
        SERVICE_PROTO,
        [
            f_u64("evaluated_at_seq", 1),
            f_enum("scope", 2, E_HEDGE_SCOPE),
            f_str("product_code", 3),
            f_str("contract_code", 4),
            f_bool("zero_net_exposure", 5),
            f_enum("covered_tax_basis", 6, TAX_BASIS),
            f_u32("covered_instrument_count", 7),
            f_u32("uncovered_instrument_count", 8),
            f_msg("covered_net_pre_tax_dv01", 9, D),
            f_msg("excluded_uncovered_gross_notional", 10, D),
            f_msg("measures", 11, HEDGE_MEAS),
        ],
        set(),
    ),
    "PreviewCoveredHedgeResponse": (
        SERVICE_PROTO,
        _unary_response_fields(".ficant.blotter.v1.PreviewCoveredHedgeResult"),
        set(),
    ),
    "GetAccountSnapshotRequest": (SERVICE_PROTO, [f_str("account_id", 1)], set()),
    "GetAccountSnapshotResult": (SERVICE_PROTO, [f_msg("snapshot", 1, M_SNAPSHOT)], set()),
    "GetAccountSnapshotResponse": (
        SERVICE_PROTO,
        _unary_response_fields(".ficant.blotter.v1.GetAccountSnapshotResult"),
        set(),
    ),
    "PollEventsRequest": (
        SERVICE_PROTO,
        [f_u64("after_seq", 1), f_u32("max_events", 2), f_opt_str("account_id", 3)],
        set(),
    ),
    "PollEventsResult": (
        SERVICE_PROTO,
        [
            f_oneof_msg("batch", 1, M_POLL_BATCH, "outcome"),
            f_oneof_msg("snapshot_fallback", 2, M_FALLBACK, "outcome"),
        ],
        set(),
    ),
    "PollEventsResponse": (
        SERVICE_PROTO,
        _unary_response_fields(".ficant.blotter.v1.PollEventsResult"),
        set(),
    ),
    "SubscribeEventsRequest": (
        SERVICE_PROTO,
        [f_u64("after_seq", 1), f_opt_str("account_id", 2)],
        set(),
    ),
    "SubscribeEventsResponse": (
        SERVICE_PROTO,
        [
            f_oneof_msg("event", 1, M_ENVELOPE, "payload"),
            f_oneof_msg("heartbeat", 2, M_STREAM_HB, "payload"),
            f_oneof_msg("snapshot_fallback", 3, M_FALLBACK, "payload"),
            f_oneof_msg("error", 4, E, "payload"),
        ],
        set(),
    ),
}

EXPECTED_ENUMS: list[tuple[str, str, str, list[str]]] = [
    (MODEL_PROTO, "SettlementBucket", "SETTLEMENT_BUCKET", ["T_PLUS_0", "T_PLUS_1"]),
    (MODEL_PROTO, "TradeSide", "TRADE_SIDE", ["BUY", "SELL"]),
    (
        MODEL_PROTO,
        "EarmarkState",
        "EARMARK_STATE",
        ["HOLD", "COMMITTED", "EXPIRED", "RELEASED", "CANCELLED", "RECONCILED"],
    ),
    (
        MODEL_PROTO,
        "PendingMatchState",
        "PENDING_MATCH_STATE",
        [
            "OPEN",
            "RESOLVED_MATCHED",
            "RESOLVED_ORPHAN_ACCEPTED",
            "RELEASED",
            "REVERSED_BY_FEED",
            "RESOLVED_BASELINE_DELTA",
        ],
    ),
    (
        MODEL_PROTO,
        "PendingMatchReason",
        "PENDING_MATCH_REASON",
        [
            "AMBIGUOUS_CANDIDATES",
            "NOTIONAL_MISMATCH_OR_SPLIT",
            "PRICE_OUT_OF_TOLERANCE",
            "ORPHAN_NO_COMMITMENT",
            "BURST_REPEAT",
            "EXISTING_PENDING_ON_KEY",
            "AMENDMENT_MISMATCH",
            "BASELINE_REVISION_MANUAL",
            "INCOMPARABLE_BASIS",
            "REVISION_HOLD",
            "GAP_HOLD",
        ],
    ),
    (
        MODEL_PROTO,
        "PendingMatchResolutionAction",
        "PENDING_MATCH_RESOLUTION_ACTION",
        [
            "MATCH_FULL",
            "SPLIT_OFFSET",
            "ACCEPT_ORPHAN",
            "RELEASE_REJECT",
            "APPLY_BASELINE_DELTA",
        ],
    ),
    (
        MODEL_PROTO,
        "TradeFeedRecordAction",
        "TRADE_FEED_RECORD_ACTION",
        ["NEW_FILL", "AMEND", "CANCEL_REVERSAL"],
    ),
    (
        MODEL_PROTO,
        "RateCoverageEligibility",
        "RATE_COVERAGE_ELIGIBILITY",
        [
            "SOVEREIGN_OR_POLICY_FIXED",
            "CREDIT_OR_SPREAD_PRODUCT",
            "FLOATING_OR_INDEXED",
            "EMBEDDED_OPTION_OR_SINKING",
        ],
    ),
    (
        MODEL_PROTO,
        "CoverageStatus",
        "COVERAGE_STATUS",
        ["COVERED_PRE_TAX", "UNCOVERED", "INACTIVE_ZERO_POSITION"],
    ),
    (
        MODEL_PROTO,
        "UncoveredReason",
        "UNCOVERED_REASON",
        [
            "INSTRUMENT_SPEC_MISSING",
            "UNSUPPORTED_COUPON_STRUCTURE",
            "EMBEDDED_OPTION_OR_CALLABLE",
            "UNSUPPORTED_CONVENTION",
            "MISSING_MARKET_QUOTE",
            "CALENDAR_COVERAGE_MISSING",
            "KERNEL_PRICING_FAILED",
            "ELIGIBILITY_UNVERIFIED",
            "CREDIT_OR_NON_RATES_ISSUER",
            "STALE_QUOTE",
        ],
    ),
    (
        MODEL_PROTO,
        "CalendarResolutionMode",
        "CALENDAR_RESOLUTION_MODE",
        ["EXACT", "PROVISIONAL_WEEKEND_BEYOND_COVERAGE"],
    ),
    (
        MODEL_PROTO,
        "FundingPrecision",
        "FUNDING_PRECISION",
        [
            "EXACT_SETTLEMENT_AMOUNT",
            "EXACT_DIRTY_PRICE",
            "CLEAN_PRICE_ESTIMATE",
            "UNPRICED_YIELD_ONLY",
        ],
    ),
    (
        MODEL_PROTO,
        "PreviewHedgeScope",
        "PREVIEW_HEDGE_SCOPE",
        ["WHOLE_ACCOUNT", "COVERED_SUBSET_ONLY"],
    ),
    (MODEL_PROTO, "DraftTargetState", "DRAFT_TARGET_STATE", ["HOLD", "COMMITTED"]),
    (
        MODEL_PROTO,
        "DraftEarmarkStatus",
        "DRAFT_EARMARK_STATUS",
        ["PENDING_CONFIRMATION", "CONFIRMED", "DISCARDED"],
    ),
    (
        MODEL_PROTO,
        "FeedHealthState",
        "FEED_HEALTH_STATE",
        ["HEALTHY", "STALE", "GAP_DETECTED", "PAUSED_BY_OPERATOR"],
    ),
    (
        MODEL_PROTO,
        "PrincipalRole",
        "PRINCIPAL_ROLE",
        ["TRADER", "NLP_PARSER", "FEED_ADAPTER", "OPERATOR"],
    ),
    (
        SPI_PROTO,
        "TradeFillOutcomeKind",
        "TRADE_FILL_OUTCOME_KIND",
        [
            "PER_ITEM_DEDUPLICATED",
            "PRE_WATERMARK_IGNORED",
            "AUTO_MATCHED",
            "PENDING_MATCH",
            "BUFFERED_GAP",
            "BUFFERED_REVISION",
            "REVERSED",
            "BASELINE_REVIEW",
            "SUPERSEDED_IN_BATCH",
        ],
    ),
]


def test_shared_buf_descriptor_is_reproducible(tmp_path: Path) -> None:
    frozen = DESCRIPTOR_PATH.read_bytes()
    buf_exe = shutil.which("buf")
    assert buf_exe is not None, "buf CLI must be on PATH"
    live_path = tmp_path / "live_descriptor.bin"
    proc = subprocess.run(
        [buf_exe, "build", "interface", "--exclude-source-info", "--output", str(live_path)],
        cwd=REPO_ROOT,
        capture_output=True,
        check=False,
    )
    assert proc.returncode == 0, proc.stderr.decode("utf-8", errors="replace")
    assert live_path.read_bytes() == frozen


def test_blotter_services_and_25_rpcs_match_frozen_descriptor() -> None:
    fds = _load_descriptor_set()
    files = _blotter_files(fds)
    assert set(files) == {MODEL_PROTO, SPI_PROTO, SERVICE_PROTO}
    for file_desc in files.values():
        assert file_desc.syntax == "proto3"

    actual_services: dict[str, tuple[str, list[tuple[str, str, str, bool, bool]]]] = {}
    for fname, file_desc in files.items():
        for svc in file_desc.service:
            actual_services[svc.name] = (
                fname,
                [
                    (
                        m.name,
                        m.input_type,
                        m.output_type,
                        m.client_streaming,
                        m.server_streaming,
                    )
                    for m in svc.method
                ],
            )

    expected_services: dict[str, tuple[str, list[tuple[str, bool]]]] = {
        "DeskBlotterService": (
            SERVICE_PROTO,
            [
                ("ImportOpeningBase", False),
                ("IngestMarketQuotes", False),
                ("CreateHold", False),
                ("ExtendHold", False),
                ("ConfirmHold", False),
                ("ReleaseHold", False),
                ("CreateCommitted", False),
                ("CancelCommitted", False),
                ("SubmitDraftEarmark", False),
                ("ConfirmDraftEarmark", False),
                ("IngestTradeFeed", False),
                ("ResolvePendingMatch", False),
                ("SweepLifecycle", False),
                ("SetFeedControl", False),
                ("PreviewCoveredHedge", False),
                ("GetAccountSnapshot", False),
                ("PollEvents", False),
                ("SubscribeEvents", True),
            ],
        ),
        "PositionBaseProviderService": (SPI_PROTO, [("FetchOpeningPositionBase", False)]),
        "TradeFeedReconcilerService": (
            SPI_PROTO,
            [
                ("PullTradeFeedBatch", False),
                ("StreamTradeFeed", True),
                ("FetchTradeRevision", False),
            ],
        ),
        "MarketQuoteStreamService": (
            SPI_PROTO,
            [("PullMarketQuotes", False), ("StreamMarketQuotes", True)],
        ),
        "IntranetNlpIntentParserService": (SPI_PROTO, [("ParseNlpIntent", False)]),
    }

    assert set(actual_services) == set(expected_services)
    assert sum(len(methods) for _, methods in actual_services.values()) == 25

    for svc_name, (exp_file, exp_rpcs) in expected_services.items():
        actual_file, actual_rpcs = actual_services[svc_name]
        assert actual_file == exp_file
        assert len(actual_rpcs) == len(exp_rpcs)
        for actual, (rpc_name, server_streaming) in zip(actual_rpcs, exp_rpcs, strict=True):
            assert actual == (
                rpc_name,
                f".ficant.blotter.v1.{rpc_name}Request",
                f".ficant.blotter.v1.{rpc_name}Response",
                False,
                server_streaming,
            )


def test_blotter_all_18_enums_match_frozen_descriptor() -> None:
    fds = _load_descriptor_set()
    files = _blotter_files(fds)
    actual_enums = {
        e.name: (fname, e)
        for fname, file_desc in files.items()
        for e in file_desc.enum_type
    }
    assert len(actual_enums) == 18
    assert len(EXPECTED_ENUMS) == 18

    for exp_file, enum_name, prefix, values in EXPECTED_ENUMS:
        actual_file, edesc = actual_enums[enum_name]
        assert actual_file == exp_file
        expected_pairs = [(f"{prefix}_UNSPECIFIED", 0)] + [
            (f"{prefix}_{suffix}", idx) for idx, suffix in enumerate(values, start=1)
        ]
        actual_pairs = [(v.name, v.number) for v in edesc.value]
        assert actual_pairs == expected_pairs


def test_blotter_all_100_messages_match_frozen_descriptor() -> None:
    fds = _load_descriptor_set()
    files = _blotter_files(fds)
    actual_msgs = {
        m.name: (fname, m)
        for fname, file_desc in files.items()
        for m in file_desc.message_type
    }
    assert len(actual_msgs) == 100
    assert set(actual_msgs) == set(EXPECTED_MESSAGES)

    for msg_name, (exp_file, exp_fields, exp_reserved) in EXPECTED_MESSAGES.items():
        actual_file, mdesc = actual_msgs[msg_name]
        assert actual_file == exp_file
        assert len(mdesc.field) == len(exp_fields), f"{msg_name} field count mismatch"

        for actual, exp in zip(mdesc.field, exp_fields, strict=True):
            assert actual.name == exp.name
            assert actual.number == exp.tag
            assert actual.type == exp.field_type
            assert actual.type_name == exp.type_name
            expected_label = FDP.LABEL_REPEATED if exp.repeated else FDP.LABEL_OPTIONAL
            assert actual.label == expected_label
            assert actual.proto3_optional == exp.proto3_optional
            if exp.oneof is not None:
                assert not actual.proto3_optional
                assert actual.HasField("oneof_index")
                assert mdesc.oneof_decl[actual.oneof_index].name == exp.oneof
            elif not actual.proto3_optional:
                assert not actual.HasField("oneof_index")

        actual_reserved: set[int] = set()
        for r in mdesc.reserved_range:
            actual_reserved.update(range(r.start, r.end))
        assert actual_reserved == exp_reserved


def test_generated_python_runtime_descriptors_and_grpc_stubs_match() -> None:
    fds = _load_descriptor_set()
    files = _blotter_files(fds)
    mod_by_file = {
        MODEL_PROTO: model_pb2,
        SPI_PROTO: spi_pb2,
        SERVICE_PROTO: service_pb2,
    }

    for fname, file_desc in files.items():
        pb_mod = mod_by_file[fname]
        assert pb_mod.DESCRIPTOR.name == fname
        assert pb_mod.DESCRIPTOR.package == "ficant.blotter.v1"

        for enum_proto in file_desc.enum_type:
            rt_enum = pb_mod.DESCRIPTOR.enum_types_by_name[enum_proto.name]
            assert [(v.name, v.number) for v in rt_enum.values] == [
                (v.name, v.number) for v in enum_proto.value
            ]

        for msg_proto in file_desc.message_type:
            rt_msg = pb_mod.DESCRIPTOR.message_types_by_name[msg_proto.name]
            assert len(rt_msg.fields) == len(msg_proto.field)
            for rt_field, proto_field in zip(rt_msg.fields, msg_proto.field, strict=True):
                assert rt_field.name == proto_field.name
                assert rt_field.number == proto_field.number
                assert rt_field.type == proto_field.type
                expected_presence = proto_field.label != FDP.LABEL_REPEATED and (
                    proto_field.proto3_optional
                    or proto_field.type == FDP.TYPE_MESSAGE
                    or proto_field.HasField("oneof_index")
                )
                assert rt_field.has_presence == expected_presence

    # All 5 gRPC Stub and Servicer classes must be exported.
    assert service_pb2_grpc.DeskBlotterServiceStub is not None
    assert service_pb2_grpc.DeskBlotterServiceServicer is not None
    assert spi_pb2_grpc.PositionBaseProviderServiceStub is not None
    assert spi_pb2_grpc.PositionBaseProviderServiceServicer is not None
    assert spi_pb2_grpc.TradeFeedReconcilerServiceStub is not None
    assert spi_pb2_grpc.TradeFeedReconcilerServiceServicer is not None
    assert spi_pb2_grpc.MarketQuoteStreamServiceStub is not None
    assert spi_pb2_grpc.MarketQuoteStreamServiceServicer is not None
    assert spi_pb2_grpc.IntranetNlpIntentParserServiceStub is not None
    assert spi_pb2_grpc.IntranetNlpIntentParserServiceServicer is not None

    # RatesAnalyticsService remains intact in the shared descriptor image.
    rates_files = [f for f in fds.file if f.package == "ficant.rates.v1"]
    assert len(rates_files) == 1
    assert [s.name for s in rates_files[0].service] == ["RatesAnalyticsService"]


def test_generated_python_oneofs_presence_and_round_trip() -> None:
    unit = UnitRef(unit_id=Ulid(value="01ARZ3NDEKTSV4RRFFQ69G5FAA"), version=1)
    quote = model_pb2.QuoteTerms(
        clean_price_per_100=DecimalValue(coefficient="10025", scale=2, unit=unit),
        settlement_amount=DecimalValue(coefficient="100250000", scale=2, unit=unit),
    )
    decoded_quote = model_pb2.QuoteTerms.FromString(quote.SerializeToString())
    assert decoded_quote == quote
    assert decoded_quote.WhichOneof("price_or_yield") == "clean_price_per_100"
    assert decoded_quote.HasField("settlement_amount")

    poll_resp = service_pb2.PollEventsResponse(
        value=service_pb2.PollEventsResult(
            snapshot_fallback=model_pb2.SnapshotFallbackRequired(
                requested_after_seq=1,
                oldest_retained_seq=10,
                current_head_seq=25,
                reason="trimmed",
            )
        )
    )
    decoded_poll = service_pb2.PollEventsResponse.FromString(poll_resp.SerializeToString())
    assert decoded_poll.WhichOneof("result") == "value"
    assert decoded_poll.value.WhichOneof("outcome") == "snapshot_fallback"

    sub_resp = service_pb2.SubscribeEventsResponse(
        event=model_pb2.BlotterEventEnvelope(seq=11, event_type="INGEST_TRADE_FEED")
    )
    decoded_sub = service_pb2.SubscribeEventsResponse.FromString(sub_resp.SerializeToString())
    assert decoded_sub.WhichOneof("payload") == "event"
    assert decoded_sub.event.seq == 11

    confirm_res = service_pb2.ConfirmHoldResult(
        committed_seq=12,
        reconfirmation_error=ErrorDetail(
            code=ERROR_CODE_VALIDATION_FAILED,
            message="expired hold rejected",
        ),
    )
    decoded_confirm = service_pb2.ConfirmHoldResult.FromString(confirm_res.SerializeToString())
    assert decoded_confirm.HasField("reconfirmation_error")
    assert decoded_confirm.committed_seq == 12

    # Explicit presence on proto3 optional scalars.
    base = model_pb2.OpeningPositionBase(watermark_source_seq=0)
    assert base.HasField("watermark_source_seq")
    assert not model_pb2.OpeningPositionBase().HasField("watermark_source_seq")

    outcome = spi_pb2.TradeFillIngestOutcome(
        source_id="cfets-1",
        external_trade_id="T-1",
        source_revision=1,
        feed_seq=11,
        kind=spi_pb2.TRADE_FILL_OUTCOME_KIND_AUTO_MATCHED,
        effective_revision=1,
    )
    assert outcome.HasField("effective_revision")
    assert not spi_pb2.TradeFillIngestOutcome().HasField("effective_revision")

    sweep_req = service_pb2.SweepLifecycleRequest(idempotency_key="k1", account_id="")
    assert sweep_req.HasField("account_id")
    assert not sweep_req.HasField("source_id")


# ======================================================================
# BP-2 §7.1 — DeskBlotterClient, BlotterError & Fail-Closed Tamper Guards
# ======================================================================

from collections.abc import Callable, Iterator  # noqa: E402
from concurrent import futures  # noqa: E402
from contextlib import contextmanager  # noqa: E402

import grpc  # noqa: E402
import pytest  # noqa: E402

SDK_ROOT = REPO_ROOT / "python" / "sdk" / "src"
if str(SDK_ROOT) not in sys.path:
    sys.path.insert(0, str(SDK_ROOT))

from ficant.core.v1.error_pb2 import (  # noqa: E402
    ERROR_CODE_FORBIDDEN,
    ERROR_CODE_STORAGE_UNAVAILABLE,
    ERROR_CODE_UNAUTHENTICATED,
    FieldViolation,
)
from ficant.rates.v1 import analytics_pb2  # noqa: E402
from ficant_sdk import (  # noqa: E402
    BlotterError,
    BlotterEventApplier,
    DeskBlotterClient,
    RatesClient,
    RatesError,
)


_CURR_UNIT = UnitRef(unit_id=Ulid(value="01ARZ3NDEKTSV4RRFFQ69G5FA1"), version=1)
_DV01_UNIT = UnitRef(unit_id=Ulid(value="01ARZ3NDEKTSV4RRFFQ69G5FA4"), version=1)
_DV01_100_UNIT = UnitRef(unit_id=Ulid(value="01ARZ3NDEKTSV4RRFFQ69G5FA5"), version=1)
_PCT_UNIT = UnitRef(unit_id=Ulid(value="01ARZ3NDEKTSV4RRFFQ69G5FA6"), version=1)
_CNT_UNIT = UnitRef(unit_id=Ulid(value="01ARZ3NDEKTSV4RRFFQ69G5FA7"), version=1)
_OTHER_UNIT = UnitRef(unit_id=Ulid(value="01ARZ3NDEKTSV4RRFFQ69G5FAZ"), version=1)
_CURR_UNIT_V2 = UnitRef(unit_id=Ulid(value="01ARZ3NDEKTSV4RRFFQ69G5FA1"), version=2)


@contextmanager
def _raw_blotter_client(
    *,
    bearer_token: str | None = "test-trader-bearer-token",
    blotter_unary: dict[str, Callable[[bytes, grpc.ServicerContext], bytes]] | None = None,
    blotter_stream: dict[str, Callable[[bytes, grpc.ServicerContext], Iterator[bytes]]] | None = None,
    quote_spi_unary: dict[str, Callable[[bytes, grpc.ServicerContext], bytes]] | None = None,
    quote_spi_stream: dict[str, Callable[[bytes, grpc.ServicerContext], Iterator[bytes]]] | None = None,
    rates_unary: dict[str, Callable[[bytes, grpc.ServicerContext], bytes]] | None = None,
) -> Iterator[tuple[DeskBlotterClient, str]]:
    """Starts an in-process raw-byte gRPC server for DeskBlotterClient tamper testing."""
    generic_handlers: list[grpc.GenericRpcHandler] = []

    blotter_handlers: dict[str, grpc.RpcMethodHandler] = {}
    for method, fn in (blotter_unary or {}).items():
        blotter_handlers[method] = grpc.unary_unary_rpc_method_handler(
            fn,
            request_deserializer=lambda data: data,
            response_serializer=lambda data: data,
        )
    for method, fn in (blotter_stream or {}).items():
        blotter_handlers[method] = grpc.unary_stream_rpc_method_handler(
            fn,
            request_deserializer=lambda data: data,
            response_serializer=lambda data: data,
        )
    if blotter_handlers:
        generic_handlers.append(
            grpc.method_handlers_generic_handler(
                "ficant.blotter.v1.DeskBlotterService", blotter_handlers
            )
        )

    quote_handlers: dict[str, grpc.RpcMethodHandler] = {}
    for method, fn in (quote_spi_unary or {}).items():
        quote_handlers[method] = grpc.unary_unary_rpc_method_handler(
            fn,
            request_deserializer=lambda data: data,
            response_serializer=lambda data: data,
        )
    for method, fn in (quote_spi_stream or {}).items():
        quote_handlers[method] = grpc.unary_stream_rpc_method_handler(
            fn,
            request_deserializer=lambda data: data,
            response_serializer=lambda data: data,
        )
    if quote_handlers:
        generic_handlers.append(
            grpc.method_handlers_generic_handler(
                "ficant.blotter.v1.MarketQuoteStreamService", quote_handlers
            )
        )

    if rates_unary:
        rates_handlers = {
            method: grpc.unary_unary_rpc_method_handler(
                fn,
                request_deserializer=lambda data: data,
                response_serializer=lambda data: data,
            )
            for method, fn in rates_unary.items()
        }
        generic_handlers.append(
            grpc.method_handlers_generic_handler(
                "ficant.rates.v1.RatesAnalyticsService", rates_handlers
            )
        )

    server = grpc.server(futures.ThreadPoolExecutor(max_workers=4))
    server.add_generic_rpc_handlers(tuple(generic_handlers))
    port = server.add_insecure_port("127.0.0.1:0")
    server.start()
    endpoint = f"127.0.0.1:{port}"
    client = DeskBlotterClient(
        endpoint,
        bearer_token=bearer_token,
        insecure=True,
        timeout_seconds=5.0,
    )
    try:
        yield client, endpoint
    finally:
        client.close()
        server.stop(grace=0).wait()


def _valid_snapshot(
    account_id: str,
    snapshot_seq: int,
    *,
    account_version: int = 1,
    tax_basis: int = analytics_pb2.BOND_TAX_BASIS_PRE_TAX,
    covered_tax_basis: int = analytics_pb2.BOND_TAX_BASIS_PRE_TAX,
    uncovered_count: int = 0,
    whole_account_allowed: bool = True,
) -> model_pb2.AccountBlotterSnapshot:
    return model_pb2.AccountBlotterSnapshot(
        snapshot_seq=snapshot_seq,
        account_id=account_id,
        account_version=account_version,
        opening_base=model_pb2.OpeningPositionBase(
            account_id=account_id,
            units=analytics_pb2.AnalysisUnits(
                currency_amount=_CURR_UNIT,
                dv01=_DV01_UNIT,
                dv01_per_100=_DV01_100_UNIT,
                dimensionless=_PCT_UNIT,
                contract_count=_CNT_UNIT,
            ),
        ),
        risk_summary=model_pb2.RiskAndCoverageSummary(
            covered_instrument_count=1,
            uncovered_instrument_count=uncovered_count,
            covered_tax_basis=covered_tax_basis,
            covered_net_pre_tax_dv01=DecimalValue(
                coefficient="5000", scale=0, unit=_DV01_UNIT
            ),
            whole_account_hedge_allowed=whole_account_allowed,
        ),
        positions=[
            model_pb2.InstrumentPositionView(
                account_id=account_id,
                instrument_code="BOND_10Y",
                position_version=account_version,
                coverage_status=model_pb2.COVERAGE_STATUS_COVERED_PRE_TAX,
                tax_basis=tax_basis,
            )
        ],
    )


def _valid_hedge_request(
    scope: int = model_pb2.PREVIEW_HEDGE_SCOPE_WHOLE_ACCOUNT,
) -> service_pb2.PreviewCoveredHedgeRequest:
    return service_pb2.PreviewCoveredHedgeRequest(
        account_id="acct-A",
        scope=scope,
        product_code="T",
        contract_code="T2609",
        ctd_dv01_per_100=DecimalValue(coefficient="75", scale=3, unit=_DV01_100_UNIT),
        conversion_factor=DecimalValue(coefficient="985", scale=3, unit=_PCT_UNIT),
    )


def _valid_hedge_response(
    scope: int = model_pb2.PREVIEW_HEDGE_SCOPE_WHOLE_ACCOUNT,
) -> service_pb2.PreviewCoveredHedgeResponse:
    return service_pb2.PreviewCoveredHedgeResponse(
        value=service_pb2.PreviewCoveredHedgeResult(
            evaluated_at_seq=5,
            scope=scope,
            product_code="T",
            contract_code="T2609",
            zero_net_exposure=False,
            covered_tax_basis=analytics_pb2.BOND_TAX_BASIS_PRE_TAX,
            covered_instrument_count=1,
            uncovered_instrument_count=0,
            covered_net_pre_tax_dv01=DecimalValue(
                coefficient="5000", scale=0, unit=_DV01_UNIT
            ),
            excluded_uncovered_gross_notional=DecimalValue(
                coefficient="0", scale=0, unit=_CURR_UNIT
            ),
            measures=analytics_pb2.FuturesHedgeMeasures(
                futures_contract_dv01=DecimalValue(
                    coefficient="7614", scale=2, unit=_DV01_UNIT
                ),
                raw_contracts=DecimalValue(
                    coefficient="-656", scale=2, unit=_CNT_UNIT
                ),
                recommended_contracts=-7,
                residual_dv01=DecimalValue(
                    coefficient="-3298", scale=2, unit=_DV01_UNIT
                ),
                hedge_effectiveness=DecimalValue(
                    coefficient="934", scale=3, unit=_PCT_UNIT
                ),
                contract_notional=DecimalValue(
                    coefficient="1000000", scale=0, unit=_CURR_UNIT
                ),
            ),
        )
    )


def test_sdk_bearer_token_forwarding_and_blotter_error_mapping() -> None:
    captured_auth: list[str | None] = []

    def _capture_meta(ctx: grpc.ServicerContext) -> None:
        meta = dict(ctx.invocation_metadata())
        captured_auth.append(meta.get("authorization"))

    def _handle_snapshot(_raw: bytes, ctx: grpc.ServicerContext) -> bytes:
        _capture_meta(ctx)
        return service_pb2.GetAccountSnapshotResponse(
            value=service_pb2.GetAccountSnapshotResult(
                snapshot=_valid_snapshot("acct-A", 3)
            )
        ).SerializeToString()

    def _handle_create_hold_err(_raw: bytes, ctx: grpc.ServicerContext) -> bytes:
        _capture_meta(ctx)
        return service_pb2.CreateHoldResponse(
            error=ErrorDetail(
                code=ERROR_CODE_FORBIDDEN,
                message="forbidden",
                retryable=False,
                field_violations=[
                    FieldViolation(field="account_id", description="not permitted")
                ],
            )
        ).SerializeToString()

    def _handle_confirm_hold_lazy_expiry(_raw: bytes, ctx: grpc.ServicerContext) -> bytes:
        _capture_meta(ctx)
        return service_pb2.ConfirmHoldResponse(
            value=service_pb2.ConfirmHoldResult(
                committed_seq=17,
                replayed=True,
                earmark=model_pb2.EarmarkRecord(
                    earmark_id=Ulid(value="01ARZ3NDEKTSV4RRFFQ69G5FAA"),
                    earmark_revision=2,
                    account_id="acct-A",
                    instrument_code="BOND_10Y",
                    state=model_pb2.EARMARK_STATE_EXPIRED,
                ),
                position=model_pb2.InstrumentPositionView(
                    account_id="acct-A",
                    instrument_code="BOND_10Y",
                    tax_basis=analytics_pb2.BOND_TAX_BASIS_PRE_TAX,
                ),
                risk_summary=model_pb2.RiskAndCoverageSummary(
                    covered_tax_basis=analytics_pb2.BOND_TAX_BASIS_PRE_TAX
                ),
                funding_summary=model_pb2.FundingGapSummary(),
                reconfirmation_error=ErrorDetail(
                    code=ERROR_CODE_VALIDATION_FAILED,
                    message="insufficient available notional after expiry",
                    retryable=False,
                    field_violations=[
                        FieldViolation(
                            field="notional",
                            description="insufficient available notional",
                        )
                    ],
                ),
            )
        ).SerializeToString()

    def _handle_subscribe_err(_raw: bytes, ctx: grpc.ServicerContext) -> Iterator[bytes]:
        _capture_meta(ctx)
        yield service_pb2.SubscribeEventsResponse(
            error=ErrorDetail(
                code=ERROR_CODE_UNAUTHENTICATED,
                message="unauthenticated",
            )
        ).SerializeToString()

    def _handle_pull_quotes(_raw: bytes, ctx: grpc.ServicerContext) -> bytes:
        _capture_meta(ctx)
        return spi_pb2.PullMarketQuotesResponse(
            value=spi_pb2.PullMarketQuotesResult(
                ticks=[],
                next_cursor=spi_pb2.QuoteCursor(
                    source_id="SRC-A", instrument_code="BOND_10Y", after_quote_seq=5
                ),
                has_more=False,
            )
        ).SerializeToString()

    token = "high-entropy-desk-bearer-123"
    with _raw_blotter_client(
        bearer_token=token,
        blotter_unary={
            "GetAccountSnapshot": _handle_snapshot,
            "CreateHold": _handle_create_hold_err,
            "ConfirmHold": _handle_confirm_hold_lazy_expiry,
        },
        blotter_stream={"SubscribeEvents": _handle_subscribe_err},
        quote_spi_unary={"PullMarketQuotes": _handle_pull_quotes},
    ) as (client, _endpoint):
        snap_res = client.get_account_snapshot(
            service_pb2.GetAccountSnapshotRequest(account_id="acct-A")
        )
        assert snap_res.snapshot.snapshot_seq == 3

        with pytest.raises(BlotterError) as hold_exc:
            client.create_hold(service_pb2.CreateHoldRequest(idempotency_key="k1"))
        assert hold_exc.value.code == ERROR_CODE_FORBIDDEN
        assert hold_exc.value.committed_seq is None
        assert hold_exc.value.field_violations[0].field == "account_id"

        with pytest.raises(BlotterError) as confirm_exc:
            client.confirm_hold(service_pb2.ConfirmHoldRequest(idempotency_key="k2"))
        assert confirm_exc.value.code == ERROR_CODE_VALIDATION_FAILED
        assert confirm_exc.value.committed_seq == 17
        assert confirm_exc.value.replayed is True
        assert confirm_exc.value.earmark is not None
        assert confirm_exc.value.earmark.state == model_pb2.EARMARK_STATE_EXPIRED
        assert confirm_exc.value.position is not None
        assert confirm_exc.value.risk_summary is not None
        assert confirm_exc.value.funding_summary is not None

        with pytest.raises(BlotterError) as sub_exc:
            list(client.subscribe_events(service_pb2.SubscribeEventsRequest(after_seq=0)))
        assert sub_exc.value.code == ERROR_CODE_UNAUTHENTICATED

        q_res = client.pull_market_quotes(
            spi_pb2.PullMarketQuotesRequest(
                cursor=spi_pb2.QuoteCursor(
                    source_id="SRC-A", instrument_code="BOND_10Y", after_quote_seq=5
                ),
                max_ticks=10,
            )
        )
        assert q_res.next_cursor.after_quote_seq == 5

    assert captured_auth == [f"Bearer {token}"] * 5

    # Tampered ConfirmHold with reconfirmation_error but non-EXPIRED earmark -> RuntimeError.
    bad_confirm = service_pb2.ConfirmHoldResponse(
        value=service_pb2.ConfirmHoldResult(
            committed_seq=18,
            earmark=model_pb2.EarmarkRecord(state=model_pb2.EARMARK_STATE_HOLD),
            reconfirmation_error=ErrorDetail(
                code=ERROR_CODE_VALIDATION_FAILED, message="bad"
            ),
        )
    )
    with _raw_blotter_client(
        blotter_unary={"ConfirmHold": lambda _r, _c: bad_confirm.SerializeToString()}
    ) as (client, _):
        with pytest.raises(RuntimeError, match="EXPIRED"):
            client.confirm_hold(service_pb2.ConfirmHoldRequest(idempotency_key="k3"))


def test_sdk_whole_transaction_cursor_commit_and_seq_monotonicity_guards() -> None:
    persisted_cursors: list[int] = []
    seen_versions_at_persist: list[tuple[int, int]] = []

    applier = BlotterEventApplier(
        {
            "acct-A": _valid_snapshot("acct-A", 1, account_version=1),
            "acct-B": _valid_snapshot("acct-B", 1, account_version=1),
        },
        cursor_sink=lambda seq: (
            persisted_cursors.append(seq),
            seen_versions_at_persist.append(
                (
                    applier.snapshot_for("acct-A").account_version,  # type: ignore[union-attr]
                    applier.snapshot_for("acct-B").account_version,  # type: ignore[union-attr]
                )
            ),
        ),
    )
    assert applier.cursor_seq == 1

    # 1. Poisoned 2-account transaction (acct-A valid, acct-B tampered TAX_ADJUSTED):
    #    MUST reject whole transaction without mutating acct-A or persisting cursor!
    poisoned_env = model_pb2.BlotterEventEnvelope(
        seq=2,
        event_type="INGEST_TRADE_FEED",
        changes=[
            model_pb2.AccountChange(
                account_id="acct-A",
                replace_snapshot=_valid_snapshot("acct-A", 2, account_version=2),
            ),
            model_pb2.AccountChange(
                account_id="acct-B",
                replace_snapshot=_valid_snapshot(
                    "acct-B",
                    2,
                    account_version=2,
                    tax_basis=analytics_pb2.BOND_TAX_BASIS_TAX_ADJUSTED,
                ),
            ),
        ],
    )
    with _raw_blotter_client(
        blotter_unary={
            "PollEvents": lambda _r, _c: service_pb2.PollEventsResponse(
                value=service_pb2.PollEventsResult(
                    batch=model_pb2.PollEventsBatch(
                        events=[poisoned_env],
                        scanned_through_seq=2,
                        current_head_seq=2,
                        has_more=False,
                    )
                )
            ).SerializeToString()
        }
    ) as (client, _):
        with pytest.raises(RuntimeError, match="tax_basis"):
            client.poll_events(
                service_pb2.PollEventsRequest(after_seq=1, max_events=10),
                applier=applier,
            )

    assert applier.cursor_seq == 1
    assert persisted_cursors == []
    assert applier.snapshot_for("acct-A").account_version == 1  # type: ignore[union-attr]
    assert applier.snapshot_for("acct-B").account_version == 1  # type: ignore[union-attr]

    # 2. Valid 2-account transaction commits BOTH accounts BEFORE calling cursor_sink!
    good_env = model_pb2.BlotterEventEnvelope(
        seq=2,
        event_type="INGEST_TRADE_FEED",
        changes=[
            model_pb2.AccountChange(
                account_id="acct-A",
                replace_snapshot=_valid_snapshot("acct-A", 2, account_version=2),
            ),
            model_pb2.AccountChange(
                account_id="acct-B",
                replace_snapshot=_valid_snapshot("acct-B", 2, account_version=3),
            ),
        ],
    )
    with _raw_blotter_client(
        blotter_unary={
            "PollEvents": lambda _r, _c: service_pb2.PollEventsResponse(
                value=service_pb2.PollEventsResult(
                    batch=model_pb2.PollEventsBatch(
                        events=[good_env],
                        scanned_through_seq=2,
                        current_head_seq=2,
                        has_more=False,
                    )
                )
            ).SerializeToString()
        }
    ) as (client, _):
        client.poll_events(
            service_pb2.PollEventsRequest(after_seq=1, max_events=10),
            applier=applier,
        )

    assert applier.cursor_seq == 2
    assert persisted_cursors == [2]
    assert seen_versions_at_persist == [(2, 3)]

    # 3. Duplicate equal seq=2 and regressing seq=1 MUST raise RuntimeError on PollEvents & SubscribeEvents!
    for bad_seq, expected_kw in ((2, "duplicate"), (1, "regressing")):
        bad_env = model_pb2.BlotterEventEnvelope(
            seq=bad_seq,
            event_type="TEST",
            changes=[
                model_pb2.AccountChange(
                    account_id="acct-A",
                    replace_snapshot=_valid_snapshot("acct-A", bad_seq, account_version=9),
                )
            ],
        )
        with _raw_blotter_client(
            blotter_unary={
                "PollEvents": lambda _r, _c, env=bad_env: service_pb2.PollEventsResponse(
                    value=service_pb2.PollEventsResult(
                        batch=model_pb2.PollEventsBatch(
                            events=[env],
                            scanned_through_seq=max(2, env.seq),
                            current_head_seq=2,
                        )
                    )
                ).SerializeToString()
            },
            blotter_stream={
                "SubscribeEvents": lambda _r, _c, env=bad_env: iter(
                    [service_pb2.SubscribeEventsResponse(event=env).SerializeToString()]
                )
            },
        ) as (client, _):
            with pytest.raises(RuntimeError, match=expected_kw):
                client.poll_events(
                    service_pb2.PollEventsRequest(after_seq=2, max_events=10),
                    applier=applier,
                )
            with pytest.raises(RuntimeError, match=expected_kw):
                list(
                    client.subscribe_events(
                        service_pb2.SubscribeEventsRequest(after_seq=2),
                        applier=applier,
                    )
                )

    # Duplicate seq within the same SubscribeEvents stream (seq=3 then seq=3) -> RuntimeError!
    env3 = model_pb2.BlotterEventEnvelope(
        seq=3,
        event_type="E3",
        changes=[
            model_pb2.AccountChange(
                account_id="acct-A",
                replace_snapshot=_valid_snapshot("acct-A", 3, account_version=4),
            )
        ],
    )
    with _raw_blotter_client(
        blotter_stream={
            "SubscribeEvents": lambda _r, _c: iter(
                [
                    service_pb2.SubscribeEventsResponse(event=env3).SerializeToString(),
                    service_pb2.SubscribeEventsResponse(event=env3).SerializeToString(),
                ]
            )
        }
    ) as (client, _):
        stream = client.subscribe_events(
            service_pb2.SubscribeEventsRequest(after_seq=2), applier=applier
        )
        first = next(stream)
        assert first.event.seq == 3
        with pytest.raises(RuntimeError, match="duplicate"):
            next(stream)
    assert applier.cursor_seq == 3

    # Regressing heartbeat scanned_through_seq=2 (< 3) -> RuntimeError!
    with _raw_blotter_client(
        blotter_stream={
            "SubscribeEvents": lambda _r, _c: iter(
                [
                    service_pb2.SubscribeEventsResponse(
                        heartbeat=model_pb2.StreamHeartbeat(
                            head_seq=3, scanned_through_seq=2
                        )
                    ).SerializeToString()
                ]
            )
        }
    ) as (client, _):
        with pytest.raises(RuntimeError, match="monotonicity"):
            list(
                client.subscribe_events(
                    service_pb2.SubscribeEventsRequest(after_seq=3), applier=applier
                )
            )


def test_sdk_cross_source_quote_cursor_tamper_guards() -> None:
    req_cursor = spi_pb2.QuoteCursor(
        source_id="SRC-A", instrument_code="BOND_10Y", after_quote_seq=10
    )

    # 1. PullMarketQuotes: cross-source next_cursor
    bad_next_src = spi_pb2.PullMarketQuotesResponse(
        value=spi_pb2.PullMarketQuotesResult(
            ticks=[],
            next_cursor=spi_pb2.QuoteCursor(
                source_id="SRC-B", instrument_code="BOND_10Y", after_quote_seq=10
            ),
        )
    )
    # 2. PullMarketQuotes: cross-source tick
    bad_tick_src = spi_pb2.PullMarketQuotesResponse(
        value=spi_pb2.PullMarketQuotesResult(
            ticks=[
                spi_pb2.MarketQuoteTick(
                    source_id="SRC-B", quote_seq=11, instrument_code="BOND_10Y"
                )
            ],
            next_cursor=spi_pb2.QuoteCursor(
                source_id="SRC-A", instrument_code="BOND_10Y", after_quote_seq=11
            ),
        )
    )
    # 3. PullMarketQuotes: regressing cursor
    bad_regress = spi_pb2.PullMarketQuotesResponse(
        value=spi_pb2.PullMarketQuotesResult(
            ticks=[],
            next_cursor=spi_pb2.QuoteCursor(
                source_id="SRC-A", instrument_code="BOND_10Y", after_quote_seq=9
            ),
        )
    )
    for bad_resp, match_pat in (
        (bad_next_src, "cross-source"),
        (bad_tick_src, "cross-source"),
        (bad_regress, "regressing"),
    ):
        with _raw_blotter_client(
            quote_spi_unary={
                "PullMarketQuotes": lambda _r, _c, r=bad_resp: r.SerializeToString()
            }
        ) as (client, _):
            with pytest.raises(RuntimeError, match=match_pat):
                client.pull_market_quotes(
                    spi_pb2.PullMarketQuotesRequest(cursor=req_cursor, max_ticks=10)
                )

    # 4. StreamMarketQuotes: cross-source tick and cross-source heartbeat cursor
    bad_stream_tick = spi_pb2.StreamMarketQuotesResponse(
        tick=spi_pb2.MarketQuoteTick(
            source_id="SRC-B", quote_seq=11, instrument_code="BOND_10Y"
        )
    )
    bad_stream_hb = spi_pb2.StreamMarketQuotesResponse(
        heartbeat=spi_pb2.QuoteHeartbeat(
            cursor=spi_pb2.QuoteCursor(
                source_id="SRC-B", instrument_code="BOND_10Y", after_quote_seq=10
            )
        )
    )
    for msg in (bad_stream_tick, bad_stream_hb):
        with _raw_blotter_client(
            quote_spi_stream={
                "StreamMarketQuotes": lambda _r, _c, m=msg: iter([m.SerializeToString()])
            }
        ) as (client, _):
            with pytest.raises(RuntimeError, match="cross-source"):
                list(
                    client.stream_market_quotes(
                        spi_pb2.StreamMarketQuotesRequest(cursor=req_cursor)
                    )
                )


def test_sdk_preview_covered_hedge_and_tax_basis_tamper_guards() -> None:
    # Compliant response succeeds.
    with _raw_blotter_client(
        blotter_unary={
            "PreviewCoveredHedge": lambda _r, _c: _valid_hedge_response().SerializeToString()
        }
    ) as (client, _):
        ok = client.preview_covered_hedge(
            _valid_hedge_request(), expected_currency_unit=_CURR_UNIT
        )
        assert ok.measures.contract_notional.coefficient == "1000000"

    def _assert_hedge_rejected(
        resp: service_pb2.PreviewCoveredHedgeResponse,
        expected_substr: str,
        *,
        req: service_pb2.PreviewCoveredHedgeRequest | None = None,
        expected_unit: UnitRef | None = _CURR_UNIT,
    ) -> None:
        with _raw_blotter_client(
            blotter_unary={
                "PreviewCoveredHedge": lambda _r, _c: resp.SerializeToString()
            }
        ) as (client, _):
            with pytest.raises(RuntimeError) as exc_info:
                client.preview_covered_hedge(
                    req or _valid_hedge_request(), expected_currency_unit=expected_unit
                )
            assert expected_substr in str(exc_info.value)

    # 1. Invalid / mismatched scope & uncovered > 0 returning WHOLE_ACCOUNT
    for bad_scope in (
        model_pb2.PREVIEW_HEDGE_SCOPE_UNSPECIFIED,
        model_pb2.PREVIEW_HEDGE_SCOPE_COVERED_SUBSET_ONLY,
        99,
    ):
        mut = _valid_hedge_response()
        mut.value.scope = bad_scope
        _assert_hedge_rejected(mut, "scope")

    uncovered_whole = _valid_hedge_response(model_pb2.PREVIEW_HEDGE_SCOPE_WHOLE_ACCOUNT)
    uncovered_whole.value.uncovered_instrument_count = 1
    _assert_hedge_rejected(uncovered_whole, "WHOLE_ACCOUNT")

    # 2. Non-PRE_TAX tax basis in PreviewCoveredHedge
    for bad_basis in (
        analytics_pb2.BOND_TAX_BASIS_UNSPECIFIED,
        analytics_pb2.BOND_TAX_BASIS_TAX_ADJUSTED,
        77,
    ):
        mut = _valid_hedge_response()
        mut.value.covered_tax_basis = bad_basis
        _assert_hedge_rejected(mut, "covered_tax_basis")

    # 3. Missing / zero / negative / non-canonical contract_notional
    no_measures = _valid_hedge_response()
    no_measures.value.ClearField("measures")
    _assert_hedge_rejected(no_measures, "measures")

    no_notional = _valid_hedge_response()
    no_notional.value.measures.ClearField("contract_notional")
    _assert_hedge_rejected(no_notional, "contract_notional")

    for coeff, scale in (("0", 0), ("-1000000", 0), ("10000000", 1), ("01000", 0), ("NaN", 0)):
        mut = _valid_hedge_response()
        mut.value.measures.contract_notional.CopyFrom(
            DecimalValue(coefficient=coeff, scale=scale, unit=_CURR_UNIT)
        )
        _assert_hedge_rejected(mut, "contract_notional")

    # 4. Missing / invalid / wrong-unit (including percentage/dimensionless unit) contract_notional
    missing_unit = _valid_hedge_response()
    missing_unit.value.measures.contract_notional.ClearField("unit")
    _assert_hedge_rejected(missing_unit, "contract_notional.unit")

    zero_ver_unit = _valid_hedge_response()
    zero_ver_unit.value.measures.contract_notional.unit.version = 0
    _assert_hedge_rejected(zero_ver_unit, "contract_notional.unit")

    wrong_pct_unit = _valid_hedge_response()
    wrong_pct_unit.value.measures.contract_notional.unit.CopyFrom(_PCT_UNIT)
    _assert_hedge_rejected(wrong_pct_unit, "contract_notional.unit", expected_unit=None)

    wrong_other_unit = _valid_hedge_response()
    wrong_other_unit.value.measures.contract_notional.unit.CopyFrom(_OTHER_UNIT)
    _assert_hedge_rejected(wrong_other_unit, "contract_notional.unit")

    wrong_ver_unit = _valid_hedge_response()
    wrong_ver_unit.value.measures.contract_notional.unit.CopyFrom(_CURR_UNIT_V2)
    _assert_hedge_rejected(wrong_ver_unit, "contract_notional.unit")

    # 5. Non-PRE_TAX tax basis on GetAccountSnapshot (risk_summary & positions) -> RuntimeError
    bad_snap_risk = service_pb2.GetAccountSnapshotResponse(
        value=service_pb2.GetAccountSnapshotResult(
            snapshot=_valid_snapshot(
                "acct-A",
                4,
                covered_tax_basis=analytics_pb2.BOND_TAX_BASIS_TAX_ADJUSTED,
            )
        )
    )
    bad_snap_pos = service_pb2.GetAccountSnapshotResponse(
        value=service_pb2.GetAccountSnapshotResult(
            snapshot=_valid_snapshot(
                "acct-A",
                4,
                tax_basis=analytics_pb2.BOND_TAX_BASIS_UNSPECIFIED,
            )
        )
    )
    bad_snap_whole_flag = service_pb2.GetAccountSnapshotResponse(
        value=service_pb2.GetAccountSnapshotResult(
            snapshot=_valid_snapshot(
                "acct-A",
                4,
                uncovered_count=1,
                whole_account_allowed=True,
            )
        )
    )
    for bad_snap, substr in (
        (bad_snap_risk, "covered_tax_basis"),
        (bad_snap_pos, "tax_basis"),
        (bad_snap_whole_flag, "whole_account_hedge_allowed"),
    ):
        with _raw_blotter_client(
            blotter_unary={
                "GetAccountSnapshot": lambda _r, _c, s=bad_snap: s.SerializeToString()
            }
        ) as (client, _):
            with pytest.raises(RuntimeError, match=substr):
                client.get_account_snapshot(
                    service_pb2.GetAccountSnapshotRequest(account_id="acct-A")
                )


def test_rates_client_remains_untouched_and_green_alongside_desk_blotter_client() -> None:
    rates_err = analytics_pb2.AnalyzeBondResponse(
        error=ErrorDetail(
            code=ERROR_CODE_STORAGE_UNAVAILABLE,
            message="mock rates business error",
        )
    )
    with _raw_blotter_client(
        rates_unary={"AnalyzeBond": lambda _r, _c: rates_err.SerializeToString()}
    ) as (_blotter_client, endpoint):
        with RatesClient(endpoint, insecure=True, timeout_seconds=5.0) as rates_client:
            with pytest.raises(RatesError) as exc_info:
                rates_client.analyze_bond(
                    analytics_pb2.AnalyzeBondRequest(
                        execution_mode=analytics_pb2.BOND_TAX_BASIS_PRE_TAX
                    )
                )
            assert exc_info.value.detail.code == ERROR_CODE_STORAGE_UNAVAILABLE
