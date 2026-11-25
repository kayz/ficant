"""Structurally isolated NLP draft gateway sidecar (F-20) with durable pre-submit intent."""

from __future__ import annotations

import base64
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass, field
from decimal import Decimal
import hashlib
import http.client
import json
from pathlib import Path
import re
import ssl
from typing import Any, Literal
from urllib.parse import urlsplit

from ficant.blotter.v1 import model_pb2, service_pb2, spi_pb2
from ficant.core.v1 import common_pb2
from ficant.rates.v1 import analytics_pb2 as rates_pb2
import grpc

from ficant_sdk.blotter import (
    AUTHORIZATION_HEADER,
    EXPECTED_LEDGER_EPOCH_HEADER,
    EXPECTED_LEDGER_INSTANCE_HEADER,
    FICANT_CREDENTIAL_HEADER,
    BlotterError,
    _expected_ledger_metadata,
    _validate_expected_ledger,
    _validate_raw_token,
)
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
    DECIMAL_SCALE,
    AccountUnitsResolver,
    decimal_to_pb,
    fixed_decimal_scaled_i128,
    is_canonical_ulid,
    validate_iso_date,
    validate_non_empty_text,
)

PARSE_NLP_INTENT_METHOD = (
    "/ficant.blotter.v1.IntranetNlpIntentParserService/ParseNlpIntent"
)
SUBMIT_DRAFT_EARMARK_METHOD = (
    "/ficant.blotter.v1.DeskBlotterService/SubmitDraftEarmark"
)
NLP_ALLOWED_RPC_METHODS: frozenset[str] = frozenset(
    {PARSE_NLP_INTENT_METHOD, SUBMIT_DRAFT_EARMARK_METHOD}
)

MAX_WEBHOOK_RESPONSE_BYTES = 262_144  # 256 KiB
_SCALED_ONE = 10**DECIMAL_SCALE
_MAX_U32 = (1 << 32) - 1

_TRADE_SIDE_MAP: dict[str, int] = {
    "BUY": model_pb2.TRADE_SIDE_BUY,
    "TRADE_SIDE_BUY": model_pb2.TRADE_SIDE_BUY,
    "SELL": model_pb2.TRADE_SIDE_SELL,
    "TRADE_SIDE_SELL": model_pb2.TRADE_SIDE_SELL,
}

_SETTLEMENT_BUCKET_MAP: dict[str, int] = {
    "T+0": model_pb2.SETTLEMENT_BUCKET_T_PLUS_0,
    "T_PLUS_0": model_pb2.SETTLEMENT_BUCKET_T_PLUS_0,
    "SETTLEMENT_BUCKET_T_PLUS_0": model_pb2.SETTLEMENT_BUCKET_T_PLUS_0,
    "T+1": model_pb2.SETTLEMENT_BUCKET_T_PLUS_1,
    "T_PLUS_1": model_pb2.SETTLEMENT_BUCKET_T_PLUS_1,
    "SETTLEMENT_BUCKET_T_PLUS_1": model_pb2.SETTLEMENT_BUCKET_T_PLUS_1,
}

_TARGET_STATE_MAP: dict[str, int] = {
    "HOLD": model_pb2.DRAFT_TARGET_STATE_HOLD,
    "DRAFT_TARGET_STATE_HOLD": model_pb2.DRAFT_TARGET_STATE_HOLD,
    "COMMITTED": model_pb2.DRAFT_TARGET_STATE_COMMITTED,
    "DRAFT_TARGET_STATE_COMMITTED": model_pb2.DRAFT_TARGET_STATE_COMMITTED,
}

_ALLOWED_NLP_INPUT_ROW_KEYS: frozenset[str] = frozenset(
    {
        "request_id",
        "account_id",
        "raw_source_text",
        "clean_price_per_100",
        "yield_to_maturity",
        "settlement_amount",
        "settlement_bucket",
        "suggested_target_state",
    }
)

_ALLOWED_WEBHOOK_TOP_LEVEL_KEYS: frozenset[str] = frozenset(
    {
        "request_id",
        "parser_source_id",
        "model_version",
        "account_id",
        "raw_source_text",
        "confidence_score",
        "instrument_code",
        "side",
        "settlement_bucket",
        "notional",
        "quote_terms",
        "clean_price_per_100",
        "yield_to_maturity",
        "settlement_amount",
        "suggested_target_state",
        "anchors",
    }
)

_ALLOWED_WEBHOOK_QUOTE_TERMS_KEYS: frozenset[str] = frozenset(
    {
        "clean_price_per_100",
        "yield_to_maturity",
        "settlement_amount",
    }
)

_ALLOWED_WEBHOOK_ANCHOR_KEYS: frozenset[str] = frozenset(
    {
        "field_name",
        "byte_start",
        "byte_end",
        "span_text",
    }
)

_TOKEN_DELIM_CHARS = frozenset({" ", "\t", "\r", "\n", ":", "：", ",", "，", ";", "；"})
_EMBEDDED_INSTRUMENT_RE = re.compile(r"[A-Za-z][A-Za-z0-9_.-]{2,}")
_CN_DIGIT_VALUES: dict[str, int] = {
    "零": 0,
    "〇": 0,
    "一": 1,
    "二": 2,
    "两": 2,
    "三": 3,
    "四": 4,
    "五": 5,
    "六": 6,
    "七": 7,
    "八": 8,
    "九": 9,
}
_CN_SMALL_UNITS: dict[str, int] = {
    "十": 10,
    "百": 100,
    "千": 1_000,
}
_CN_LARGE_UNITS: dict[str, int] = {
    "万": 10_000,
    "亿": 100_000_000,
}
_CN_AMOUNT_RE = re.compile(
    r"(?P<expr>(?:\d+(?:\.\d+)?|[零〇一二两三四五六七八九十百千]+)(?:万|亿)?)"
)


def _units_equal(lhs: common_pb2.UnitRef, rhs: common_pb2.UnitRef) -> bool:
    return (
        lhs.HasField("unit_id")
        and rhs.HasField("unit_id")
        and lhs.unit_id.value == rhs.unit_id.value
        and lhs.version == rhs.version
    )


def _validate_webhook_url(url: str) -> tuple[str, str, int, str]:
    validate_non_empty_text(url, "webhook_url")
    parsed = urlsplit(url)
    scheme = parsed.scheme.lower()
    if scheme not in ("http", "https"):
        raise ValueError(
            f"webhook_url must use http or https scheme, got {parsed.scheme!r}"
        )
    if parsed.username is not None or parsed.password is not None:
        raise ValueError("webhook_url must not contain embedded userinfo credentials")
    if parsed.fragment:
        raise ValueError("webhook_url must not contain a URL fragment")
    host = parsed.hostname
    if not host:
        raise ValueError(f"webhook_url missing host: {url!r}")
    port = parsed.port or (443 if scheme == "https" else 80)
    path = parsed.path or "/"
    if parsed.query:
        path = f"{path}?{parsed.query}"
    return (scheme, host, port, path)


def _utf8_codepoint_boundaries(text: str) -> set[int]:
    boundaries: set[int] = {0}
    offset = 0
    for ch in text:
        offset += len(ch.encode("utf-8"))
        boundaries.add(offset)
    return boundaries


