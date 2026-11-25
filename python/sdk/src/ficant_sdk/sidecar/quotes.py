"""Reference market quote stream sidecar (F-13) with canonical hash dedup and atomic checkpoints."""

from __future__ import annotations

import base64
from collections.abc import Callable, Mapping
import csv
from dataclasses import dataclass, field
import hashlib
import io
import json
from pathlib import Path
from typing import Any, Literal

from ficant.blotter.v1 import service_pb2, spi_pb2
from ficant.market.v1 import fact_pb2
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
    market_quote_canonical_payload_hash,
    market_time_instant_key,
    market_time_to_rfc3339,
    parse_market_time,
    validate_iso_date,
    validate_non_empty_text,
)

MAX_QUOTE_BATCH_SIZE = 256
_MAX_U64 = (1 << 64) - 1

_PRICE_SOURCE_TYPE_MAP: dict[str, int] = {
    "REAL_TRADE": fact_pb2.PRICE_SOURCE_TYPE_REAL_TRADE,
    "PRICE_SOURCE_TYPE_REAL_TRADE": fact_pb2.PRICE_SOURCE_TYPE_REAL_TRADE,
    "ACTIVE_QUOTE": fact_pb2.PRICE_SOURCE_TYPE_ACTIVE_QUOTE,
    "PRICE_SOURCE_TYPE_ACTIVE_QUOTE": fact_pb2.PRICE_SOURCE_TYPE_ACTIVE_QUOTE,
    "MODEL_VALUATION": fact_pb2.PRICE_SOURCE_TYPE_MODEL_VALUATION,
    "PRICE_SOURCE_TYPE_MODEL_VALUATION": fact_pb2.PRICE_SOURCE_TYPE_MODEL_VALUATION,
    "CURVE_INTERPOLATION": fact_pb2.PRICE_SOURCE_TYPE_CURVE_INTERPOLATION,
    "PRICE_SOURCE_TYPE_CURVE_INTERPOLATION": fact_pb2.PRICE_SOURCE_TYPE_CURVE_INTERPOLATION,
}

_ALLOWED_QUOTE_ROW_KEYS = frozenset(
    {
        "account_id",
        "source_id",
        "instrument_code",
        "quote_seq",
        "source_type",
        "bid_clean_price",
        "ask_clean_price",
        "mid_clean_price",
        "mid_yield_to_maturity",
        "observed_at",
        "received_at",
        "market_timezone",
        "observed_local_trading_date",
        "received_local_trading_date",
    }
)


@dataclass(frozen=True)
class QuoteStreamConfig:
    """Configuration for ``QuoteStreamSidecar``."""

    sidecar_id: str
    trading_date: str
    checkpoint_path: Path
    input_path: Path
    batch_size: int = 64
    input_format: Literal["auto", "csv", "jsonl"] = "auto"
    market_timezone: str = MARKET_TIMEZONE
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
        validate_iso_date(self.trading_date, "trading_date")
        if (
            isinstance(self.batch_size, bool)
            or not isinstance(self.batch_size, int)
            or self.batch_size < 1
            or self.batch_size > MAX_QUOTE_BATCH_SIZE
        ):
            raise ValueError(
                f"batch_size must be an integer in 1..={MAX_QUOTE_BATCH_SIZE}"
            )
        if self.input_format not in ("auto", "csv", "jsonl"):
            raise ValueError("input_format must be 'auto', 'csv', or 'jsonl'")
        validate_non_empty_text(self.market_timezone, "market_timezone")
        if self.expected_ledger is not None:
            LedgerIdentity(
                ledger_epoch=self.expected_ledger[0],
                ledger_instance_id=self.expected_ledger[1],
            )
        if self.health_timeout_seconds <= 0:
            raise ValueError("health_timeout_seconds must be positive")


@dataclass(frozen=True)
class QuoteBatchReport:
    """Summary report produced by ``QuoteStreamSidecar.run_once``."""

    ledger_epoch: int
    ledger_instance_id: str
    batches_sent: int
    ticks_read: int
    ticks_sent: int
    ticks_skipped_dedup: int
    applied_tick_count: int
    skipped_stale_tick_count: int
    replayed_in_flight: bool
    quarantined_previous_in_flight: bool
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


