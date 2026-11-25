"""Reference trade feed sidecar (F-07) with dual transport/business cursors and frozen in-flight replay."""

from __future__ import annotations

import base64
from collections.abc import Callable, Mapping
import csv
from dataclasses import dataclass, field
from datetime import datetime, timezone
import hashlib
import io
import json
from pathlib import Path
from typing import Any, Literal

from ficant.blotter.v1 import model_pb2, service_pb2, spi_pb2
from ficant.rates.v1 import analytics_pb2 as rates_pb2

from ficant_sdk.sidecar.backoff import BackoffPolicy
from ficant_sdk.sidecar.checkpoint import (
    CheckpointCorruptionError,
    HealthProbeFn,
    JsonCheckpointStore,
    LedgerIdentity,
    SidecarError,
    compute_file_fingerprint,
    invoke_health_probe,
    reconcile_ledger_checkpoint,
)
from ficant_sdk.sidecar.codecs import (
    MARKET_TIMEZONE,
    AccountUnitsResolver,
    decimal_to_pb,
    fixed_decimal_scaled_i128,
    market_time_from_utc_datetime,
    market_time_to_rfc3339,
    parse_market_time,
    trade_fill_business_hash,
    trade_fill_payload_hash,
    validate_iso_date,
    validate_non_empty_text,
)

MAX_TRADE_FEED_BATCH_SIZE = 256
_MAX_U64 = (1 << 64) - 1

VALID_TRADE_FILL_OUTCOME_KINDS: frozenset[int] = frozenset(
    {
        spi_pb2.TRADE_FILL_OUTCOME_KIND_PER_ITEM_DEDUPLICATED,
        spi_pb2.TRADE_FILL_OUTCOME_KIND_PRE_WATERMARK_IGNORED,
        spi_pb2.TRADE_FILL_OUTCOME_KIND_AUTO_MATCHED,
        spi_pb2.TRADE_FILL_OUTCOME_KIND_PENDING_MATCH,
        spi_pb2.TRADE_FILL_OUTCOME_KIND_BUFFERED_GAP,
        spi_pb2.TRADE_FILL_OUTCOME_KIND_BUFFERED_REVISION,
        spi_pb2.TRADE_FILL_OUTCOME_KIND_REVERSED,
        spi_pb2.TRADE_FILL_OUTCOME_KIND_BASELINE_REVIEW,
        spi_pb2.TRADE_FILL_OUTCOME_KIND_SUPERSEDED_IN_BATCH,
    }
)

_OUTCOME_KIND_NAMES: dict[int, str] = {
    spi_pb2.TRADE_FILL_OUTCOME_KIND_PER_ITEM_DEDUPLICATED: "PER_ITEM_DEDUPLICATED",
    spi_pb2.TRADE_FILL_OUTCOME_KIND_PRE_WATERMARK_IGNORED: "PRE_WATERMARK_IGNORED",
    spi_pb2.TRADE_FILL_OUTCOME_KIND_AUTO_MATCHED: "AUTO_MATCHED",
    spi_pb2.TRADE_FILL_OUTCOME_KIND_PENDING_MATCH: "PENDING_MATCH",
    spi_pb2.TRADE_FILL_OUTCOME_KIND_BUFFERED_GAP: "BUFFERED_GAP",
    spi_pb2.TRADE_FILL_OUTCOME_KIND_BUFFERED_REVISION: "BUFFERED_REVISION",
    spi_pb2.TRADE_FILL_OUTCOME_KIND_REVERSED: "REVERSED",
    spi_pb2.TRADE_FILL_OUTCOME_KIND_BASELINE_REVIEW: "BASELINE_REVIEW",
    spi_pb2.TRADE_FILL_OUTCOME_KIND_SUPERSEDED_IN_BATCH: "SUPERSEDED_IN_BATCH",
}

_TRADE_ACTION_MAP: dict[str, int] = {
    "NEW_FILL": model_pb2.TRADE_FEED_RECORD_ACTION_NEW_FILL,
    "TRADE_FEED_RECORD_ACTION_NEW_FILL": model_pb2.TRADE_FEED_RECORD_ACTION_NEW_FILL,
    "AMEND": model_pb2.TRADE_FEED_RECORD_ACTION_AMEND,
    "TRADE_FEED_RECORD_ACTION_AMEND": model_pb2.TRADE_FEED_RECORD_ACTION_AMEND,
    "CANCEL_REVERSAL": model_pb2.TRADE_FEED_RECORD_ACTION_CANCEL_REVERSAL,
    "TRADE_FEED_RECORD_ACTION_CANCEL_REVERSAL": model_pb2.TRADE_FEED_RECORD_ACTION_CANCEL_REVERSAL,
}