def validate_source_text_anchors(
    raw_source_text: str,
    anchors: Sequence[model_pb2.SourceTextAnchor],
    *,
    require_non_empty: bool = True,
) -> None:
    """Validate UTF-8 codepoint boundaries, exact span echo, and non-ambiguity of anchors."""
    if not isinstance(raw_source_text, str) or not raw_source_text.strip():
        raise ValueError("raw_source_text must be a non-empty string")
    if require_non_empty and len(anchors) == 0:
        raise ValueError("proposal must contain at least one source text anchor")

    raw_bytes = raw_source_text.encode("utf-8")
    total_len = len(raw_bytes)
    boundaries = _utf8_codepoint_boundaries(raw_source_text)

    seen_fields: set[str] = set()
    intervals: list[tuple[int, int, str]] = []

    for anchor in anchors:
        field_name = validate_non_empty_text(anchor.field_name, "anchor.field_name")
        if field_name in seen_fields:
            raise ValueError(f"ambiguous duplicate anchor field_name={field_name!r}")
        seen_fields.add(field_name)

        start = anchor.byte_start
        end = anchor.byte_end
        if (
            isinstance(start, bool)
            or not isinstance(start, int)
            or isinstance(end, bool)
            or not isinstance(end, int)
        ):
            raise ValueError(f"anchor {field_name!r} byte bounds must be integers")
        if start < 0 or end <= start or end > total_len or end > _MAX_U32:
            raise ValueError(
                f"anchor {field_name!r} has out-of-range UTF-8 span [{start}, {end}) "
                f"for {total_len}-byte text"
            )
        if start not in boundaries or end not in boundaries:
            raise ValueError(
                f"anchor {field_name!r} span [{start}, {end}) does not fall on valid UTF-8 codepoint boundaries"
            )

        matched = raw_bytes[start:end].decode("utf-8")
        if not matched:
            raise ValueError(f"anchor {field_name!r} matched an empty slice")
        if anchor.span_text != matched:
            raise ValueError(
                f"anchor {field_name!r} span_text {anchor.span_text!r} does not match "
                f"raw_source_text UTF-8 slice [{start}, {end}) = {matched!r}"
            )
        intervals.append((start, end, field_name))

    intervals.sort(key=lambda item: (item[0], item[1]))
    for idx in range(1, len(intervals)):
        prev_start, prev_end, prev_field = intervals[idx - 1]
        curr_start, curr_end, curr_field = intervals[idx]
        if curr_start < prev_end:
            raise ValueError(
                f"overlapping/ambiguous anchors: {prev_field!r} [{prev_start}, {prev_end}) "
                f"overlaps {curr_field!r} [{curr_start}, {curr_end})"
            )


def validate_nlp_proposal(
    proposal: spi_pb2.NlpEarmarkProposal,
    *,
    expected_account_id: str,
    expected_raw_source_text: str,
    units: rates_pb2.AnalysisUnits,
) -> None:
    """Validate every economic, echo, confidence, unit, and UTF-8 anchor invariant."""
    if not isinstance(proposal, spi_pb2.NlpEarmarkProposal):
        raise ValueError("proposal must be an NlpEarmarkProposal protobuf message")

    validate_non_empty_text(proposal.parser_source_id, "proposal.parser_source_id")
    validate_non_empty_text(proposal.model_version, "proposal.model_version")
    validate_non_empty_text(proposal.account_id, "proposal.account_id")
    validate_non_empty_text(proposal.instrument_code, "proposal.instrument_code")

    if proposal.account_id != expected_account_id:
        raise ValueError(
            f"proposal account_id mismatch: expected {expected_account_id!r}, got {proposal.account_id!r}"
        )
    if proposal.raw_source_text != expected_raw_source_text:
        raise ValueError(
            "proposal raw_source_text does not byte-match the caller input text"
        )

    if proposal.side not in (model_pb2.TRADE_SIDE_BUY, model_pb2.TRADE_SIDE_SELL):
        raise ValueError(f"unsupported proposal side: {proposal.side}")
    if proposal.settlement_bucket not in (
        model_pb2.SETTLEMENT_BUCKET_T_PLUS_0,
        model_pb2.SETTLEMENT_BUCKET_T_PLUS_1,
    ):
        raise ValueError(
            f"unsupported proposal settlement_bucket: {proposal.settlement_bucket}"
        )
    if proposal.suggested_target_state not in (
        model_pb2.DRAFT_TARGET_STATE_HOLD,
        model_pb2.DRAFT_TARGET_STATE_COMMITTED,
    ):
        raise ValueError(
            f"unsupported proposal suggested_target_state: {proposal.suggested_target_state}"
        )

    if not proposal.HasField("confidence_score"):
        raise ValueError("proposal missing confidence_score")
    if not proposal.confidence_score.HasField("unit") or not _units_equal(
        proposal.confidence_score.unit, units.dimensionless
    ):
        raise ValueError("proposal confidence_score must use dimensionless unit")
    conf_scaled = fixed_decimal_scaled_i128(proposal.confidence_score)
    if conf_scaled < 0 or conf_scaled > _SCALED_ONE:
        raise ValueError(
            f"proposal confidence_score must be in closed [0, 1], got {proposal.confidence_score.coefficient}e-{proposal.confidence_score.scale}"
        )

    if not proposal.HasField("notional"):
        raise ValueError("proposal missing notional")
    if not proposal.notional.HasField("unit") or not _units_equal(
        proposal.notional.unit, units.currency_amount
    ):
        raise ValueError("proposal notional must use currency_amount unit")
    if fixed_decimal_scaled_i128(proposal.notional) <= 0:
        raise ValueError("proposal notional must be strictly positive")

    if not proposal.HasField("quote_terms"):
        raise ValueError("proposal missing quote_terms")
    qt = proposal.quote_terms
    branch = qt.WhichOneof("price_or_yield")
    has_settle = qt.HasField("settlement_amount")
    if branch is None and not has_settle:
        raise ValueError(
            "proposal quote_terms must specify clean_price_per_100, yield_to_maturity, or settlement_amount"
        )
    if branch == "clean_price_per_100":
        if not qt.clean_price_per_100.HasField("unit") or not _units_equal(
            qt.clean_price_per_100.unit, units.price_per_100
        ):
            raise ValueError(
                "proposal clean_price_per_100 must use price_per_100 unit"
            )
        if fixed_decimal_scaled_i128(qt.clean_price_per_100) <= 0:
            raise ValueError("proposal clean_price_per_100 must be strictly positive")
    elif branch == "yield_to_maturity":
        if not qt.yield_to_maturity.HasField("unit") or not _units_equal(
            qt.yield_to_maturity.unit, units.rate
        ):
            raise ValueError("proposal yield_to_maturity must use rate unit")
        if fixed_decimal_scaled_i128(qt.yield_to_maturity) < 0:
            raise ValueError("proposal yield_to_maturity must be non-negative")
    if has_settle:
        if not qt.settlement_amount.HasField("unit") or not _units_equal(
            qt.settlement_amount.unit, units.currency_amount
        ):
            raise ValueError(
                "proposal settlement_amount must use currency_amount unit"
            )
        if fixed_decimal_scaled_i128(qt.settlement_amount) <= 0:
            raise ValueError("proposal settlement_amount must be strictly positive")

    validate_source_text_anchors(
        expected_raw_source_text, proposal.anchors, require_non_empty=True
    )