def _parse_price_source_type(raw: Any) -> int:
    if not isinstance(raw, str):
        raise ValueError(
            f"source_type must be one of REAL_TRADE, ACTIVE_QUOTE, MODEL_VALUATION, CURVE_INTERPOLATION; got {raw!r}"
        )
    key = raw.strip().upper()
    resolved = _PRICE_SOURCE_TYPE_MAP.get(key)
    if resolved is None:
        raise ValueError(f"unsupported PriceSourceType: {raw!r}")
    return resolved


def _optional_cell(row: Mapping[str, Any], key: str) -> Any | None:
    if key not in row:
        return None
    val = row[key]
    if val is None:
        return None
    if isinstance(val, str) and val.strip() == "":
        return None
    return val.strip() if isinstance(val, str) else val


def parse_quote_row(
    row: Mapping[str, Any],
    *,
    units_resolver: AccountUnitsResolver,
    default_market_timezone: str = MARKET_TIMEZONE,
) -> tuple[str, spi_pb2.MarketQuoteTick, str, str]:
    """Parse and validate a single quote row into ``(account_id, tick, payload_hash_hex, received_at_rfc3339)``."""
    unknown = set(row.keys()) - _ALLOWED_QUOTE_ROW_KEYS
    if unknown:
        raise ValueError(f"unknown quote row field(s): {sorted(unknown)}")

    account_id = validate_non_empty_text(
        row.get("account_id").strip() if isinstance(row.get("account_id"), str) else row.get("account_id"),
        "account_id",
    )
    source_id = validate_non_empty_text(
        row.get("source_id").strip() if isinstance(row.get("source_id"), str) else row.get("source_id"),
        "source_id",
    )
    instrument_code = validate_non_empty_text(
        row.get("instrument_code").strip()
        if isinstance(row.get("instrument_code"), str)
        else row.get("instrument_code"),
        "instrument_code",
    )
    quote_seq = _parse_positive_u64(row.get("quote_seq"), "quote_seq")
    source_type = _parse_price_source_type(row.get("source_type"))

    tz_raw = _optional_cell(row, "market_timezone")
    market_tz = validate_non_empty_text(tz_raw, "market_timezone") if tz_raw is not None else default_market_timezone

    obs_raw = _optional_cell(row, "observed_at")
    rec_raw = _optional_cell(row, "received_at")
    if obs_raw is None or rec_raw is None:
        raise ValueError("quote row requires both observed_at and received_at")

    obs_date_raw = _optional_cell(row, "observed_local_trading_date")
    rec_date_raw = _optional_cell(row, "received_local_trading_date")

    observed_at = parse_market_time(
        obs_raw,
        market_timezone=market_tz,
        local_trading_date=obs_date_raw,
    )
    received_at = parse_market_time(
        rec_raw,
        market_timezone=market_tz,
        local_trading_date=rec_date_raw,
    )
    if market_time_instant_key(observed_at) > market_time_instant_key(received_at):
        raise ValueError("quote observed_at must be <= received_at")

    units = units_resolver.resolve(account_id)

    bid_raw = _optional_cell(row, "bid_clean_price")
    ask_raw = _optional_cell(row, "ask_clean_price")
    mid_raw = _optional_cell(row, "mid_clean_price")
    ytm_raw = _optional_cell(row, "mid_yield_to_maturity")

    bid_pb = decimal_to_pb(bid_raw, units.price_per_100) if bid_raw is not None else None
    ask_pb = decimal_to_pb(ask_raw, units.price_per_100) if ask_raw is not None else None
    mid_pb = decimal_to_pb(mid_raw, units.price_per_100) if mid_raw is not None else None
    ytm_pb = decimal_to_pb(ytm_raw, units.rate) if ytm_raw is not None else None

    if bid_pb is not None and fixed_decimal_scaled_i128(bid_pb) <= 0:
        raise ValueError("bid_clean_price must be strictly positive")
    if ask_pb is not None and fixed_decimal_scaled_i128(ask_pb) <= 0:
        raise ValueError("ask_clean_price must be strictly positive")
    if bid_pb is not None and ask_pb is not None:
        if fixed_decimal_scaled_i128(bid_pb) > fixed_decimal_scaled_i128(ask_pb):
            raise ValueError("bid_clean_price must be <= ask_clean_price")
    if mid_pb is not None and fixed_decimal_scaled_i128(mid_pb) <= 0:
        raise ValueError("mid_clean_price must be strictly positive")
    if ytm_pb is not None and fixed_decimal_scaled_i128(ytm_pb) < 0:
        raise ValueError("mid_yield_to_maturity must be non-negative")

    has_usable_price = (
        mid_pb is not None
        or (bid_pb is not None and ask_pb is not None)
        or ytm_pb is not None
    )
    if not has_usable_price:
        raise ValueError(
            "quote must include usable price: mid_clean_price, both bid_clean_price and ask_clean_price, or mid_yield_to_maturity"
        )

    tick_kwargs: dict[str, Any] = {
        "source_id": source_id,
        "quote_seq": quote_seq,
        "instrument_code": instrument_code,
        "source_type": source_type,
        "observed_at": observed_at,
        "received_at": received_at,
    }
    if bid_pb is not None:
        tick_kwargs["bid_clean_price"] = bid_pb
    if ask_pb is not None:
        tick_kwargs["ask_clean_price"] = ask_pb
    if mid_pb is not None:
        tick_kwargs["mid_clean_price"] = mid_pb
    if ytm_pb is not None:
        tick_kwargs["mid_yield_to_maturity"] = ytm_pb

    tick = spi_pb2.MarketQuoteTick(**tick_kwargs)
    payload_hash_hex = market_quote_canonical_payload_hash(tick).hex()
    return (account_id, tick, payload_hash_hex, market_time_to_rfc3339(received_at))


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
    """Read newly available records starting at byte ``start_offset``."""
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