_TRADE_SIDE_MAP: dict[str, int] = {
    "BUY": model_pb2.TRADE_SIDE_BUY,
    "TRADE_SIDE_BUY": model_pb2.TRADE_SIDE_BUY,
    "SELL": model_pb2.TRADE_SIDE_SELL,
    "TRADE_SIDE_SELL": model_pb2.TRADE_SIDE_SELL,
}

_ALLOWED_TRADE_ROW_KEYS = frozenset(
    {
        "source_id",
        "external_trade_id",
        "source_revision",
        "feed_seq",
        "action",
        "account_id",
        "instrument_code",
        "side",
        "settlement_date",
        "notional",
        "clean_price_per_100",
        "yield_to_maturity",
        "settlement_amount",
        "executed_at",
        "batch_first_feed_seq",
        "batch_last_feed_seq",
        "market_timezone",
        "executed_local_trading_date",
    }
)


@dataclass(frozen=True)
class TradeFeedConfig:
    """Configuration for ``TradeFeedSidecar``."""

    sidecar_id: str
    source_id: str
    trading_date: str
    checkpoint_path: Path
    input_path: Path
    batch_size: int = 64
    input_format: Literal["auto", "csv", "jsonl"] = "auto"
    market_timezone: str = MARKET_TIMEZONE
    heartbeat_at: str | None = None
    heartbeat_interval_seconds: float | None = 30.0
    send_idle_heartbeat: bool = False
    expected_ledger: tuple[int, str] | None = None
    reset_checkpoint_for_restore: bool = False
    allow_snapshot_discovery: bool = False
    explicit_units: (
        rates_pb2.AnalysisUnits | Mapping[str, rates_pb2.AnalysisUnits] | None
    ) = None
    backoff: BackoffPolicy = field(default_factory=BackoffPolicy)
    endpoint: str | None = None
    insecure: bool = False
    ca_cert_path: Path | None = None
    root_certificates: bytes | None = None
    health_timeout_seconds: float = 2.0

    def __post_init__(self) -> None:
        validate_non_empty_text(self.sidecar_id, "sidecar_id")
        validate_non_empty_text(self.source_id, "source_id")
        validate_iso_date(self.trading_date, "trading_date")
        if (
            isinstance(self.batch_size, bool)
            or not isinstance(self.batch_size, int)
            or self.batch_size < 1
            or self.batch_size > MAX_TRADE_FEED_BATCH_SIZE
        ):
            raise ValueError(
                f"batch_size must be an integer in 1..={MAX_TRADE_FEED_BATCH_SIZE}"
            )
        if self.input_format not in ("auto", "csv", "jsonl"):
            raise ValueError("input_format must be 'auto', 'csv', or 'jsonl'")
        validate_non_empty_text(self.market_timezone, "market_timezone")
        if self.heartbeat_at is not None:
            parse_market_time(self.heartbeat_at, market_timezone=self.market_timezone)
        if self.heartbeat_interval_seconds is not None and self.heartbeat_interval_seconds <= 0:
            raise ValueError("heartbeat_interval_seconds must be positive when set")
        if self.expected_ledger is not None:
            LedgerIdentity(
                ledger_epoch=self.expected_ledger[0],
                ledger_instance_id=self.expected_ledger[1],
            )
        if self.health_timeout_seconds <= 0:
            raise ValueError("health_timeout_seconds must be positive")


@dataclass(frozen=True)
class TradeFeedBatchReport:
    """Summary report produced by ``TradeFeedSidecar.run_once``."""

    ledger_epoch: int
    ledger_instance_id: str
    source_id: str
    batches_sent: int
    fills_read: int
    fills_sent: int
    fills_skipped_dedup: int
    replayed_in_flight: bool
    quarantined_previous_in_flight: bool
    contiguous_feed_seq: int
    max_seen_feed_seq: int
    missing_ranges: tuple[tuple[int, int], ...]
    backfill_from_feed_seq: int
    outcome_counts: dict[str, int]
    committed_seqs: tuple[int, ...]


def _parse_positive_u64(raw: Any, field_name: str) -> int:
    if isinstance(raw, bool):
        raise ValueError(f"{field_name} must be a positive integer, not bool")
    if isinstance(raw, int):
        val = raw
    elif isinstance(raw, str):
        text = raw.strip()
        if not text or not text.isascii() or not text.isdigit():
            raise ValueError(f"{field_name} must be a positive integer, got {raw!r}")
        val = int(text)
    else:
        raise ValueError(f"{field_name} must be a positive integer, got {type(raw).__name__}")
    if val < 1 or val > _MAX_U64:
        raise ValueError(f"{field_name} out of uint64 range [1, 2^64-1]: {val}")
    return val