class NlpDraftOnlyClient:
    """Structurally isolated gRPC client binding ONLY ``ParseNlpIntent`` and ``SubmitDraftEarmark``.

    Deliberately does NOT instantiate ``DeskBlotterServiceStub`` (which registers
    all 18 blotter RPCs) and exposes zero trade-confirmation, earmark-creation,
    or ``GetAccountSnapshot`` capabilities.
    """

    def __init__(
        self,
        endpoint: str | None = None,
        *,
        channel: grpc.Channel | None = None,
        bearer_token: str | None = None,
        x_ficant_credential: str | None = None,
        header_mode: str | None = None,
        expected_ledger: tuple[int, str] | None = None,
        insecure: bool = False,
        timeout_seconds: float = 30.0,
        root_certificates: bytes | None = None,
    ) -> None:
        if (endpoint is None) == (channel is None):
            raise ValueError("specify exactly one of endpoint or channel")
        if endpoint is not None and not endpoint:
            raise ValueError("endpoint must be a non-empty string")
        if timeout_seconds <= 0:
            raise ValueError("timeout_seconds must be positive")
        if header_mode is not None and header_mode not in (
            "bearer",
            "x-ficant-credential",
        ):
            raise ValueError(
                "header_mode must be 'bearer' or 'x-ficant-credential'"
            )
        if bearer_token is None and x_ficant_credential is None:
            raise ValueError(
                "NlpDraftOnlyClient requires a fixed NLP_PARSER credential "
                "(bearer_token or x_ficant_credential)"
            )
        if bearer_token is not None:
            _validate_raw_token(bearer_token, "bearer_token")
        if x_ficant_credential is not None:
            _validate_raw_token(x_ficant_credential, "x_ficant_credential")
        if (
            bearer_token is not None
            and x_ficant_credential is not None
            and bearer_token != x_ficant_credential
        ):
            raise ValueError(
                "bearer_token and x_ficant_credential must match when both are set"
            )

        raw_token = (
            x_ficant_credential
            if x_ficant_credential is not None
            else bearer_token
        )
        assert raw_token is not None
        if header_mode is None:
            effective_mode = (
                "x-ficant-credential"
                if x_ficant_credential is not None and bearer_token is None
                else "bearer"
            )
        else:
            if (
                header_mode == "bearer"
                and bearer_token is None
                and x_ficant_credential is not None
            ):
                raise ValueError(
                    "cannot use header_mode='bearer' when only x_ficant_credential is provided"
                )
            effective_mode = header_mode

        if effective_mode == "bearer":
            self._metadata: tuple[tuple[str, str], ...] = (
                (AUTHORIZATION_HEADER, f"Bearer {raw_token}"),
            )
        else:
            self._metadata = ((FICANT_CREDENTIAL_HEADER, raw_token),)

        self._expected_ledger: tuple[int, str] | None = (
            _validate_expected_ledger(expected_ledger)
            if expected_ledger is not None
            else None
        )
        self._timeout = timeout_seconds

        if channel is not None:
            self._channel = channel
            self._owns_channel = False
        else:
            assert endpoint is not None
            if insecure:
                self._channel = grpc.insecure_channel(endpoint)
            else:
                creds = grpc.ssl_channel_credentials(
                    root_certificates=root_certificates
                )
                self._channel = grpc.secure_channel(endpoint, creds)
            self._owns_channel = True

        # Structural 2-path binding ONLY — never instantiate DeskBlotterServiceStub!
        self._parse_nlp_intent = self._channel.unary_unary(
            PARSE_NLP_INTENT_METHOD,
            request_serializer=spi_pb2.ParseNlpIntentRequest.SerializeToString,
            response_deserializer=spi_pb2.ParseNlpIntentResponse.FromString,
        )
        self._submit_draft_earmark = self._channel.unary_unary(
            SUBMIT_DRAFT_EARMARK_METHOD,
            request_serializer=service_pb2.SubmitDraftEarmarkRequest.SerializeToString,
            response_deserializer=service_pb2.SubmitDraftEarmarkResponse.FromString,
        )

    @property
    def expected_ledger(self) -> tuple[int, str] | None:
        return self._expected_ledger

    def set_expected_ledger(self, expected_ledger: tuple[int, str] | None) -> None:
        self._expected_ledger = (
            _validate_expected_ledger(expected_ledger)
            if expected_ledger is not None
            else None
        )

    def close(self) -> None:
        if self._owns_channel:
            self._channel.close()

    def __enter__(self) -> NlpDraftOnlyClient:
        return self

    def __exit__(self, *_: object) -> None:
        self.close()

    def _build_metadata(
        self,
        *,
        expected_ledger: tuple[int, str] | None = None,
        use_default_expected_ledger: bool = False,
        extra_metadata: Sequence[tuple[str, str]] | None = None,
    ) -> tuple[tuple[str, str], ...]:
        merged: list[tuple[str, str]] = list(self._metadata)
        extra_pairs: list[tuple[str, str]] = []
        extra_has_ledger = False
        if extra_metadata is not None:
            for item in extra_metadata:
                if (
                    not isinstance(item, tuple)
                    or len(item) != 2
                    or not isinstance(item[0], str)
                    or not isinstance(item[1], str)
                ):
                    raise ValueError("metadata entries must be (str, str) pairs")
                key_lower = item[0].lower()
                if key_lower in (
                    EXPECTED_LEDGER_EPOCH_HEADER,
                    EXPECTED_LEDGER_INSTANCE_HEADER,
                ):
                    extra_has_ledger = True
                extra_pairs.append(item)
        if expected_ledger is not None and extra_has_ledger:
            raise ValueError(
                "cannot pass both expected_ledger and explicit x-ficant-expected-ledger-* metadata"
            )
        effective_ledger = (
            expected_ledger
            if expected_ledger is not None
            else (
                self._expected_ledger
                if (use_default_expected_ledger and not extra_has_ledger)
                else None
            )
        )
        if effective_ledger is not None:
            merged.extend(_expected_ledger_metadata(effective_ledger))
        merged.extend(extra_pairs)
        return tuple(merged)

    def parse_nlp_intent(
        self,
        request: spi_pb2.ParseNlpIntentRequest,
        *,
        metadata: Sequence[tuple[str, str]] | None = None,
    ) -> spi_pb2.NlpEarmarkProposal:
        call_metadata = self._build_metadata(
            use_default_expected_ledger=False,
            extra_metadata=metadata,
        )
        response: spi_pb2.ParseNlpIntentResponse = self._parse_nlp_intent(
            request, metadata=call_metadata, timeout=self._timeout
        )
        branch = response.WhichOneof("result")
        if branch == "error":
            raise BlotterError(response.error)
        if branch != "value":
            raise RuntimeError("ficant-server returned an incomplete ParseNlpIntent response")
        return response.value

    def submit_draft_earmark(
        self,
        request: service_pb2.SubmitDraftEarmarkRequest,
        *,
        expected_ledger: tuple[int, str] | None = None,
        metadata: Sequence[tuple[str, str]] | None = None,
    ) -> service_pb2.SubmitDraftEarmarkResult:
        call_metadata = self._build_metadata(
            expected_ledger=expected_ledger,
            use_default_expected_ledger=True,
            extra_metadata=metadata,
        )
        response: service_pb2.SubmitDraftEarmarkResponse = self._submit_draft_earmark(
            request, metadata=call_metadata, timeout=self._timeout
        )
        branch = response.WhichOneof("result")
        if branch == "error":
            raise BlotterError(response.error)
        if branch != "value":
            raise RuntimeError(
                "ficant-server returned an incomplete SubmitDraftEarmark response"
            )
        return response.value


# ======================================================================
# Deterministic Local Token Parser (`token` adapter)
# ======================================================================


def _tokenize_with_utf8_spans(text: str) -> list[tuple[int, int, str]]:
    tokens: list[tuple[int, int, str]] = []
    byte_offset = 0
    token_start_byte: int | None = None
    token_start_char: int | None = None

    for char_idx, ch in enumerate(text):
        ch_bytes = len(ch.encode("utf-8"))
        if ch in _TOKEN_DELIM_CHARS:
            if token_start_byte is not None and token_start_char is not None:
                tokens.append(
                    (token_start_byte, byte_offset, text[token_start_char:char_idx])
                )
                token_start_byte = None
                token_start_char = None
        else:
            if token_start_byte is None:
                token_start_byte = byte_offset
                token_start_char = char_idx
        byte_offset += ch_bytes

    if token_start_byte is not None and token_start_char is not None:
        tokens.append((token_start_byte, byte_offset, text[token_start_char:]))
    return tokens


def _char_to_utf8_byte_offsets(text: str) -> list[int]:
    offsets = [0]
    acc = 0
    for ch in text:
        acc += len(ch.encode("utf-8"))
        offsets.append(acc)
    return offsets