def _initial_quotes_state() -> dict[str, Any]:
    return {
        "acked_quotes": {},
        "heads": {},
        "batch_counter": 0,
    }


def _make_quote_idempotency_key(
    *,
    sidecar_id: str,
    ledger: LedgerIdentity,
    account_id: str,
    source_id: str,
    batch_counter: int,
    ticks_meta: list[dict[str, Any]],
) -> str:
    h = hashlib.sha256()
    h.update(sidecar_id.encode("utf-8"))
    h.update(b"\x00")
    h.update(str(ledger.ledger_epoch).encode("ascii"))
    h.update(b"\x00")
    h.update(ledger.ledger_instance_id.encode("ascii"))
    h.update(b"\x00")
    h.update(account_id.encode("utf-8"))
    h.update(b"\x00")
    h.update(source_id.encode("utf-8"))
    h.update(b"\x00")
    h.update(str(batch_counter).encode("ascii"))
    for item in ticks_meta:
        h.update(b"\x00")
        h.update(str(item["instrument_code"]).encode("utf-8"))
        h.update(b":")
        h.update(str(item["quote_seq"]).encode("ascii"))
        h.update(b":")
        h.update(str(item["payload_hash"]).encode("ascii"))
    return f"quotes:{sidecar_id}:e{ledger.ledger_epoch}:b{batch_counter}:{h.hexdigest()[:16]}"