def _optional_cell(row: Mapping[str, Any], key: str) -> Any | None:
    if key not in row:
        return None
    val = row[key]
    if val is None:
        return None
    if isinstance(val, str) and val.strip() == "":
        return None
    return val.strip() if isinstance(val, str) else val


def _parse_trade_action(raw: Any) -> int:
    if not isinstance(raw, str):
        raise ValueError(
            f"action must be NEW_FILL, AMEND, or CANCEL_REVERSAL; got {raw!r}"
        )
    resolved = _TRADE_ACTION_MAP.get(raw.strip().upper())
    if resolved is None:
        raise ValueError(f"unsupported TradeFeedRecordAction: {raw!r}")
    return resolved


def _parse_trade_side(raw: Any) -> int:
    if not isinstance(raw, str):
        raise ValueError(f"side must be BUY or SELL; got {raw!r}")
    resolved = _TRADE_SIDE_MAP.get(raw.strip().upper())
    if resolved is None:
        raise ValueError(f"unsupported TradeSide: {raw!r}")
    return resolved


def _business_key(source_id: str, external_trade_id: str, source_revision: int) -> str:
    return f"{source_id}\x1f{external_trade_id}\x1f{source_revision}"


def parse_trade_fill_row(
    row: Mapping[str, Any],
    *,
    expected_source_id: str,
    units_resolver: AccountUnitsResolver,
    default_market_timezone: str = MARKET_TIMEZONE,
) -> tuple[spi_pb2.TradeFillRecord, str, str]:
    """Parse and validate a trade fill row into ``(TradeFillRecord, payload_hash_hex, business_hash_hex)``."""
    unknown = set(row.keys()) - _ALLOWED_TRADE_ROW_KEYS
    if unknown:
        raise ValueError(f"unknown trade fill row field(s): {sorted(unknown)}")

    source_id = validate_non_empty_text(
        row.get("source_id").strip() if isinstance(row.get("source_id"), str) else row.get("source_id"),
        "source_id",
    )
    if source_id != expected_source_id:
        raise ValueError(
            f"trade fill source_id {source_id!r} does not match configured source_id {expected_source_id!r}"
        )

    external_trade_id = validate_non_empty_text(
        row.get("external_trade_id").strip()
        if isinstance(row.get("external_trade_id"), str)
        else row.get("external_trade_id"),
        "external_trade_id",
    )
    source_revision = _parse_positive_u64(row.get("source_revision"), "source_revision")
    feed_seq = _parse_positive_u64(row.get("feed_seq"), "feed_seq")
    action = _parse_trade_action(row.get("action"))
    account_id = validate_non_empty_text(
        row.get("account_id").strip() if isinstance(row.get("account_id"), str) else row.get("account_id"),
        "account_id",
    )
    instrument_code = validate_non_empty_text(
        row.get("instrument_code").strip()
        if isinstance(row.get("instrument_code"), str)
        else row.get("instrument_code"),
        "instrument_code",
    )
    side = _parse_trade_side(row.get("side"))
    settlement_date = validate_iso_date(
        row.get("settlement_date").strip()
        if isinstance(row.get("settlement_date"), str)
        else row.get("settlement_date"),
        "settlement_date",
    ).isoformat()

    raw_first = _optional_cell(row, "batch_first_feed_seq")
    raw_last = _optional_cell(row, "batch_last_feed_seq")
    batch_first_feed_seq = (
        _parse_positive_u64(raw_first, "batch_first_feed_seq")
        if raw_first is not None
        else feed_seq
    )
    batch_last_feed_seq = (
        _parse_positive_u64(raw_last, "batch_last_feed_seq")
        if raw_last is not None
        else feed_seq
    )
    if not (batch_first_feed_seq <= feed_seq <= batch_last_feed_seq):
        raise ValueError(
            f"batch bounds violated: batch_first_feed_seq={batch_first_feed_seq} <= "
            f"feed_seq={feed_seq} <= batch_last_feed_seq={batch_last_feed_seq} required"
        )

    units = units_resolver.resolve(account_id)

    notional_raw = _optional_cell(row, "notional")
    if notional_raw is None:
        raise ValueError("trade fill requires notional")
    notional_pb = decimal_to_pb(notional_raw, units.currency_amount)
    notional_scaled = fixed_decimal_scaled_i128(notional_pb)
    if action in (
        model_pb2.TRADE_FEED_RECORD_ACTION_NEW_FILL,
        model_pb2.TRADE_FEED_RECORD_ACTION_AMEND,
    ):
        if notional_scaled <= 0:
            raise ValueError("NEW_FILL and AMEND notional must be strictly positive")
    else:
        if notional_scaled < 0:
            raise ValueError("CANCEL_REVERSAL notional must be non-negative")

    clean_raw = _optional_cell(row, "clean_price_per_100")
    ytm_raw = _optional_cell(row, "yield_to_maturity")
    settle_amt_raw = _optional_cell(row, "settlement_amount")

    if clean_raw is not None and ytm_raw is not None:
        raise ValueError(
            "trade fill cannot specify both clean_price_per_100 and yield_to_maturity"
        )

    quote_terms_pb: model_pb2.QuoteTerms | None = None
    if clean_raw is not None or ytm_raw is not None or settle_amt_raw is not None:
        qt_kwargs: dict[str, Any] = {}
        if clean_raw is not None:
            clean_pb = decimal_to_pb(clean_raw, units.price_per_100)
            if fixed_decimal_scaled_i128(clean_pb) <= 0:
                raise ValueError("clean_price_per_100 must be strictly positive")
            qt_kwargs["clean_price_per_100"] = clean_pb
        if ytm_raw is not None:
            ytm_pb = decimal_to_pb(ytm_raw, units.rate)
            if fixed_decimal_scaled_i128(ytm_pb) < 0:
                raise ValueError("yield_to_maturity must be non-negative")
            qt_kwargs["yield_to_maturity"] = ytm_pb
        if settle_amt_raw is not None:
            settle_pb = decimal_to_pb(settle_amt_raw, units.currency_amount)
            if fixed_decimal_scaled_i128(settle_pb) <= 0:
                raise ValueError("settlement_amount must be strictly positive")
            qt_kwargs["settlement_amount"] = settle_pb
        quote_terms_pb = model_pb2.QuoteTerms(**qt_kwargs)

    tz_raw = _optional_cell(row, "market_timezone")
    market_tz = (
        validate_non_empty_text(tz_raw, "market_timezone")
        if tz_raw is not None
        else default_market_timezone
    )
    exec_raw = _optional_cell(row, "executed_at")
    if exec_raw is None:
        raise ValueError("trade fill requires executed_at")
    exec_date_raw = _optional_cell(row, "executed_local_trading_date")
    executed_at = parse_market_time(
        exec_raw,
        market_timezone=market_tz,
        local_trading_date=exec_date_raw,
    )

    fill_kwargs: dict[str, Any] = {
        "source_id": source_id,
        "external_trade_id": external_trade_id,
        "source_revision": source_revision,
        "feed_seq": feed_seq,
        "action": action,
        "account_id": account_id,
        "instrument_code": instrument_code,
        "side": side,
        "settlement_date": settlement_date,
        "notional": notional_pb,
        "executed_at": executed_at,
        "batch_first_feed_seq": batch_first_feed_seq,
        "batch_last_feed_seq": batch_last_feed_seq,
    }
    if quote_terms_pb is not None:
        fill_kwargs["quote_terms"] = quote_terms_pb

    fill = spi_pb2.TradeFillRecord(**fill_kwargs)
    p_hash_hex = trade_fill_payload_hash(fill).hex()
    b_hash_hex = trade_fill_business_hash(fill).hex()
    return (fill, p_hash_hex, b_hash_hex)