def _parse_chinese_or_numeric_amount(expr: str) -> Decimal | None:
    if not expr:
        return None
    multiplier = Decimal(1)
    core = expr
    if core[-1] in _CN_LARGE_UNITS:
        multiplier = Decimal(_CN_LARGE_UNITS[core[-1]])
        core = core[:-1]
        if not core:
            return None

    if re.fullmatch(r"\d+(?:\.\d+)?", core):
        return Decimal(core) * multiplier

    # Parse Chinese numeral section (e.g. 一千, 五百, 两千五百, 十, 二十)
    total = 0
    current_digit = 0
    has_any = False
    for ch in core:
        if ch in _CN_DIGIT_VALUES:
            current_digit = _CN_DIGIT_VALUES[ch]
            has_any = True
        elif ch in _CN_SMALL_UNITS:
            unit_val = _CN_SMALL_UNITS[ch]
            if current_digit == 0 and ch == "十" and not has_any:
                current_digit = 1
            elif current_digit == 0:
                return None
            total += current_digit * unit_val
            current_digit = 0
            has_any = True
        else:
            return None
    total += current_digit
    if not has_any or total <= 0:
        return None
    return Decimal(total) * multiplier


def _make_anchor(
    field_name: str,
    byte_start: int,
    byte_end: int,
    span_text: str,
) -> model_pb2.SourceTextAnchor:
    return model_pb2.SourceTextAnchor(
        field_name=field_name,
        byte_start=byte_start,
        byte_end=byte_end,
        span_text=span_text,
    )


def parse_token_nlp_intent(
    *,
    request_id: str,
    account_id: str,
    raw_source_text: str,
    units: rates_pb2.AnalysisUnits,
    parser_source_id: str = "reference-token-parser",
    model_version: str = "1.0.0",
    confidence_score: Decimal | str = Decimal("0.95"),
    default_clean_price: str | None = "100",
    row_overrides: Mapping[str, Any] | None = None,
) -> spi_pb2.NlpEarmarkProposal:
    """Deterministic local token/phrase parser producing exact UTF-8 byte anchors.

    Supports both structured key=value phrases (including Chinese/English hybrid
    such as ``"买入 T+0 INST=BOND_10Y NOTIONAL=10000000 CLEAN=100.25 TARGET=COMMITTED"``)
    and compact Chinese phrases (such as ``"买入十年国债BOND_10Y面额一千万"``).
    """
    validate_non_empty_text(request_id, "request_id")
    validate_non_empty_text(account_id, "account_id")
    validate_non_empty_text(parser_source_id, "parser_source_id")
    validate_non_empty_text(model_version, "model_version")
    if not isinstance(raw_source_text, str) or not raw_source_text.strip():
        raise ValueError("raw_source_text must be a non-empty string")

    side: int | None = None
    settlement_bucket: int | None = None
    instrument_code: str | None = None
    notional_val: Decimal | str | None = None
    clean_price_val: Decimal | str | None = None
    ytm_val: Decimal | str | None = None
    settlement_amount_val: Decimal | str | None = None
    target_state: int = model_pb2.DRAFT_TARGET_STATE_HOLD
    anchors: list[model_pb2.SourceTextAnchor] = []
    anchor_fields: set[str] = set()

    def add_anchor(field_name: str, start: int, end: int, span: str) -> None:
        if field_name in anchor_fields:
            raise ValueError(f"ambiguous duplicate {field_name!r} token in raw_source_text")
        anchor_fields.add(field_name)
        anchors.append(_make_anchor(field_name, start, end, span))

    # Pass 1: Tokenized key/value & keyword matching
    for tok_start, tok_end, token in _tokenize_with_utf8_spans(raw_source_text):
        upper = token.upper()
        if upper == "BUY" or token == "买入":
            side = model_pb2.TRADE_SIDE_BUY
            add_anchor("side", tok_start, tok_end, token)
        elif upper == "SELL" or token == "卖出":
            side = model_pb2.TRADE_SIDE_SELL
            add_anchor("side", tok_start, tok_end, token)
        elif upper in ("T+0", "T_PLUS_0"):
            settlement_bucket = model_pb2.SETTLEMENT_BUCKET_T_PLUS_0
            add_anchor("settlement_bucket", tok_start, tok_end, token)
        elif upper in ("T+1", "T_PLUS_1"):
            settlement_bucket = model_pb2.SETTLEMENT_BUCKET_T_PLUS_1
            add_anchor("settlement_bucket", tok_start, tok_end, token)
        elif "=" in token:
            key_raw, val_raw = token.split("=", 1)
            key_upper = key_raw.upper()
            val_start = tok_start + len(key_raw.encode("utf-8")) + 1
            if key_upper in ("INST", "INSTRUMENT"):
                instrument_code = validate_non_empty_text(val_raw, "instrument_code")
                add_anchor("instrument_code", val_start, tok_end, val_raw)
            elif key_upper in ("NOTIONAL", "QTY"):
                notional_val = validate_non_empty_text(val_raw, "notional")
                add_anchor("notional", val_start, tok_end, val_raw)
            elif key_upper in ("CLEAN", "PRICE"):
                clean_price_val = validate_non_empty_text(val_raw, "clean_price_per_100")
                add_anchor("clean_price_per_100", val_start, tok_end, val_raw)
            elif key_upper in ("YTM", "YIELD"):
                ytm_val = validate_non_empty_text(val_raw, "yield_to_maturity")
                add_anchor("yield_to_maturity", val_start, tok_end, val_raw)
            elif key_upper in ("SETTLE", "SETTLEMENT_AMOUNT"):
                settlement_amount_val = validate_non_empty_text(
                    val_raw, "settlement_amount"
                )
                add_anchor("settlement_amount", val_start, tok_end, val_raw)
            elif key_upper == "TARGET":
                resolved_target = _TARGET_STATE_MAP.get(val_raw.upper())
                if resolved_target is None:
                    raise ValueError(f"unsupported TARGET value: {val_raw!r}")
                target_state = resolved_target
                add_anchor("suggested_target_state", val_start, tok_end, val_raw)

    # Pass 2: Compact Chinese / unspaced phrase extraction (e.g. "买入十年国债BOND_10Y面额一千万")
    char_byte_offsets = _char_to_utf8_byte_offsets(raw_source_text)

    if side is None:
        buy_idx = raw_source_text.find("买入")
        sell_idx = raw_source_text.find("卖出")
        if buy_idx != -1 and sell_idx != -1:
            raise ValueError("ambiguous raw_source_text contains both 买入 and 卖出")
        if buy_idx != -1:
            side = model_pb2.TRADE_SIDE_BUY
            add_anchor(
                "side",
                char_byte_offsets[buy_idx],
                char_byte_offsets[buy_idx + 2],
                "买入",
            )
        elif sell_idx != -1:
            side = model_pb2.TRADE_SIDE_SELL
            add_anchor(
                "side",
                char_byte_offsets[sell_idx],
                char_byte_offsets[sell_idx + 2],
                "卖出",
            )

    if instrument_code is None:
        inst_matches = list(_EMBEDDED_INSTRUMENT_RE.finditer(raw_source_text))
        if len(inst_matches) == 1:
            m = inst_matches[0]
            instrument_code = m.group(0)
            add_anchor(
                "instrument_code",
                char_byte_offsets[m.start()],
                char_byte_offsets[m.end()],
                instrument_code,
            )
        elif len(inst_matches) > 1:
            raise ValueError(
                "ambiguous raw_source_text contains multiple instrument code candidates"
            )

    if notional_val is None:
        amt_label_match = re.search(r"(?:面额|金额|规模)", raw_source_text)
        if amt_label_match is not None:
            after_pos = amt_label_match.end()
            amt_match = _CN_AMOUNT_RE.match(raw_source_text, after_pos)
            if amt_match is not None:
                parsed_amt = _parse_chinese_or_numeric_amount(amt_match.group("expr"))
                if parsed_amt is not None and parsed_amt > 0:
                    notional_val = parsed_amt
                    add_anchor(
                        "notional",
                        char_byte_offsets[amt_match.start("expr")],
                        char_byte_offsets[amt_match.end("expr")],
                        amt_match.group("expr"),
                    )

    # Apply optional explicit row overrides (for fields not expressed in free-form text)
    if row_overrides is not None:
        if settlement_bucket is None and row_overrides.get("settlement_bucket"):
            raw_bkt = str(row_overrides["settlement_bucket"]).strip().upper()
            resolved_bkt = _SETTLEMENT_BUCKET_MAP.get(raw_bkt)
            if resolved_bkt is None:
                raise ValueError(f"unsupported settlement_bucket: {raw_bkt!r}")
            settlement_bucket = resolved_bkt
        if row_overrides.get("suggested_target_state") and "suggested_target_state" not in anchor_fields:
            raw_tgt = str(row_overrides["suggested_target_state"]).strip().upper()
            resolved_tgt = _TARGET_STATE_MAP.get(raw_tgt)
            if resolved_tgt is None:
                raise ValueError(f"unsupported suggested_target_state: {raw_tgt!r}")
            target_state = resolved_tgt
        if clean_price_val is None and ytm_val is None and row_overrides.get("clean_price_per_100") is not None:
            clean_price_val = row_overrides["clean_price_per_100"]
        if ytm_val is None and clean_price_val is None and row_overrides.get("yield_to_maturity") is not None:
            ytm_val = row_overrides["yield_to_maturity"]
        if settlement_amount_val is None and row_overrides.get("settlement_amount") is not None:
            settlement_amount_val = row_overrides["settlement_amount"]

    if side is None:
        raise ValueError("unable to parse trade side from raw_source_text")
    if instrument_code is None:
        raise ValueError("unable to parse instrument_code from raw_source_text")
    if notional_val is None:
        raise ValueError("unable to parse notional from raw_source_text")
    if settlement_bucket is None:
        settlement_bucket = model_pb2.SETTLEMENT_BUCKET_T_PLUS_0

    if clean_price_val is not None and ytm_val is not None:
        raise ValueError(
            "raw_source_text cannot specify both clean_price_per_100 and yield_to_maturity"
        )
    if clean_price_val is None and ytm_val is None and settlement_amount_val is None:
        if default_clean_price is not None:
            clean_price_val = default_clean_price
        else:
            raise ValueError(
                "unable to determine quote_terms (clean_price_per_100, yield_to_maturity, or settlement_amount)"
            )

    qt_kwargs: dict[str, Any] = {}
    if clean_price_val is not None:
        qt_kwargs["clean_price_per_100"] = decimal_to_pb(
            clean_price_val, units.price_per_100
        )
    if ytm_val is not None:
        qt_kwargs["yield_to_maturity"] = decimal_to_pb(ytm_val, units.rate)
    if settlement_amount_val is not None:
        qt_kwargs["settlement_amount"] = decimal_to_pb(
            settlement_amount_val, units.currency_amount
        )

    anchors.sort(key=lambda a: (a.byte_start, a.byte_end))
    proposal = spi_pb2.NlpEarmarkProposal(
        parser_source_id=parser_source_id,
        model_version=model_version,
        raw_source_text=raw_source_text,
        anchors=anchors,
        confidence_score=decimal_to_pb(confidence_score, units.dimensionless),
        account_id=account_id,
        instrument_code=instrument_code,
        side=side,
        settlement_bucket=settlement_bucket,
        notional=decimal_to_pb(notional_val, units.currency_amount),
        quote_terms=model_pb2.QuoteTerms(**qt_kwargs),
        suggested_target_state=target_state,
    )
    validate_nlp_proposal(
        proposal,
        expected_account_id=account_id,
        expected_raw_source_text=raw_source_text,
        units=units,
    )
    return proposal