class QuoteStreamSidecar:
    """Crash-safe market quote sidecar implementing ``F-13`` confirmation & dedup rules."""

    def __init__(
        self,
        config: QuoteStreamConfig,
        client: Any,
        *,
        health_probe: HealthProbeFn | None = None,
        on_retry: Callable[[int, BaseException, float], None] | None = None,
    ) -> None:
        self._config = config
        self._client = client
        self._health_probe = health_probe
        self._on_retry = on_retry
        self._units_resolver = AccountUnitsResolver(
            explicit_units=config.explicit_units,
            client=client,
            allow_snapshot_discovery=config.allow_snapshot_discovery,
        )

    @property
    def config(self) -> QuoteStreamConfig:
        return self._config

    @property
    def units_resolver(self) -> AccountUnitsResolver:
        return self._units_resolver

    def run_once(self) -> QuoteBatchReport:
        """Probe ``/healthz``, recover any frozen in-flight batch, and ingest new quotes."""
        input_path = Path(self._config.input_path)
        if not input_path.is_file():
            raise SidecarError(f"quote input file does not exist: {input_path}")
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
                kind="quotes",
                sidecar_id=self._config.sidecar_id,
                trading_date=self._config.trading_date,
                input_fingerprint=input_fingerprint,
                initial_state_factory=_initial_quotes_state,
                expected_ledger=self._config.expected_ledger,
                reset_checkpoint_for_restore=self._config.reset_checkpoint_for_restore,
            )
            ledger = reconcile.ledger
            checkpoint = reconcile.checkpoint
            state = checkpoint["state"]
            state.setdefault("acked_quotes", {})
            state.setdefault("heads", {})
            state.setdefault("batch_counter", 0)

            batches_sent = 0
            ticks_read = 0
            ticks_sent = 0
            ticks_skipped_dedup = 0
            applied_tick_count = 0
            skipped_stale_tick_count = 0
            replayed_in_flight = False
            committed_seqs: list[int] = []

            # Phase 1: Replay frozen in_flight batch verbatim (unaffected by appended file rows)
            if checkpoint.get("in_flight") is not None:
                in_flight = checkpoint["in_flight"]
                req = service_pb2.IngestMarketQuotesRequest()
                req.ParseFromString(base64.b64decode(in_flight["request_pb_b64"]))
                res = self._config.backoff.execute(
                    lambda: self._client.ingest_market_quotes(
                        req, expected_ledger=ledger.as_tuple()
                    ),
                    on_retry=self._on_retry,
                )
                self._commit_in_flight_success(store, checkpoint, in_flight)
                replayed_in_flight = True
                batches_sent += 1
                ticks_sent += len(req.ticks)
                applied_tick_count += res.applied_tick_count
                skipped_stale_tick_count += res.skipped_stale_tick_count
                committed_seqs.append(res.committed_seq)

            # Phase 2: Read new rows strictly after confirmed_offset
            current_fingerprint = compute_file_fingerprint(input_path)
            raw_rows = _read_rows_from_offset(
                input_path,
                input_format=input_format,
                start_offset=checkpoint["confirmed_offset"],
            )

            if not raw_rows:
                return QuoteBatchReport(
                    ledger_epoch=ledger.ledger_epoch,
                    ledger_instance_id=ledger.ledger_instance_id,
                    batches_sent=batches_sent,
                    ticks_read=ticks_read,
                    ticks_sent=ticks_sent,
                    ticks_skipped_dedup=ticks_skipped_dedup,
                    applied_tick_count=applied_tick_count,
                    skipped_stale_tick_count=skipped_stale_tick_count,
                    replayed_in_flight=replayed_in_flight,
                    quarantined_previous_in_flight=reconcile.quarantined_previous_in_flight,
                    committed_seqs=tuple(committed_seqs),
                )

            # Group consecutive ticks by (account_id, source_id) up to batch_size
            pending_key: tuple[str, str] | None = None
            pending_ticks: list[spi_pb2.MarketQuoteTick] = []
            pending_meta: list[dict[str, Any]] = []
            pending_in_batch_hashes: dict[tuple[str, str, int], str] = {}
            pending_offset = checkpoint["confirmed_offset"]
            acked_quotes = state["acked_quotes"]

            def flush_pending() -> None:
                nonlocal batches_sent, ticks_sent, applied_tick_count, skipped_stale_tick_count
                nonlocal pending_key, pending_ticks, pending_meta, pending_in_batch_hashes
                if not pending_ticks or pending_key is None:
                    return
                acct_id, src_id = pending_key
                next_counter = int(state.get("batch_counter", 0)) + 1
                idempotency_key = _make_quote_idempotency_key(
                    sidecar_id=self._config.sidecar_id,
                    ledger=ledger,
                    account_id=acct_id,
                    source_id=src_id,
                    batch_counter=next_counter,
                    ticks_meta=pending_meta,
                )
                req = service_pb2.IngestMarketQuotesRequest(
                    idempotency_key=idempotency_key,
                    account_id=acct_id,
                    source_id=src_id,
                    ticks=pending_ticks,
                )
                in_flight_entry = {
                    "idempotency_key": idempotency_key,
                    "account_id": acct_id,
                    "source_id": src_id,
                    "batch_counter": next_counter,
                    "request_pb_b64": base64.b64encode(req.SerializeToString()).decode(
                        "ascii"
                    ),
                    "pending_offset": pending_offset,
                    "pending_input_fingerprint": current_fingerprint,
                    "ticks_meta": list(pending_meta),
                }
                # Freeze in_flight atomically BEFORE RPC
                checkpoint["in_flight"] = in_flight_entry
                store.save(checkpoint)

                res = self._config.backoff.execute(
                    lambda: self._client.ingest_market_quotes(
                        req, expected_ledger=ledger.as_tuple()
                    ),
                    on_retry=self._on_retry,
                )
                self._commit_in_flight_success(store, checkpoint, in_flight_entry)
                batches_sent += 1
                ticks_sent += len(pending_ticks)
                applied_tick_count += res.applied_tick_count
                skipped_stale_tick_count += res.skipped_stale_tick_count
                committed_seqs.append(res.committed_seq)

                pending_key = None
                pending_ticks = []
                pending_meta = []
                pending_in_batch_hashes = {}

            for row_dict, row_end_offset in raw_rows:
                ticks_read += 1
                account_id, tick, payload_hash_hex, received_at_rfc3339 = parse_quote_row(
                    row_dict,
                    units_resolver=self._units_resolver,
                    default_market_timezone=self._config.market_timezone,
                )
                triple = (tick.source_id, tick.instrument_code, tick.quote_seq)

                existing_entry = (
                    acked_quotes.get(tick.source_id, {})
                    .get(tick.instrument_code, {})
                    .get(str(tick.quote_seq))
                )
                if (
                    existing_entry is not None
                    and existing_entry.get("payload_hash") == payload_hash_hex
                ):
                    # Exact same business content (including re-delivery with different received_at):
                    # skip locally while preserving original first_received_at.
                    ticks_skipped_dedup += 1
                    pending_offset = row_end_offset
                    continue

                if pending_in_batch_hashes.get(triple) == payload_hash_hex:
                    # Identical duplicate within the same unread batch: preserve first tick's received_at.
                    ticks_skipped_dedup += 1
                    pending_offset = row_end_offset
                    continue

                batch_key = (account_id, tick.source_id)
                if pending_ticks and (
                    pending_key != batch_key
                    or len(pending_ticks) >= self._config.batch_size
                ):
                    flush_pending()

                pending_key = batch_key
                pending_ticks.append(tick)
                pending_meta.append(
                    {
                        "source_id": tick.source_id,
                        "instrument_code": tick.instrument_code,
                        "quote_seq": tick.quote_seq,
                        "payload_hash": payload_hash_hex,
                        "received_at": received_at_rfc3339,
                    }
                )
                pending_in_batch_hashes.setdefault(triple, payload_hash_hex)
                pending_offset = row_end_offset

            if pending_ticks:
                flush_pending()
            elif pending_offset > checkpoint["confirmed_offset"]:
                # All unread rows were exact dedup skips; advance offset atomically.
                checkpoint["confirmed_offset"] = pending_offset
                checkpoint["input_fingerprint"] = current_fingerprint
                store.save(checkpoint)

            return QuoteBatchReport(
                ledger_epoch=ledger.ledger_epoch,
                ledger_instance_id=ledger.ledger_instance_id,
                batches_sent=batches_sent,
                ticks_read=ticks_read,
                ticks_sent=ticks_sent,
                ticks_skipped_dedup=ticks_skipped_dedup,
                applied_tick_count=applied_tick_count,
                skipped_stale_tick_count=skipped_stale_tick_count,
                replayed_in_flight=replayed_in_flight,
                quarantined_previous_in_flight=reconcile.quarantined_previous_in_flight,
                committed_seqs=tuple(committed_seqs),
            )

    @staticmethod
    def _commit_in_flight_success(
        store: JsonCheckpointStore,
        checkpoint: dict[str, Any],
        in_flight: Mapping[str, Any],
    ) -> None:
        state = checkpoint["state"]
        acked_quotes: dict[str, dict[str, dict[str, dict[str, str]]]] = state.setdefault(
            "acked_quotes", {}
        )
        heads: dict[str, dict[str, int]] = state.setdefault("heads", {})

        for meta in in_flight["ticks_meta"]:
            src_id = meta["source_id"]
            inst_code = meta["instrument_code"]
            seq = int(meta["quote_seq"])
            seq_key = str(seq)
            payload_hash = meta["payload_hash"]
            received_at = meta["received_at"]

            inst_map = acked_quotes.setdefault(src_id, {}).setdefault(inst_code, {})
            if seq_key not in inst_map:
                inst_map[seq_key] = {
                    "payload_hash": payload_hash,
                    "first_received_at": received_at,
                }
            # Preserve immutable first_received_at if already present.

            src_heads = heads.setdefault(src_id, {})
            prev_head = int(src_heads.get(inst_code, 0))
            if seq > prev_head:
                src_heads[inst_code] = seq

        state["batch_counter"] = max(
            int(state.get("batch_counter", 0)),
            int(in_flight.get("batch_counter", 0)),
        )
        checkpoint["confirmed_offset"] = int(in_flight["pending_offset"])
        checkpoint["input_fingerprint"] = str(in_flight["pending_input_fingerprint"])
        checkpoint["in_flight"] = None
        store.save(checkpoint)