def validate_trade_feed_result(
    request: service_pb2.IngestTradeFeedRequest,
    result: service_pb2.IngestTradeFeedResult,
) -> dict[str, int]:
    """Strictly validate ``IngestTradeFeedResult`` before advancing any checkpoint cursor.

    Accepts all 9 legal ``TradeFillOutcomeKind`` values (``1..=9``) and rejects ``0``
    (``UNSPECIFIED``) or any unknown discriminant.
    """
    if not result.HasField("feed_status"):
        raise SidecarError("IngestTradeFeedResult missing feed_status")
    if result.feed_status.source_id != request.source_id:
        raise SidecarError(
            f"IngestTradeFeedResult feed_status.source_id mismatch: "
            f"expected {request.source_id!r}, got {result.feed_status.source_id!r}"
        )
    if len(result.item_outcomes) != len(request.fills):
        raise SidecarError(
            f"IngestTradeFeedResult item_outcomes count mismatch: "
            f"expected {len(request.fills)}, got {len(result.item_outcomes)}"
        )

    outcome_counts: dict[str, int] = {}
    for req_fill, outcome in zip(request.fills, result.item_outcomes, strict=True):
        if outcome.kind not in VALID_TRADE_FILL_OUTCOME_KINDS:
            raise SidecarError(
                f"IngestTradeFeedResult contains unsupported TradeFillOutcomeKind={outcome.kind} "
                f"for feed_seq={outcome.feed_seq}"
            )
        if (
            outcome.source_id != req_fill.source_id
            or outcome.external_trade_id != req_fill.external_trade_id
            or outcome.source_revision != req_fill.source_revision
            or outcome.feed_seq != req_fill.feed_seq
        ):
            raise SidecarError(
                "IngestTradeFeedResult item_outcome identity mismatch: "
                f"expected ({req_fill.source_id!r}, {req_fill.external_trade_id!r}, "
                f"rev={req_fill.source_revision}, seq={req_fill.feed_seq}), "
                f"got ({outcome.source_id!r}, {outcome.external_trade_id!r}, "
                f"rev={outcome.source_revision}, seq={outcome.feed_seq})"
            )
        label = _OUTCOME_KIND_NAMES[outcome.kind]
        outcome_counts[label] = outcome_counts.get(label, 0) + 1
    return outcome_counts