# ======================================================================
# Strict Intranet Webhook Parser (`webhook` adapter)
# ======================================================================


def _reject_duplicate_json_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    out: dict[str, Any] = {}
    for key, value in pairs:
        if key in out:
            raise ValueError(f"duplicate JSON key {key!r} in webhook response")
        out[key] = value
    return out


def _parse_enum_str(raw: Any, mapping: Mapping[str, int], field_name: str) -> int:
    if not isinstance(raw, str):
        raise ValueError(f"{field_name} must be a string, got {type(raw).__name__}")
    resolved = mapping.get(raw.strip().upper())
    if resolved is None:
        raise ValueError(f"unsupported {field_name}: {raw!r}")
    return resolved


def _parse_webhook_response_payload(
    raw_body: bytes,
    *,
    expected_request_id: str,
    expected_account_id: str,
    expected_raw_source_text: str,
    units: rates_pb2.AnalysisUnits,
    default_parser_source_id: str,
    default_model_version: str,
) -> spi_pb2.NlpEarmarkProposal:
    if len(raw_body) > MAX_WEBHOOK_RESPONSE_BYTES:
        raise SidecarError(
            f"webhook response exceeds {MAX_WEBHOOK_RESPONSE_BYTES} byte limit"
        )
    try:
        decoded = json.loads(
            raw_body.decode("utf-8"),
            object_pairs_hook=_reject_duplicate_json_keys,
        )
    except Exception as exc:
        raise SidecarError(f"invalid webhook JSON response: {exc}") from exc

    if not isinstance(decoded, dict):
        raise SidecarError("webhook response JSON root must be an object")

    unknown_top = set(decoded.keys()) - _ALLOWED_WEBHOOK_TOP_LEVEL_KEYS
    if unknown_top:
        raise SidecarError(
            f"webhook response contains forbidden unknown key(s): {sorted(unknown_top)}"
        )

    if "request_id" in decoded:
        if decoded["request_id"] != expected_request_id:
            raise SidecarError(
                f"webhook echoed request_id mismatch: expected {expected_request_id!r}, "
                f"got {decoded['request_id']!r}"
            )

    if "account_id" not in decoded or not isinstance(decoded["account_id"], str):
        raise SidecarError("webhook response missing required echoed account_id")
    if decoded["account_id"] != expected_account_id:
        raise SidecarError(
            f"webhook echoed account_id mismatch: expected {expected_account_id!r}, "
            f"got {decoded['account_id']!r}"
        )

    if "raw_source_text" not in decoded or not isinstance(
        decoded["raw_source_text"], str
    ):
        raise SidecarError("webhook response missing required echoed raw_source_text")
    if decoded["raw_source_text"] != expected_raw_source_text:
        raise SidecarError(
            "webhook echoed raw_source_text does not byte-match caller input"
        )

    parser_source_id = validate_non_empty_text(
        decoded.get("parser_source_id", default_parser_source_id),
        "parser_source_id",
    )
    model_version = validate_non_empty_text(
        decoded.get("model_version", default_model_version),
        "model_version",
    )
    instrument_code = validate_non_empty_text(
        decoded.get("instrument_code"), "instrument_code"
    )
    side = _parse_enum_str(decoded.get("side"), _TRADE_SIDE_MAP, "side")
    settlement_bucket = _parse_enum_str(
        decoded.get("settlement_bucket"),
        _SETTLEMENT_BUCKET_MAP,
        "settlement_bucket",
    )
    suggested_target_state = _parse_enum_str(
        decoded.get("suggested_target_state"),
        _TARGET_STATE_MAP,
        "suggested_target_state",
    )

    if "confidence_score" not in decoded:
        raise SidecarError("webhook response missing required confidence_score")
    confidence_pb = decimal_to_pb(decoded["confidence_score"], units.dimensionless)

    if "notional" not in decoded:
        raise SidecarError("webhook response missing required notional")
    notional_pb = decimal_to_pb(decoded["notional"], units.currency_amount)

    has_nested_qt = "quote_terms" in decoded and decoded["quote_terms"] is not None
    flat_qt_keys = (
        {"clean_price_per_100", "yield_to_maturity", "settlement_amount"}
        & set(decoded.keys())
    )
    if has_nested_qt and flat_qt_keys:
        raise SidecarError(
            "webhook response cannot mix nested quote_terms with flat price/yield fields"
        )

    if has_nested_qt:
        qt_src = decoded["quote_terms"]
        if not isinstance(qt_src, dict):
            raise SidecarError("webhook quote_terms must be a JSON object")
        unknown_qt = set(qt_src.keys()) - _ALLOWED_WEBHOOK_QUOTE_TERMS_KEYS
        if unknown_qt:
            raise SidecarError(
                f"webhook quote_terms contains forbidden unknown key(s): {sorted(unknown_qt)}"
            )
    else:
        qt_src = {k: decoded[k] for k in flat_qt_keys if decoded[k] is not None}

    clean_raw = qt_src.get("clean_price_per_100")
    ytm_raw = qt_src.get("yield_to_maturity")
    settle_raw = qt_src.get("settlement_amount")
    if clean_raw is not None and ytm_raw is not None:
        raise SidecarError(
            "webhook quote_terms cannot specify both clean_price_per_100 and yield_to_maturity"
        )
    qt_kwargs: dict[str, Any] = {}
    if clean_raw is not None:
        qt_kwargs["clean_price_per_100"] = decimal_to_pb(
            clean_raw, units.price_per_100
        )
    if ytm_raw is not None:
        qt_kwargs["yield_to_maturity"] = decimal_to_pb(ytm_raw, units.rate)
    if settle_raw is not None:
        qt_kwargs["settlement_amount"] = decimal_to_pb(
            settle_raw, units.currency_amount
        )
    quote_terms_pb = model_pb2.QuoteTerms(**qt_kwargs)

    raw_anchors = decoded.get("anchors")
    if not isinstance(raw_anchors, list):
        raise SidecarError("webhook response missing anchors list")
    anchors_pb: list[model_pb2.SourceTextAnchor] = []
    for idx, item in enumerate(raw_anchors):
        if not isinstance(item, dict):
            raise SidecarError(f"webhook anchors[{idx}] must be a JSON object")
        unknown_anchor = set(item.keys()) - _ALLOWED_WEBHOOK_ANCHOR_KEYS
        if unknown_anchor:
            raise SidecarError(
                f"webhook anchors[{idx}] contains forbidden unknown key(s): {sorted(unknown_anchor)}"
            )
        missing_anchor = _ALLOWED_WEBHOOK_ANCHOR_KEYS - set(item.keys())
        if missing_anchor:
            raise SidecarError(
                f"webhook anchors[{idx}] missing required key(s): {sorted(missing_anchor)}"
            )
        b_start = item["byte_start"]
        b_end = item["byte_end"]
        if (
            isinstance(b_start, bool)
            or not isinstance(b_start, int)
            or isinstance(b_end, bool)
            or not isinstance(b_end, int)
            or b_start < 0
            or b_end < 0
            or b_end > _MAX_U32
        ):
            raise SidecarError(
                f"webhook anchors[{idx}] byte_start/byte_end must be uint32 integers"
            )
        if not isinstance(item["span_text"], str):
            raise SidecarError(f"webhook anchors[{idx}].span_text must be a string")
        anchors_pb.append(
            model_pb2.SourceTextAnchor(
                field_name=validate_non_empty_text(
                    item["field_name"], f"anchors[{idx}].field_name"
                ),
                byte_start=b_start,
                byte_end=b_end,
                span_text=item["span_text"],
            )
        )

    proposal = spi_pb2.NlpEarmarkProposal(
        parser_source_id=parser_source_id,
        model_version=model_version,
        raw_source_text=expected_raw_source_text,
        anchors=anchors_pb,
        confidence_score=confidence_pb,
        account_id=expected_account_id,
        instrument_code=instrument_code,
        side=side,
        settlement_bucket=settlement_bucket,
        notional=notional_pb,
        quote_terms=quote_terms_pb,
        suggested_target_state=suggested_target_state,
    )
    validate_nlp_proposal(
        proposal,
        expected_account_id=expected_account_id,
        expected_raw_source_text=expected_raw_source_text,
        units=units,
    )
    return proposal


def _invoke_webhook_post(
    *,
    webhook_url: str,
    timeout_seconds: float,
    payload: Mapping[str, Any],
    ca_cert_path: Path | None = None,
    root_certificates: bytes | None = None,
) -> bytes:
    """POST JSON strictly to the configured intranet webhook URL (no redirects, 256 KiB cap)."""
    scheme, host, port, path = _validate_webhook_url(webhook_url)
    body_bytes = json.dumps(payload, ensure_ascii=False, separators=(",", ":")).encode(
        "utf-8"
    )
    conn: http.client.HTTPConnection
    if scheme == "https":
        if ca_cert_path is not None and root_certificates is not None:
            raise ValueError("specify at most one of ca_cert_path or root_certificates")
        if ca_cert_path is not None:
            ctx = ssl.create_default_context(cafile=str(ca_cert_path))
        elif root_certificates is not None:
            ctx = ssl.create_default_context(
                cadata=root_certificates.decode("ascii")
            )
        else:
            ctx = ssl.create_default_context()
        conn = http.client.HTTPSConnection(
            host, port, timeout=timeout_seconds, context=ctx
        )
    else:
        conn = http.client.HTTPConnection(host, port, timeout=timeout_seconds)

    try:
        conn.request(
            "POST",
            path,
            body=body_bytes,
            headers={
                "Content-Type": "application/json; charset=utf-8",
                "Accept": "application/json",
                "Content-Length": str(len(body_bytes)),
            },
        )
        resp = conn.getresponse()
        if 300 <= resp.status < 400:
            raise SidecarError(
                f"webhook redirect HTTP {resp.status} is forbidden; configure exact intranet endpoint"
            )
        content_length_raw = resp.getheader("Content-Length")
        if content_length_raw is not None:
            try:
                content_length = int(content_length_raw.strip())
            except ValueError as exc:
                raise SidecarError(
                    f"webhook returned invalid Content-Length header: {content_length_raw!r}"
                ) from exc
            if content_length > MAX_WEBHOOK_RESPONSE_BYTES:
                raise SidecarError(
                    f"webhook Content-Length {content_length} exceeds {MAX_WEBHOOK_RESPONSE_BYTES} byte limit"
                )
        raw_body = resp.read(MAX_WEBHOOK_RESPONSE_BYTES + 1)
        if len(raw_body) > MAX_WEBHOOK_RESPONSE_BYTES:
            raise SidecarError(
                f"webhook response body exceeds {MAX_WEBHOOK_RESPONSE_BYTES} byte limit"
            )
        if resp.status != 200:
            raise SidecarError(
                f"webhook returned HTTP {resp.status} (expected 200)"
            )
        return raw_body
    except (TimeoutError, ConnectionError):
        raise
    except (OSError, http.client.HTTPException) as exc:
        raise ConnectionError(f"webhook HTTP request failed: {exc}") from exc
    finally:
        conn.close()


# ======================================================================
# Gateway Configuration, Report & Sidecar Engine
# ======================================================================


@dataclass(frozen=True)
class NlpDraftGatewayConfig:
    """Configuration for ``NlpDraftGatewaySidecar``."""

    sidecar_id: str
    trading_date: str
    checkpoint_path: Path
    input_path: Path
    adapter: Literal["token", "webhook", "spi"] = "token"
    parser_source_id: str = "reference-token-parser"
    model_version: str = "1.0.0"
    actor_scope: str = "nlp-parser"
    webhook_url: str | None = None
    webhook_timeout_ms: int = 5_000
    webhook_ca_cert_path: Path | None = None
    webhook_root_certificates: bytes | None = None
    expected_ledger: tuple[int, str] | None = None
    reset_checkpoint_for_restore: bool = False
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
        if self.adapter not in ("token", "webhook", "spi"):
            raise ValueError("adapter must be 'token', 'webhook', or 'spi'")
        validate_non_empty_text(self.parser_source_id, "parser_source_id")
        validate_non_empty_text(self.model_version, "model_version")
        validate_non_empty_text(self.actor_scope, "actor_scope")
        if (
            isinstance(self.webhook_timeout_ms, bool)
            or not isinstance(self.webhook_timeout_ms, int)
            or self.webhook_timeout_ms <= 0
        ):
            raise ValueError("webhook_timeout_ms must be a positive integer")
        if self.adapter == "webhook":
            if self.webhook_url is None:
                raise ValueError("webhook_url is required when adapter='webhook'")
            _validate_webhook_url(self.webhook_url)
        elif self.webhook_url is not None:
            _validate_webhook_url(self.webhook_url)
        if self.expected_ledger is not None:
            LedgerIdentity(
                ledger_epoch=self.expected_ledger[0],
                ledger_instance_id=self.expected_ledger[1],
            )
        if self.health_timeout_seconds <= 0:
            raise ValueError("health_timeout_seconds must be positive")