def _resolve_input_format(path: Path, configured: str) -> Literal["csv", "jsonl"]:
    if configured in ("csv", "jsonl"):
        return configured  # type: ignore[return-value]
    suffix = path.suffix.lower()
    if suffix == ".csv":
        return "csv"
    if suffix in (".jsonl", ".ndjson", ".json"):
        return "jsonl"
    raise ValueError(
        f"cannot infer input format from suffix {suffix!r}; pass input_format='csv' or 'jsonl'"
    )


def _read_rows_from_offset(
    path: Path,
    *,
    input_format: Literal["csv", "jsonl"],
    start_offset: int,
) -> list[tuple[dict[str, Any], int]]:
    file_size = path.stat().st_size
    if start_offset > file_size:
        raise CheckpointCorruptionError(
            f"input file {path} size ({file_size}) is smaller than checkpoint confirmed_offset ({start_offset})"
        )

    rows: list[tuple[dict[str, Any], int]] = []
    with path.open("rb") as handle:
        if input_format == "csv":
            header_raw = handle.readline()
            if not header_raw:
                return []
            header_end = handle.tell()
            header_text = header_raw.decode("utf-8-sig")
            parsed_header = next(csv.reader(io.StringIO(header_text)), [])
            headers = [col.strip() for col in parsed_header]
            if not headers or any(not h for h in headers):
                raise ValueError(f"invalid CSV header in {path}")
            if start_offset > header_end:
                handle.seek(start_offset)
            while True:
                raw_line = handle.readline()
                if not raw_line:
                    break
                next_offset = handle.tell()
                decoded = raw_line.decode("utf-8")
                if not decoded.strip():
                    continue
                values = next(csv.reader(io.StringIO(decoded)), [])
                if len(values) != len(headers):
                    raise ValueError(
                        f"CSV column count mismatch in {path}: expected {len(headers)}, got {len(values)}"
                    )
                rows.append((dict(zip(headers, values, strict=True)), next_offset))
        else:
            handle.seek(start_offset)
            while True:
                raw_line = handle.readline()
                if not raw_line:
                    break
                next_offset = handle.tell()
                decoded = raw_line.decode("utf-8-sig" if start_offset == 0 and not rows else "utf-8").strip()
                if not decoded:
                    continue
                obj = json.loads(decoded)
                if not isinstance(obj, dict):
                    raise ValueError(f"JSONL line in {path} must decode to a JSON object")
                rows.append((obj, next_offset))
    return rows


def _initial_trade_feed_state() -> dict[str, Any]:
    return {
        "transport": {
            "contiguous_feed_seq": 0,
            "max_seen_feed_seq": 0,
            "missing_ranges": [],
            "feed_seq_hashes": {},
        },
        "business": {},
        "batch_counter": 0,
        "last_heartbeat_at": None,
        "last_receipt": None,
    }


def _make_trade_feed_idempotency_key(
    *,
    sidecar_id: str,
    ledger: LedgerIdentity,
    source_id: str,
    batch_counter: int,
    fills_meta: list[dict[str, Any]],
    heartbeat_rfc3339: str | None,
) -> str:
    h = hashlib.sha256()
    h.update(sidecar_id.encode("utf-8"))
    h.update(b"\x00")
    h.update(str(ledger.ledger_epoch).encode("ascii"))
    h.update(b"\x00")
    h.update(ledger.ledger_instance_id.encode("ascii"))
    h.update(b"\x00")
    h.update(source_id.encode("utf-8"))
    h.update(b"\x00")
    h.update(str(batch_counter).encode("ascii"))
    h.update(b"\x00")
    h.update((heartbeat_rfc3339 or "").encode("ascii"))
    for item in fills_meta:
        h.update(b"\x00")
        h.update(str(item["feed_seq"]).encode("ascii"))
        h.update(b":")
        h.update(str(item["payload_hash"]).encode("ascii"))
    return f"trade-feed:{sidecar_id}:e{ledger.ledger_epoch}:b{batch_counter}:{h.hexdigest()[:16]}"