@dataclass(frozen=True)
class NlpDraftBatchReport:
    """Summary report produced by ``NlpDraftGatewaySidecar.run_once``."""

    ledger_epoch: int
    ledger_instance_id: str
    adapter: str
    requests_read: int
    drafts_submitted: int
    requests_skipped_dedup: int
    replayed_in_flight: bool
    server_replayed_count: int
    quarantined_previous_in_flight: bool
    draft_ids: tuple[str, ...]
    committed_seqs: tuple[int, ...]


def parse_nlp_input_row(row: Mapping[str, Any]) -> tuple[str, str, str, dict[str, Any]]:
    """Validate one JSONL input row into ``(request_id, account_id, raw_source_text, overrides)``."""
    if not isinstance(row, Mapping):
        raise ValueError("NLP JSONL line must decode to a JSON object")
    unknown = set(row.keys()) - _ALLOWED_NLP_INPUT_ROW_KEYS
    if unknown:
        raise ValueError(f"unknown NLP input row field(s): {sorted(unknown)}")

    req_id_raw = row.get("request_id")
    acct_id_raw = row.get("account_id")
    raw_text = row.get("raw_source_text")

    request_id = validate_non_empty_text(
        req_id_raw.strip() if isinstance(req_id_raw, str) else req_id_raw,
        "request_id",
    )
    account_id = validate_non_empty_text(
        acct_id_raw.strip() if isinstance(acct_id_raw, str) else acct_id_raw,
        "account_id",
    )
    if not isinstance(raw_text, str) or not raw_text.strip():
        raise ValueError("raw_source_text must be a non-empty string")

    overrides: dict[str, Any] = {}
    for key in (
        "clean_price_per_100",
        "yield_to_maturity",
        "settlement_amount",
        "settlement_bucket",
        "suggested_target_state",
    ):
        val = row.get(key)
        if val is not None and not (isinstance(val, str) and val.strip() == ""):
            overrides[key] = val.strip() if isinstance(val, str) else val

    return (request_id, account_id, raw_text, overrides)


def _read_jsonl_rows_from_offset(
    path: Path, *, start_offset: int
) -> list[tuple[dict[str, Any], int]]:
    file_size = path.stat().st_size
    if start_offset > file_size:
        raise CheckpointCorruptionError(
            f"input file {path} size ({file_size}) is smaller than checkpoint confirmed_offset ({start_offset})"
        )
    rows: list[tuple[dict[str, Any], int]] = []
    with path.open("rb") as handle:
        handle.seek(start_offset)
        while True:
            raw_line = handle.readline()
            if not raw_line:
                break
            next_offset = handle.tell()
            decoded = raw_line.decode(
                "utf-8-sig" if start_offset == 0 and not rows else "utf-8"
            ).strip()
            if not decoded:
                continue
            obj = json.loads(decoded, object_pairs_hook=_reject_duplicate_json_keys)
            if not isinstance(obj, dict):
                raise ValueError(f"JSONL line in {path} must decode to a JSON object")
            rows.append((obj, next_offset))
    return rows


def _initial_nlp_state() -> dict[str, Any]:
    return {
        "completed_requests": {},
        "request_counter": 0,
    }


def _make_nlp_idempotency_key(
    *,
    sidecar_id: str,
    ledger: LedgerIdentity,
    actor_scope: str,
    account_id: str,
    request_id: str,
    raw_source_text_sha256: str,
) -> str:
    h = hashlib.sha256()
    h.update(sidecar_id.encode("utf-8"))
    h.update(b"\x00")
    h.update(str(ledger.ledger_epoch).encode("ascii"))
    h.update(b"\x00")
    h.update(ledger.ledger_instance_id.encode("ascii"))
    h.update(b"\x00")
    h.update(actor_scope.encode("utf-8"))
    h.update(b"\x00")
    h.update(account_id.encode("utf-8"))
    h.update(b"\x00")
    h.update(request_id.encode("utf-8"))
    h.update(b"\x00")
    h.update(raw_source_text_sha256.encode("ascii"))
    return f"nlp-draft:{sidecar_id}:e{ledger.ledger_epoch}:{h.hexdigest()[:16]}"


class NlpDraftGatewaySidecar:
    """Crash-safe NLP draft submission sidecar (F-20) with durable pre-submit intent."""

    def __init__(
        self,
        config: NlpDraftGatewayConfig,
        client: NlpDraftOnlyClient | Any,
        *,
        health_probe: HealthProbeFn | None = None,
        on_retry: Callable[[int, BaseException, float], None] | None = None,
        on_intent_persisted: Callable[[dict[str, Any]], None] | None = None,
    ) -> None:
        self._config = config
        self._client = client
        self._health_probe = health_probe
        self._on_retry = on_retry
        self._on_intent_persisted = on_intent_persisted
        # NLP_PARSER structurally never performs GetAccountSnapshot discovery.
        self._units_resolver = AccountUnitsResolver(
            explicit_units=config.explicit_units,
            client=None,
            allow_snapshot_discovery=False,
        )

    @property
    def config(self) -> NlpDraftGatewayConfig:
        return self._config

    @property
    def units_resolver(self) -> AccountUnitsResolver:
        return self._units_resolver

    def _parse_with_adapter(
        self,
        *,
        request_id: str,
        account_id: str,
        raw_source_text: str,
        units: rates_pb2.AnalysisUnits,
        overrides: Mapping[str, Any],
    ) -> spi_pb2.NlpEarmarkProposal:
        adapter = self._config.adapter
        if adapter == "token":
            return parse_token_nlp_intent(
                request_id=request_id,
                account_id=account_id,
                raw_source_text=raw_source_text,
                units=units,
                parser_source_id=self._config.parser_source_id,
                model_version=self._config.model_version,
                row_overrides=overrides,
            )
        if adapter == "webhook":
            assert self._config.webhook_url is not None
            payload = {
                "request_id": request_id,
                "account_id": account_id,
                "raw_source_text": raw_source_text,
            }
            raw_body = self._config.backoff.execute(
                lambda: _invoke_webhook_post(
                    webhook_url=self._config.webhook_url,  # type: ignore[arg-type]
                    timeout_seconds=self._config.webhook_timeout_ms / 1_000.0,
                    payload=payload,
                    ca_cert_path=self._config.webhook_ca_cert_path
                    or self._config.ca_cert_path,
                    root_certificates=self._config.webhook_root_certificates
                    or self._config.root_certificates,
                ),
                on_retry=self._on_retry,
            )
            return _parse_webhook_response_payload(
                raw_body,
                expected_request_id=request_id,
                expected_account_id=account_id,
                expected_raw_source_text=raw_source_text,
                units=units,
                default_parser_source_id=self._config.parser_source_id,
                default_model_version=self._config.model_version,
            )

        # adapter == "spi"
        req = spi_pb2.ParseNlpIntentRequest(
            request_id=request_id,
            account_id=account_id,
            raw_source_text=raw_source_text,
        )
        proposal = self._config.backoff.execute(
            lambda: self._client.parse_nlp_intent(req),
            on_retry=self._on_retry,
        )
        validate_nlp_proposal(
            proposal,
            expected_account_id=account_id,
            expected_raw_source_text=raw_source_text,
            units=units,
        )
        return proposal

    @staticmethod
    def _validate_submit_result(
        result: service_pb2.SubmitDraftEarmarkResult,
    ) -> str:
        if not result.HasField("draft"):
            raise SidecarError("SubmitDraftEarmarkResult missing draft")
        if not result.draft.HasField("draft_id"):
            raise SidecarError("SubmitDraftEarmarkResult draft missing draft_id")
        draft_id = result.draft.draft_id.value
        if not is_canonical_ulid(draft_id):
            raise SidecarError(
                f"SubmitDraftEarmarkResult returned invalid draft_id={draft_id!r}"
            )
        return draft_id

    @staticmethod
    def _commit_intent_receipt(
        store: JsonCheckpointStore,
        checkpoint: dict[str, Any],
        in_flight: Mapping[str, Any],
        result: service_pb2.SubmitDraftEarmarkResult,
        draft_id: str,
    ) -> None:
        state = checkpoint["state"]
        completed: dict[str, dict[str, Any]] = state.setdefault(
            "completed_requests", {}
        )
        request_id = str(in_flight["request_id"])
        completed[request_id] = {
            "request_id": request_id,
            "account_id": str(in_flight["account_id"]),
            "actor_scope": str(in_flight["actor_scope"]),
            "raw_source_text_sha256": str(in_flight["raw_source_text_sha256"]),
            "idempotency_key": str(in_flight["idempotency_key"]),
            "ledger_epoch": int(in_flight["ledger_epoch"]),
            "ledger_instance_id": str(in_flight["ledger_instance_id"]),
            "draft_id": draft_id,
            "committed_seq": int(result.committed_seq),
            "replayed": bool(result.replayed),
        }
        state["request_counter"] = int(state.get("request_counter", 0)) + 1
        checkpoint["confirmed_offset"] = int(in_flight["pending_offset"])
        checkpoint["input_fingerprint"] = str(in_flight["pending_input_fingerprint"])
        checkpoint["in_flight"] = None
        store.save(checkpoint)

    def run_once(self) -> NlpDraftBatchReport:
        """Probe ``/healthz``, replay any durable pre-submit intent, and process new JSONL rows."""
        input_path = Path(self._config.input_path)
        if not input_path.is_file():
            raise SidecarError(f"NLP input file does not exist: {input_path}")

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
                kind="nlp_draft",
                sidecar_id=self._config.sidecar_id,
                trading_date=self._config.trading_date,
                input_fingerprint=input_fingerprint,
                initial_state_factory=_initial_nlp_state,
                expected_ledger=self._config.expected_ledger,
                reset_checkpoint_for_restore=self._config.reset_checkpoint_for_restore,
            )
            ledger = reconcile.ledger
            checkpoint = reconcile.checkpoint
            state = checkpoint["state"]
            completed: dict[str, dict[str, Any]] = state.setdefault(
                "completed_requests", {}
            )
            state.setdefault("request_counter", 0)

            requests_read = 0
            drafts_submitted = 0
            requests_skipped_dedup = 0
            replayed_in_flight = False
            server_replayed_count = 0
            draft_ids: list[str] = []
            committed_seqs: list[int] = []

            # Phase 1: Replay persisted pre-submit intent verbatim WITHOUT re-calling parser/webhook
            if checkpoint.get("in_flight") is not None:
                in_flight = checkpoint["in_flight"]
                if (
                    int(in_flight.get("ledger_epoch", -1)) != ledger.ledger_epoch
                    or str(in_flight.get("ledger_instance_id", ""))
                    != ledger.ledger_instance_id
                ):
                    raise CheckpointCorruptionError(
                        "persisted NLP in_flight intent ledger identity does not match active ledger"
                    )
                req = service_pb2.SubmitDraftEarmarkRequest()
                req.ParseFromString(base64.b64decode(in_flight["request_pb_b64"]))
                if req.idempotency_key != str(in_flight["idempotency_key"]):
                    raise CheckpointCorruptionError(
                        "persisted NLP in_flight idempotency_key mismatch with serialized request"
                    )
                if req.account_id != str(in_flight["account_id"]):
                    raise CheckpointCorruptionError(
                        "persisted NLP in_flight account_id mismatch with serialized request"
                    )

                res = self._config.backoff.execute(
                    lambda: self._client.submit_draft_earmark(
                        req, expected_ledger=ledger.as_tuple()
                    ),
                    on_retry=self._on_retry,
                )
                draft_id = self._validate_submit_result(res)
                self._commit_intent_receipt(store, checkpoint, in_flight, res, draft_id)

                replayed_in_flight = True
                drafts_submitted += 1
                if res.replayed:
                    server_replayed_count += 1
                draft_ids.append(draft_id)
                committed_seqs.append(int(res.committed_seq))

            # Phase 2: Read new JSONL rows after confirmed_offset
            current_fingerprint = compute_file_fingerprint(input_path)
            raw_rows = _read_jsonl_rows_from_offset(
                input_path, start_offset=checkpoint["confirmed_offset"]
            )

            for row_dict, row_end_offset in raw_rows:
                requests_read += 1
                request_id, account_id, raw_source_text, overrides = (
                    parse_nlp_input_row(row_dict)
                )
                raw_text_sha256 = hashlib.sha256(
                    raw_source_text.encode("utf-8")
                ).hexdigest()

                existing = completed.get(request_id)
                if existing is not None:
                    if (
                        existing.get("raw_source_text_sha256") != raw_text_sha256
                        or existing.get("account_id") != account_id
                        or existing.get("actor_scope") != self._config.actor_scope
                    ):
                        raise SidecarError(
                            f"conflicting reuse of request_id={request_id!r} with altered "
                            "raw_source_text, account_id, or actor_scope"
                        )
                    requests_skipped_dedup += 1
                    checkpoint["confirmed_offset"] = row_end_offset
                    checkpoint["input_fingerprint"] = current_fingerprint
                    store.save(checkpoint)
                    continue

                units = self._units_resolver.resolve(account_id)
                proposal = self._parse_with_adapter(
                    request_id=request_id,
                    account_id=account_id,
                    raw_source_text=raw_source_text,
                    units=units,
                    overrides=overrides,
                )

                idempotency_key = _make_nlp_idempotency_key(
                    sidecar_id=self._config.sidecar_id,
                    ledger=ledger,
                    actor_scope=self._config.actor_scope,
                    account_id=account_id,
                    request_id=request_id,
                    raw_source_text_sha256=raw_text_sha256,
                )
                req = service_pb2.SubmitDraftEarmarkRequest(
                    idempotency_key=idempotency_key,
                    account_id=account_id,
                    proposal=proposal,
                )
                req_bytes = req.SerializeToString()

                in_flight_entry = {
                    "request_id": request_id,
                    "raw_source_text_sha256": raw_text_sha256,
                    "account_id": account_id,
                    "actor_scope": self._config.actor_scope,
                    "ledger_epoch": ledger.ledger_epoch,
                    "ledger_instance_id": ledger.ledger_instance_id,
                    "idempotency_key": idempotency_key,
                    "request_pb_b64": base64.b64encode(req_bytes).decode("ascii"),
                    "pending_offset": row_end_offset,
                    "pending_input_fingerprint": current_fingerprint,
                }
                # Atomically persist durable submit intent BEFORE sending SubmitDraftEarmark
                checkpoint["in_flight"] = in_flight_entry
                store.save(checkpoint)
                if self._on_intent_persisted is not None:
                    self._on_intent_persisted(dict(in_flight_entry))

                res = self._config.backoff.execute(
                    lambda: self._client.submit_draft_earmark(
                        req, expected_ledger=ledger.as_tuple()
                    ),
                    on_retry=self._on_retry,
                )
                draft_id = self._validate_submit_result(res)
                self._commit_intent_receipt(
                    store, checkpoint, in_flight_entry, res, draft_id
                )

                drafts_submitted += 1
                if res.replayed:
                    server_replayed_count += 1
                draft_ids.append(draft_id)
                committed_seqs.append(int(res.committed_seq))

            return NlpDraftBatchReport(
                ledger_epoch=ledger.ledger_epoch,
                ledger_instance_id=ledger.ledger_instance_id,
                adapter=self._config.adapter,
                requests_read=requests_read,
                drafts_submitted=drafts_submitted,
                requests_skipped_dedup=requests_skipped_dedup,
                replayed_in_flight=replayed_in_flight,
                server_replayed_count=server_replayed_count,
                quarantined_previous_in_flight=reconcile.quarantined_previous_in_flight,
                draft_ids=tuple(draft_ids),
                committed_seqs=tuple(committed_seqs),
            )