class TradeFeedSidecar:
    """Crash-safe trade feed sidecar implementing ``F-07`` dual cursors and exact in-flight replay."""

    def __init__(
        self,
        config: TradeFeedConfig,
        client: Any,
        *,
        health_probe: HealthProbeFn | None = None,
        now_utc_fn: Callable[[], datetime] | None = None,
        on_retry: Callable[[int, BaseException, float], None] | None = None,
    ) -> None:
        self._config = config
        self._client = client
        self._health_probe = health_probe
        self._now_utc_fn = now_utc_fn or (lambda: datetime.now(timezone.utc))
        self._on_retry = on_retry
        self._units_resolver = AccountUnitsResolver(
            explicit_units=config.explicit_units,
            client=client,
            allow_snapshot_discovery=config.allow_snapshot_discovery,
        )

    @property
    def config(self) -> TradeFeedConfig:
        return self._config

    @property
    def units_resolver(self) -> AccountUnitsResolver:
        return self._units_resolver

    def _resolve_heartbeat(self) -> tuple[Any | None, str | None]:
        if self._config.heartbeat_at is not None:
            hb = parse_market_time(
                self._config.heartbeat_at,
                market_timezone=self._config.market_timezone,
            )
            return (hb, market_time_to_rfc3339(hb))
        if self._config.heartbeat_interval_seconds is not None:
            hb = market_time_from_utc_datetime(
                self._now_utc_fn(),
                market_timezone=self._config.market_timezone,
            )
            return (hb, market_time_to_rfc3339(hb))
        return (None, None)

    def run_once(self) -> TradeFeedBatchReport:
        """Probe ``/healthz``, replay any frozen in-flight request, and ingest new trade fills."""
        input_path = Path(self._config.input_path)
        if not input_path.is_file():
            raise SidecarError(f"trade feed input file does not exist: {input_path}")
        input_format = _resolve_input_format(input_path, self._config.input_format)

        with JsonCheckpointStore(self._config.checkpoint_path) as store:
            input_fingerprint = compute_file_fingerprint(input_path)
            health = invoke_health_probe(
                health_probe=self._health_probe,
                endpoint=self._config.endpoint,
                insecure=self._config.insecure,
                ca_cert_path=self._config.ca_cert_path,
                root_certificates=self._config.root_certificates,
                timeout_seconds=self._config.health_timeout_seconds,
            )
            reconcile = reconcile_ledger_checkpoint(
                store=store,
                server_health=health,
                kind="trade_feed",
                sidecar_id=self._config.sidecar_id,
                trading_date=self._config.trading_date,
                input_fingerprint=input_fingerprint,
                initial_state_factory=_initial_trade_feed_state,
                expected_ledger=self._config.expected_ledger,
                reset_checkpoint_for_restore=self._config.reset_checkpoint_for_restore,
            )
            ledger = reconcile.ledger
            checkpoint = reconcile.checkpoint
            state = checkpoint["state"]
            transport = state.setdefault(
                "transport",
                {
                    "contiguous_feed_seq": 0,
                    "max_seen_feed_seq": 0,
                    "missing_ranges": [],
                    "feed_seq_hashes": {},
                },
            )
            state.setdefault("business", {})
            state.setdefault("batch_counter", 0)

            batches_sent = 0
            fills_read = 0
            fills_sent = 0
            fills_skipped_dedup = 0
            replayed_in_flight = False
            committed_seqs: list[int] = []
            outcome_counts: dict[str, int] = {}

            # Phase 1: Replay exact frozen in_flight request bytes (unaffected by appended file rows)
            if checkpoint.get("in_flight") is not None:
                in_flight = checkpoint["in_flight"]
                req = service_pb2.IngestTradeFeedRequest()
                req.ParseFromString(base64.b64decode(in_flight["request_pb_b64"]))
                res = self._config.backoff.execute(
                    lambda: self._client.ingest_trade_feed(
                        req, expected_ledger=ledger.as_tuple()
                    ),
                    on_retry=self._on_retry,
                )
                batch_outcomes = validate_trade_feed_result(req, res)
                self._commit_in_flight_success(store, checkpoint, in_flight, res)
                replayed_in_flight = True
                batches_sent += 1
                fills_sent += len(req.fills)
                committed_seqs.append(res.committed_seq)
                for label, count in batch_outcomes.items():
                    outcome_counts[label] = outcome_counts.get(label, 0) + count

            # Phase 2: Read newly appended rows starting from confirmed_offset
            current_fingerprint = compute_file_fingerprint(input_path)
            raw_rows = _read_rows_from_offset(
                input_path,
                input_format=input_format,
                start_offset=checkpoint["confirmed_offset"],
            )

            feed_seq_hashes: dict[str, str] = transport.setdefault("feed_seq_hashes", {})
            pending_fills: list[spi_pb2.TradeFillRecord] = []
            pending_meta: list[dict[str, Any]] = []
            pending_in_batch_seq_hashes: dict[int, str] = {}
            pending_offset = checkpoint["confirmed_offset"]

            def flush_pending(*, allow_empty_heartbeat: bool = False) -> None:
                nonlocal batches_sent, fills_sent
                nonlocal pending_fills, pending_meta, pending_in_batch_seq_hashes
                if not pending_fills and not allow_empty_heartbeat:
                    return
                hb_pb, hb_rfc3339 = self._resolve_heartbeat()
                if not pending_fills and hb_pb is None:
                    return
                next_counter = int(state.get("batch_counter", 0)) + 1
                idempotency_key = _make_trade_feed_idempotency_key(
                    sidecar_id=self._config.sidecar_id,
                    ledger=ledger,
                    source_id=self._config.source_id,
                    batch_counter=next_counter,
                    fills_meta=pending_meta,
                    heartbeat_rfc3339=hb_rfc3339,
                )
                req_kwargs: dict[str, Any] = {
                    "idempotency_key": idempotency_key,
                    "source_id": self._config.source_id,
                    "fills": pending_fills,
                }
                if hb_pb is not None:
                    req_kwargs["heartbeat_at"] = hb_pb
                req = service_pb2.IngestTradeFeedRequest(**req_kwargs)
                in_flight_entry = {
                    "idempotency_key": idempotency_key,
                    "source_id": self._config.source_id,
                    "batch_counter": next_counter,
                    "heartbeat_at": hb_rfc3339,
                    "request_pb_b64": base64.b64encode(req.SerializeToString()).decode(
                        "ascii"
                    ),
                    "pending_offset": pending_offset,
                    "pending_input_fingerprint": current_fingerprint,
                    "fills_meta": list(pending_meta),
                }
                # Atomically freeze full request, heartbeat_at, idempotency_key, and pending_offset BEFORE RPC
                checkpoint["in_flight"] = in_flight_entry
                store.save(checkpoint)

                res = self._config.backoff.execute(
                    lambda: self._client.ingest_trade_feed(
                        req, expected_ledger=ledger.as_tuple()
                    ),
                    on_retry=self._on_retry,
                )
                batch_outcomes = validate_trade_feed_result(req, res)
                self._commit_in_flight_success(store, checkpoint, in_flight_entry, res)
                batches_sent += 1
                fills_sent += len(pending_fills)
                committed_seqs.append(res.committed_seq)
                for label, count in batch_outcomes.items():
                    outcome_counts[label] = outcome_counts.get(label, 0) + count

                pending_fills = []
                pending_meta = []
                pending_in_batch_seq_hashes = {}

            for row_dict, row_end_offset in raw_rows:
                fills_read += 1
                fill, payload_hash_hex, business_hash_hex = parse_trade_fill_row(
                    row_dict,
                    expected_source_id=self._config.source_id,
                    units_resolver=self._units_resolver,
                    default_market_timezone=self._config.market_timezone,
                )
                seq_key = str(fill.feed_seq)
                existing_seq_hash = feed_seq_hashes.get(seq_key)

                # Transport dedup ONLY on identical (feed_seq, payload_hash).
                # An alias feed_seq carrying an already-seen business revision MUST still be sent
                # so the transport watermark advances while the business effect applies once.
                if existing_seq_hash == payload_hash_hex:
                    fills_skipped_dedup += 1
                    pending_offset = row_end_offset
                    continue

                if pending_in_batch_seq_hashes.get(fill.feed_seq) == payload_hash_hex:
                    fills_skipped_dedup += 1
                    pending_offset = row_end_offset
                    continue

                if len(pending_fills) >= self._config.batch_size:
                    flush_pending()

                pending_fills.append(fill)
                pending_meta.append(
                    {
                        "source_id": fill.source_id,
                        "external_trade_id": fill.external_trade_id,
                        "source_revision": fill.source_revision,
                        "feed_seq": fill.feed_seq,
                        "payload_hash": payload_hash_hex,
                        "business_hash": business_hash_hex,
                    }
                )
                pending_in_batch_seq_hashes.setdefault(fill.feed_seq, payload_hash_hex)
                pending_offset = row_end_offset

            if pending_fills:
                flush_pending()
            elif pending_offset > checkpoint["confirmed_offset"]:
                checkpoint["confirmed_offset"] = pending_offset
                checkpoint["input_fingerprint"] = current_fingerprint
                store.save(checkpoint)
            elif self._config.send_idle_heartbeat and batches_sent == 0:
                flush_pending(allow_empty_heartbeat=True)

            final_transport = checkpoint["state"]["transport"]
            contiguous = int(final_transport["contiguous_feed_seq"])
            max_seen = int(final_transport["max_seen_feed_seq"])
            missing_ranges = tuple(
                (int(pair[0]), int(pair[1]))
                for pair in final_transport.get("missing_ranges", [])
            )

            return TradeFeedBatchReport(
                ledger_epoch=ledger.ledger_epoch,
                ledger_instance_id=ledger.ledger_instance_id,
                source_id=self._config.source_id,
                batches_sent=batches_sent,
                fills_read=fills_read,
                fills_sent=fills_sent,
                fills_skipped_dedup=fills_skipped_dedup,
                replayed_in_flight=replayed_in_flight,
                quarantined_previous_in_flight=reconcile.quarantined_previous_in_flight,
                contiguous_feed_seq=contiguous,
                max_seen_feed_seq=max_seen,
                missing_ranges=missing_ranges,
                backfill_from_feed_seq=contiguous,
                outcome_counts=outcome_counts,
                committed_seqs=tuple(committed_seqs),
            )

    @staticmethod
    def _commit_in_flight_success(
        store: JsonCheckpointStore,
        checkpoint: dict[str, Any],
        in_flight: Mapping[str, Any],
        result: service_pb2.IngestTradeFeedResult,
    ) -> None:
        state = checkpoint["state"]
        transport = state.setdefault(
            "transport",
            {
                "contiguous_feed_seq": 0,
                "max_seen_feed_seq": 0,
                "missing_ranges": [],
                "feed_seq_hashes": {},
            },
        )
        feed_seq_hashes: dict[str, str] = transport.setdefault("feed_seq_hashes", {})
        business: dict[str, dict[str, Any]] = state.setdefault("business", {})

        for meta in in_flight["fills_meta"]:
            src_id = str(meta["source_id"])
            ext_id = str(meta["external_trade_id"])
            rev = int(meta["source_revision"])
            seq = int(meta["feed_seq"])
            p_hash = str(meta["payload_hash"])
            b_hash = str(meta["business_hash"])

            feed_seq_hashes[str(seq)] = p_hash
            b_key = _business_key(src_id, ext_id, rev)
            existing_biz = business.get(b_key)
            if existing_biz is None:
                business[b_key] = {
                    "source_id": src_id,
                    "external_trade_id": ext_id,
                    "source_revision": rev,
                    "business_hash": b_hash,
                    "canonical_feed_seq": seq,
                    "alias_seqs": [],
                }
            else:
                canonical_seq = int(existing_biz["canonical_feed_seq"])
                if seq != canonical_seq:
                    aliases: list[int] = [int(x) for x in existing_biz.get("alias_seqs", [])]
                    if seq not in aliases:
                        aliases.append(seq)
                        aliases.sort()
                        existing_biz["alias_seqs"] = aliases

        # Authoritative server transport cursors (contiguous_feed_seq is the backfill lower bound).
        fs = result.feed_status
        transport["contiguous_feed_seq"] = int(fs.contiguous_feed_seq)
        transport["max_seen_feed_seq"] = int(fs.max_seen_feed_seq)
        transport["missing_ranges"] = [
            [int(r.first), int(r.last)] for r in fs.missing_feed_ranges
        ]

        state["batch_counter"] = max(
            int(state.get("batch_counter", 0)),
            int(in_flight.get("batch_counter", 0)),
        )
        if in_flight.get("heartbeat_at") is not None:
            state["last_heartbeat_at"] = in_flight["heartbeat_at"]
        state["last_receipt"] = {
            "idempotency_key": str(in_flight["idempotency_key"]),
            "committed_seq": int(result.committed_seq),
            "replayed": bool(result.replayed),
            "item_count": len(result.item_outcomes),
        }

        checkpoint["confirmed_offset"] = int(in_flight["pending_offset"])
        checkpoint["input_fingerprint"] = str(in_flight["pending_input_fingerprint"])
        checkpoint["in_flight"] = None
        # Single atomic os.replace committing receipt + dual cursors + offset + clearing in_flight
        store.save(checkpoint)
