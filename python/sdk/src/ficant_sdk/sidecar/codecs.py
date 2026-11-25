"""Canonical decimal, MarketTime, AnalysisUnits, and Rust-identical hashing codecs for sidecars."""

from __future__ import annotations

from collections.abc import Mapping
from datetime import date, datetime, timedelta, timezone, tzinfo
from decimal import Decimal, InvalidOperation
import hashlib
import json
from pathlib import Path
import re
import struct
from typing import Any
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

from ficant.blotter.v1 import model_pb2, service_pb2, spi_pb2
from ficant.core.v1 import common_pb2, error_pb2
from ficant.market.v1 import fact_pb2
from ficant.rates.v1 import analytics_pb2 as rates_pb2
from google.protobuf.timestamp_pb2 import Timestamp

from ficant_sdk.blotter import BlotterError

MARKET_TIMEZONE = "Asia/Shanghai"
DECIMAL_SCALE = 12
MAX_DECIMAL_PRECISION = 28
_I128_MIN = -(1 << 127)
_I128_MAX = (1 << 127) - 1
_MAX_U64 = (1 << 64) - 1

_ULID_ALPHABET = frozenset("0123456789ABCDEFGHJKMNPQRSTVWXYZ")
_ULID_FIRST_CHARS = frozenset("01234567")

_RFC3339_RE = re.compile(
    r"\A"
    r"(?P<year>\d{4})-(?P<month>\d{2})-(?P<day>\d{2})"
    r"[Tt]"
    r"(?P<hour>\d{2}):(?P<minute>\d{2}):(?P<second>\d{2})"
    r"(?:\.(?P<frac>\d{1,9}))?"
    r"(?P<tz>[Zz]|[+-]\d{2}:\d{2})"
    r"\Z"
)
_ISO_DATE_RE = re.compile(r"\A\d{4}-\d{2}-\d{2}\Z")

# Fixed-offset fallback when system/tzdata zoneinfo database is unavailable (e.g. minimal Windows).
_FIXED_OFFSET_IANA_FALLBACKS: dict[str, tzinfo] = {
    "Asia/Shanghai": timezone(timedelta(hours=8)),
    "Asia/Hong_Kong": timezone(timedelta(hours=8)),
    "Asia/Singapore": timezone(timedelta(hours=8)),
    "Asia/Tokyo": timezone(timedelta(hours=9)),
    "UTC": timezone.utc,
    "Etc/UTC": timezone.utc,
}

_EPOCH_UTC = datetime(1970, 1, 1, tzinfo=timezone.utc)

_DEFAULT_REFERENCE_UNIT_ULIDS: dict[str, str] = {
    "currency_amount": "01ARZ3NDEKTSV4RRFFQ69G5FA1",
    "price_per_100": "01ARZ3NDEKTSV4RRFFQ69G5FA2",
    "rate": "01ARZ3NDEKTSV4RRFFQ69G5FA3",
    "dv01": "01ARZ3NDEKTSV4RRFFQ69G5FA4",
    "dv01_per_100": "01ARZ3NDEKTSV4RRFFQ69G5FA5",
    "dimensionless": "01ARZ3NDEKTSV4RRFFQ69G5FA6",
    "contract_count": "01ARZ3NDEKTSV4RRFFQ69G5FA7",
}

_REQUIRED_ANALYSIS_UNIT_FIELDS = tuple(_DEFAULT_REFERENCE_UNIT_ULIDS.keys())

_PRICE_SOURCE_TYPE_HASH_TAGS: dict[int, int] = {
    fact_pb2.PRICE_SOURCE_TYPE_REAL_TRADE: 1,
    fact_pb2.PRICE_SOURCE_TYPE_ACTIVE_QUOTE: 2,
    fact_pb2.PRICE_SOURCE_TYPE_MODEL_VALUATION: 3,
    fact_pb2.PRICE_SOURCE_TYPE_CURVE_INTERPOLATION: 4,
}

_TRADE_ACTION_HASH_TAGS: dict[int, int] = {
    model_pb2.TRADE_FEED_RECORD_ACTION_NEW_FILL: 1,
    model_pb2.TRADE_FEED_RECORD_ACTION_AMEND: 2,
    model_pb2.TRADE_FEED_RECORD_ACTION_CANCEL_REVERSAL: 3,
}

_TRADE_SIDE_HASH_TAGS: dict[int, int] = {
    model_pb2.TRADE_SIDE_BUY: 1,
    model_pb2.TRADE_SIDE_SELL: 2,
}


def is_canonical_ulid(value: object) -> bool:
    """Return True iff ``value`` is a canonical 26-char uppercase Crockford Base32 ULID."""
    if not isinstance(value, str) or len(value) != 26 or not value.isascii():
        return False
    return value[0] in _ULID_FIRST_CHARS and all(ch in _ULID_ALPHABET for ch in value)


def validate_non_empty_text(value: object, field_name: str) -> str:
    """Validate that ``value`` is a non-empty trimmed string without leading/trailing whitespace."""
    if not isinstance(value, str) or not value or value != value.strip():
        raise ValueError(
            f"{field_name} must be a non-empty string without leading/trailing whitespace"
        )
    return value


def validate_iso_date(value: object, field_name: str = "date") -> date:
    """Parse and validate a strict ``YYYY-MM-DD`` calendar date."""
    if not isinstance(value, str) or not _ISO_DATE_RE.fullmatch(value):
        raise ValueError(f"{field_name} must be an ISO YYYY-MM-DD date string")
    try:
        return date.fromisoformat(value)
    except ValueError as exc:
        raise ValueError(f"invalid {field_name}: {value!r}") from exc


def validate_unit_ref(unit: common_pb2.UnitRef, field_name: str = "unit") -> common_pb2.UnitRef:
    """Validate that ``unit`` carries a canonical ULID and version >= 1."""
    if not isinstance(unit, common_pb2.UnitRef):
        raise ValueError(f"{field_name} must be a UnitRef protobuf message")
    if not unit.HasField("unit_id") or not is_canonical_ulid(unit.unit_id.value):
        raise ValueError(f"{field_name}.unit_id must be a canonical 26-char uppercase ULID")
    if isinstance(unit.version, bool) or not isinstance(unit.version, int) or unit.version < 1:
        raise ValueError(f"{field_name}.version must be >= 1")
    return unit


def default_reference_units() -> rates_pb2.AnalysisUnits:
    """Build the default reference ``AnalysisUnits`` matching Rust ``default_reference_analysis_units``."""
    return rates_pb2.AnalysisUnits(
        currency_amount=common_pb2.UnitRef(
            unit_id=common_pb2.Ulid(value=_DEFAULT_REFERENCE_UNIT_ULIDS["currency_amount"]),
            version=1,
        ),
        price_per_100=common_pb2.UnitRef(
            unit_id=common_pb2.Ulid(value=_DEFAULT_REFERENCE_UNIT_ULIDS["price_per_100"]),
            version=1,
        ),
        rate=common_pb2.UnitRef(
            unit_id=common_pb2.Ulid(value=_DEFAULT_REFERENCE_UNIT_ULIDS["rate"]),
            version=1,
        ),
        dv01=common_pb2.UnitRef(
            unit_id=common_pb2.Ulid(value=_DEFAULT_REFERENCE_UNIT_ULIDS["dv01"]),
            version=1,
        ),
        dv01_per_100=common_pb2.UnitRef(
            unit_id=common_pb2.Ulid(value=_DEFAULT_REFERENCE_UNIT_ULIDS["dv01_per_100"]),
            version=1,
        ),
        dimensionless=common_pb2.UnitRef(
            unit_id=common_pb2.Ulid(value=_DEFAULT_REFERENCE_UNIT_ULIDS["dimensionless"]),
            version=1,
        ),
        contract_count=common_pb2.UnitRef(
            unit_id=common_pb2.Ulid(value=_DEFAULT_REFERENCE_UNIT_ULIDS["contract_count"]),
            version=1,
        ),
    )


def validate_analysis_units(units: rates_pb2.AnalysisUnits) -> rates_pb2.AnalysisUnits:
    """Validate all required ``UnitRef`` fields on ``AnalysisUnits``."""
    if not isinstance(units, rates_pb2.AnalysisUnits):
        raise ValueError("units must be an AnalysisUnits protobuf message")
    for field_name in _REQUIRED_ANALYSIS_UNIT_FIELDS:
        if not units.HasField(field_name):
            raise ValueError(f"AnalysisUnits missing required field {field_name!r}")
        validate_unit_ref(getattr(units, field_name), f"units.{field_name}")
    for optional_field in ("years", "years_squared"):
        if units.HasField(optional_field):
            validate_unit_ref(getattr(units, optional_field), f"units.{optional_field}")
    return units


def _clone_analysis_units(units: rates_pb2.AnalysisUnits) -> rates_pb2.AnalysisUnits:
    copy = rates_pb2.AnalysisUnits()
    copy.CopyFrom(units)
    return copy


def parse_units_json(raw: str | Path | Mapping[str, Any]) -> rates_pb2.AnalysisUnits:
    """Parse ``AnalysisUnits`` from a JSON string, file path, or mapping."""
    if isinstance(raw, Path):
        data = json.loads(raw.read_text(encoding="utf-8"))
    elif isinstance(raw, str):
        data = json.loads(raw)
    elif isinstance(raw, Mapping):
        data = dict(raw)
    else:
        raise ValueError("units JSON source must be a Path, JSON string, or mapping")

    if not isinstance(data, Mapping):
        raise ValueError("units JSON must decode to an object")

    allowed_fields = set(_REQUIRED_ANALYSIS_UNIT_FIELDS) | {"years", "years_squared"}
    unknown = set(data.keys()) - allowed_fields
    if unknown:
        raise ValueError(f"unknown AnalysisUnits fields: {sorted(unknown)}")

    kwargs: dict[str, common_pb2.UnitRef] = {}
    for field_name, entry in data.items():
        if isinstance(entry, str):
            unit_id = entry
            version = 1
        elif isinstance(entry, Mapping):
            unknown_entry = set(entry.keys()) - {"unit_id", "version"}
            if unknown_entry:
                raise ValueError(
                    f"unknown fields in units.{field_name}: {sorted(unknown_entry)}"
                )
            unit_id = entry.get("unit_id")
            version = entry.get("version", 1)
        else:
            raise ValueError(f"invalid UnitRef specification for {field_name!r}")
        if not is_canonical_ulid(unit_id):
            raise ValueError(f"units.{field_name}.unit_id must be a canonical ULID")
        if isinstance(version, bool) or not isinstance(version, int) or version < 1:
            raise ValueError(f"units.{field_name}.version must be a positive integer")
        kwargs[field_name] = common_pb2.UnitRef(
            unit_id=common_pb2.Ulid(value=unit_id),
            version=version,
        )

    return validate_analysis_units(rates_pb2.AnalysisUnits(**kwargs))


class AccountUnitsResolver:
    """Resolves per-account ``AnalysisUnits`` with strict precedence.

    Precedence:
      1. Explicit ``units`` (global or per-account mapping).
      2. Opt-in snapshot discovery (``allow_snapshot_discovery=True`` only).
      3. ``default_reference_units()``.

    When ``allow_snapshot_discovery=False`` (default), makes **0**
    ``GetAccountSnapshot`` RPC calls. When ``allow_snapshot_discovery=True``,
    ``UNAUTHENTICATED`` / ``FORBIDDEN`` errors propagate immediately without fallback.
    """

    def __init__(
        self,
        *,
        explicit_units: rates_pb2.AnalysisUnits
        | Mapping[str, rates_pb2.AnalysisUnits]
        | None = None,
        client: Any | None = None,
        allow_snapshot_discovery: bool = False,
    ) -> None:
        if isinstance(explicit_units, rates_pb2.AnalysisUnits):
            validate_analysis_units(explicit_units)
            self._global_explicit: rates_pb2.AnalysisUnits | None = _clone_analysis_units(
                explicit_units
            )
            self._per_account_explicit: dict[str, rates_pb2.AnalysisUnits] = {}
        elif isinstance(explicit_units, Mapping):
            self._global_explicit = None
            self._per_account_explicit = {}
            for acct_id, acct_units in explicit_units.items():
                validate_non_empty_text(acct_id, "account_id")
                validate_analysis_units(acct_units)
                self._per_account_explicit[acct_id] = _clone_analysis_units(acct_units)
        elif explicit_units is None:
            self._global_explicit = None
            self._per_account_explicit = {}
        else:
            raise ValueError(
                "explicit_units must be AnalysisUnits, a mapping of account_id -> AnalysisUnits, or None"
            )

        self._client = client
        self._allow_snapshot_discovery = bool(allow_snapshot_discovery)
        self._discovered_cache: dict[str, rates_pb2.AnalysisUnits] = {}

    def resolve(self, account_id: str) -> rates_pb2.AnalysisUnits:
        validate_non_empty_text(account_id, "account_id")
        if account_id in self._per_account_explicit:
            return _clone_analysis_units(self._per_account_explicit[account_id])
        if self._global_explicit is not None:
            return _clone_analysis_units(self._global_explicit)
        if account_id in self._discovered_cache:
            return _clone_analysis_units(self._discovered_cache[account_id])

        if self._allow_snapshot_discovery:
            if self._client is None:
                raise ValueError(
                    "client is required when allow_snapshot_discovery=True"
                )
            try:
                result = self._client.get_account_snapshot(
                    service_pb2.GetAccountSnapshotRequest(account_id=account_id)
                )
            except BlotterError as exc:
                if exc.code in (
                    error_pb2.ERROR_CODE_UNAUTHENTICATED,
                    error_pb2.ERROR_CODE_FORBIDDEN,
                ):
                    raise
                if exc.code == error_pb2.ERROR_CODE_NOT_FOUND:
                    return default_reference_units()
                raise

            if (
                result.HasField("snapshot")
                and result.snapshot.HasField("opening_base")
                and result.snapshot.opening_base.HasField("units")
            ):
                units = validate_analysis_units(result.snapshot.opening_base.units)
                self._discovered_cache[account_id] = _clone_analysis_units(units)
                return _clone_analysis_units(units)

        return default_reference_units()


def _parse_canonical_decimal_parts(
    value: Decimal | int | str,
) -> tuple[str, int, int]:
    """Return ``(canonical_coefficient_str, normalized_scale, scaled_i128)``.

    Enforces exact Rust parity:
    - Rejects ``bool``, ``float``, non-finite ``Decimal`` (NaN/sNaN/Infinity),
      ``> 28`` significant digits, normalized ``scale > 12``, and ``10**12``-scaled
      ``i128`` overflow.
    - Emits canonical ``("0", 0, 0)`` for zero and strips trailing fractional zeros.
    """
    if isinstance(value, bool):
        raise ValueError("bool is not a valid decimal value")
    if isinstance(value, float):
        raise ValueError("float is not allowed for exact financial decimals; use Decimal, int, or str")

    if isinstance(value, Decimal):
        dec = value
    elif isinstance(value, int):
        dec = Decimal(value)
    elif isinstance(value, str):
        if not value or value != value.strip():
            raise ValueError(f"decimal string must be non-empty and trimmed: {value!r}")
        try:
            dec = Decimal(value)
        except InvalidOperation as exc:
            raise ValueError(f"invalid decimal string: {value!r}") from exc
    else:
        raise ValueError(f"unsupported decimal input type: {type(value).__name__}")

    if not dec.is_finite():
        raise ValueError(f"decimal must be finite, got {dec!r}")

    sign, digits, exp = dec.as_tuple()
    assert isinstance(exp, int)

    # Zero check (handles 0, -0, 0.000, 0e-20, 0e50, etc.)
    if not digits or all(d == 0 for d in digits):
        return ("0", 0, 0)

    # Strip leading zeros from digit tuple
    first_non_zero = 0
    while first_non_zero < len(digits) and digits[first_non_zero] == 0:
        first_non_zero += 1
    sig_digits = list(digits[first_non_zero:])

    if exp >= 0:
        if len(sig_digits) + exp > MAX_DECIMAL_PRECISION:
            raise ValueError(
                f"decimal exceeds {MAX_DECIMAL_PRECISION} significant digits: {value!r}"
            )
        coeff_str = "".join(str(d) for d in sig_digits) + ("0" * exp)
        scale = 0
    else:
        scale = -exp
        while scale > 0 and sig_digits and sig_digits[-1] == 0:
            sig_digits.pop()
            scale -= 1
        if len(sig_digits) > MAX_DECIMAL_PRECISION:
            raise ValueError(
                f"decimal exceeds {MAX_DECIMAL_PRECISION} significant digits: {value!r}"
            )
        if scale > DECIMAL_SCALE:
            raise ValueError(
                f"normalized decimal scale {scale} exceeds maximum {DECIMAL_SCALE}: {value!r}"
            )
        coeff_str = "".join(str(d) for d in sig_digits)

    coeff_int = int(coeff_str)
    if sign:
        coeff_int = -coeff_int
        canonical_coeff = f"-{coeff_str}"
    else:
        canonical_coeff = coeff_str

    scaled = coeff_int * (10 ** (DECIMAL_SCALE - scale))
    if scaled < _I128_MIN or scaled > _I128_MAX:
        raise ValueError(
            f"decimal scaled by 10^{DECIMAL_SCALE} overflows signed i128: {value!r}"
        )

    return (canonical_coeff, scale, scaled)


def decimal_to_pb(
    value: Decimal | int | str,
    unit: common_pb2.UnitRef,
) -> common_pb2.DecimalValue:
    """Encode an exact decimal to a canonical ``DecimalValue`` protobuf."""
    validate_unit_ref(unit, "unit")
    coeff_str, scale, _scaled = _parse_canonical_decimal_parts(value)
    unit_copy = common_pb2.UnitRef()
    unit_copy.CopyFrom(unit)
    return common_pb2.DecimalValue(
        coefficient=coeff_str,
        scale=scale,
        unit=unit_copy,
    )


def fixed_decimal_scaled_i128(
    value: Decimal | int | str | common_pb2.DecimalValue,
) -> int:
    """Compute the exact signed ``i128`` matching Rust ``FixedDecimal::scaled()``."""
    if isinstance(value, common_pb2.DecimalValue):
        coeff = value.coefficient
        scale = value.scale
        if not isinstance(coeff, str) or not coeff or coeff != coeff.strip():
            raise ValueError(f"invalid DecimalValue coefficient: {coeff!r}")
        if isinstance(scale, bool) or not isinstance(scale, int) or scale < 0 or scale > DECIMAL_SCALE:
            raise ValueError(f"invalid DecimalValue scale: {scale!r}")
        unsigned = coeff[1:] if coeff.startswith("-") else coeff
        if not unsigned or not unsigned.isascii() or not unsigned.isdigit():
            raise ValueError(f"non-canonical DecimalValue coefficient: {coeff!r}")
        if len(unsigned) > MAX_DECIMAL_PRECISION:
            raise ValueError(f"DecimalValue precision exceeds {MAX_DECIMAL_PRECISION}")
        if coeff == "-0":
            raise ValueError("non-canonical negative zero in DecimalValue")
        if len(unsigned) > 1 and unsigned.startswith("0"):
            raise ValueError(f"leading zeros in DecimalValue coefficient: {coeff!r}")
        if scale > 0 and unsigned.endswith("0"):
            raise ValueError(
                f"trailing fractional zero in DecimalValue coefficient={coeff!r} scale={scale}"
            )
        coeff_int = int(coeff)
        if coeff_int == 0 and scale != 0:
            raise ValueError("zero DecimalValue must have scale=0")
        scaled = coeff_int * (10 ** (DECIMAL_SCALE - scale))
        if scaled < _I128_MIN or scaled > _I128_MAX:
            raise ValueError("DecimalValue scaled value overflows signed i128")
        return scaled

    _coeff_str, _scale, scaled = _parse_canonical_decimal_parts(value)
    return scaled


def fixed_decimal_to_be_bytes(
    value: Decimal | int | str | common_pb2.DecimalValue,
) -> bytes:
    """Encode ``FixedDecimal::scaled()`` as 16-byte big-endian signed two's complement."""
    return fixed_decimal_scaled_i128(value).to_bytes(16, "big", signed=True)


def _resolve_iana_tz(market_timezone: str) -> tzinfo:
    validate_non_empty_text(market_timezone, "market_timezone")
    try:
        return ZoneInfo(market_timezone)
    except ZoneInfoNotFoundError:
        fallback = _FIXED_OFFSET_IANA_FALLBACKS.get(market_timezone)
        if fallback is not None:
            return fallback
        raise ValueError(f"unknown or unavailable IANA market_timezone: {market_timezone!r}")


def parse_market_time(
    value: str,
    *,
    market_timezone: str = MARKET_TIMEZONE,
    local_trading_date: str | None = None,
) -> common_pb2.MarketTime:
    """Parse an RFC3339 timestamp with explicit offset into ``MarketTime``.

    Preserves full 9-digit nanosecond precision in wire ``Timestamp(seconds, nanos)``.
    """
    if not isinstance(value, str):
        raise ValueError("RFC3339 timestamp must be a string")
    match = _RFC3339_RE.fullmatch(value)
    if match is None:
        raise ValueError(
            f"invalid RFC3339 timestamp (explicit Z or ±HH:MM offset and <=9 fractional digits required): {value!r}"
        )

    year = int(match.group("year"))
    month = int(match.group("month"))
    day = int(match.group("day"))
    hour = int(match.group("hour"))
    minute = int(match.group("minute"))
    second = int(match.group("second"))
    frac = match.group("frac") or ""
    tz_raw = match.group("tz")

    if tz_raw in ("Z", "z"):
        offset_tz = timezone.utc
    else:
        sign = 1 if tz_raw[0] == "+" else -1
        off_h = int(tz_raw[1:3])
        off_m = int(tz_raw[4:6])
        if off_h > 23 or off_m > 59:
            raise ValueError(f"invalid RFC3339 timezone offset: {tz_raw!r}")
        offset_tz = timezone(timedelta(hours=sign * off_h, minutes=sign * off_m))

    try:
        dt_aware = datetime(year, month, day, hour, minute, second, tzinfo=offset_tz)
    except ValueError as exc:
        raise ValueError(f"invalid calendar date/time in RFC3339 timestamp: {value!r}") from exc

    utc_dt = dt_aware.astimezone(timezone.utc)
    delta = utc_dt - _EPOCH_UTC
    seconds = delta.days * 86_400 + delta.seconds
    nanos = int(frac.ljust(9, "0")) if frac else 0

    tz_obj = _resolve_iana_tz(market_timezone)
    computed_local_date = utc_dt.astimezone(tz_obj).date()

    if local_trading_date is not None:
        explicit_date = validate_iso_date(local_trading_date, "local_trading_date")
        if explicit_date != computed_local_date:
            raise ValueError(
                f"local_trading_date {explicit_date.isoformat()!r} does not match "
                f"timestamp date {computed_local_date.isoformat()!r} in {market_timezone!r}"
            )
        final_date_str = explicit_date.isoformat()
    else:
        final_date_str = computed_local_date.isoformat()

    return common_pb2.MarketTime(
        instant=Timestamp(seconds=seconds, nanos=nanos),
        market_timezone=market_timezone,
        local_trading_date=final_date_str,
    )


def market_time_from_utc_datetime(
    dt_utc: datetime,
    *,
    market_timezone: str = MARKET_TIMEZONE,
    extra_nanos: int = 0,
) -> common_pb2.MarketTime:
    """Build a ``MarketTime`` from a timezone-aware ``datetime``."""
    if dt_utc.tzinfo is None:
        raise ValueError("datetime must be timezone-aware")
    if extra_nanos < 0 or extra_nanos >= 1_000:
        raise ValueError("extra_nanos must be in [0, 999]")
    utc_dt = dt_utc.astimezone(timezone.utc)
    delta = utc_dt - _EPOCH_UTC
    seconds = delta.days * 86_400 + delta.seconds
    nanos = utc_dt.microsecond * 1_000 + extra_nanos
    tz_obj = _resolve_iana_tz(market_timezone)
    local_date_str = utc_dt.astimezone(tz_obj).date().isoformat()
    return common_pb2.MarketTime(
        instant=Timestamp(seconds=seconds, nanos=nanos),
        market_timezone=market_timezone,
        local_trading_date=local_date_str,
    )


def market_time_instant_key(mt: common_pb2.MarketTime) -> tuple[int, int]:
    """Return exact ``(seconds, nanos)`` ordering key for a ``MarketTime``."""
    if not mt.HasField("instant"):
        raise ValueError("MarketTime missing instant")
    nanos = mt.instant.nanos
    if nanos < 0 or nanos > 999_999_999:
        raise ValueError(f"MarketTime instant.nanos out of range: {nanos}")
    return (mt.instant.seconds, nanos)


def market_time_timestamp_micros(mt: common_pb2.MarketTime) -> int:
    """Compute exact microsecond floor matching Rust ``chrono::DateTime::timestamp_micros()``."""
    seconds, nanos = market_time_instant_key(mt)
    return seconds * 1_000_000 + (nanos // 1_000)


def market_time_to_rfc3339(mt: common_pb2.MarketTime) -> str:
    """Render a ``MarketTime`` instant as canonical UTC RFC3339 preserving nanosecond precision."""
    seconds, nanos = market_time_instant_key(mt)
    utc_dt = _EPOCH_UTC + timedelta(seconds=seconds)
    base = utc_dt.strftime("%Y-%m-%dT%H:%M:%S")
    if nanos == 0:
        return f"{base}Z"
    frac = f"{nanos:09d}".rstrip("0")
    return f"{base}.{frac}Z"


# ======================================================================
# Rust-Identical Canonical Binary Encoding & SHA-256 Hashing
# ======================================================================


def _append_u8(buf: bytearray, value: int) -> None:
    if value < 0 or value > 255:
        raise ValueError(f"u8 out of range: {value}")
    buf.append(value)


def _append_u64(buf: bytearray, value: int) -> None:
    if isinstance(value, bool) or value < 0 or value > _MAX_U64:
        raise ValueError(f"u64 out of range: {value}")
    buf.extend(struct.pack(">Q", value))


def _append_bytes(buf: bytearray, data: bytes) -> None:
    _append_u64(buf, len(data))
    buf.extend(data)


def _append_str(buf: bytearray, text: str) -> None:
    _append_bytes(buf, text.encode("utf-8"))


def _append_fixed(
    buf: bytearray,
    value: Decimal | int | str | common_pb2.DecimalValue,
) -> None:
    buf.extend(fixed_decimal_to_be_bytes(value))


def _append_opt_fixed(
    buf: bytearray,
    value: Decimal | int | str | common_pb2.DecimalValue | None,
) -> None:
    if value is None:
        _append_u8(buf, 0)
    else:
        _append_u8(buf, 1)
        _append_fixed(buf, value)


def _append_market_time(buf: bytearray, mt: common_pb2.MarketTime) -> None:
    micros = market_time_timestamp_micros(mt)
    buf.extend(struct.pack(">q", micros))
    _append_str(buf, mt.market_timezone)
    _append_str(buf, mt.local_trading_date)


def _append_quote_terms(buf: bytearray, quote: model_pb2.QuoteTerms | None) -> None:
    if quote is None:
        _append_u8(buf, 0)
        return
    _append_u8(buf, 1)
    branch = quote.WhichOneof("price_or_yield")
    if branch == "clean_price_per_100":
        _append_u8(buf, 1)
        _append_fixed(buf, quote.clean_price_per_100)
    elif branch == "yield_to_maturity":
        _append_u8(buf, 2)
        _append_fixed(buf, quote.yield_to_maturity)
    else:
        _append_u8(buf, 0)

    if quote.HasField("settlement_amount"):
        _append_u8(buf, 1)
        _append_fixed(buf, quote.settlement_amount)
    else:
        _append_u8(buf, 0)


def market_quote_canonical_payload_hash(tick: spi_pb2.MarketQuoteTick) -> bytes:
    """Compute the 32-byte SHA-256 digest identical to Rust ``MarketQuoteTick::canonical_payload_hash``.

    Deliberately excludes ``received_at`` and hashes ``observed_at`` with microsecond
    floor precision (matching ``chrono::DateTime::timestamp_micros()``).
    """
    source_tag = _PRICE_SOURCE_TYPE_HASH_TAGS.get(tick.source_type)
    if source_tag is None:
        raise ValueError(f"unsupported PriceSourceType: {tick.source_type}")
    if not tick.HasField("observed_at"):
        raise ValueError("MarketQuoteTick missing observed_at")

    buf = bytearray()
    _append_str(buf, tick.source_id)
    _append_str(buf, tick.instrument_code)
    _append_u64(buf, tick.quote_seq)
    _append_u8(buf, source_tag)
    _append_opt_fixed(
        buf, tick.bid_clean_price if tick.HasField("bid_clean_price") else None
    )
    _append_opt_fixed(
        buf, tick.ask_clean_price if tick.HasField("ask_clean_price") else None
    )
    _append_opt_fixed(
        buf, tick.mid_clean_price if tick.HasField("mid_clean_price") else None
    )
    _append_opt_fixed(
        buf,
        tick.mid_yield_to_maturity if tick.HasField("mid_yield_to_maturity") else None,
    )
    _append_market_time(buf, tick.observed_at)
    return hashlib.sha256(bytes(buf)).digest()


def trade_fill_business_hash(
    fill: spi_pb2.TradeFillRecord,
    *,
    exact_dirty_price_input: tuple[Decimal | int | str, Decimal | int | str] | None = None,
) -> bytes:
    """Compute the 32-byte SHA-256 digest identical to Rust ``TradeFillRecord::business_hash``.

    Excludes transport ``feed_seq`` and source batch bounds ``batch_first_feed_seq`` /
    ``batch_last_feed_seq``.
    """
    action_tag = _TRADE_ACTION_HASH_TAGS.get(fill.action)
    if action_tag is None:
        raise ValueError(f"unsupported TradeFeedRecordAction: {fill.action}")
    side_tag = _TRADE_SIDE_HASH_TAGS.get(fill.side)
    if side_tag is None:
        raise ValueError(f"unsupported TradeSide: {fill.side}")
    if not fill.HasField("notional"):
        raise ValueError("TradeFillRecord missing notional")
    if not fill.HasField("executed_at"):
        raise ValueError("TradeFillRecord missing executed_at")
    settlement_date = validate_iso_date(fill.settlement_date, "settlement_date")

    buf = bytearray()
    _append_str(buf, fill.source_id)
    _append_str(buf, fill.external_trade_id)
    _append_u64(buf, fill.source_revision)
    _append_u8(buf, action_tag)
    _append_str(buf, fill.account_id)
    _append_str(buf, fill.instrument_code)
    _append_u8(buf, side_tag)
    _append_str(buf, settlement_date.isoformat())
    _append_fixed(buf, fill.notional)
    _append_quote_terms(buf, fill.quote_terms if fill.HasField("quote_terms") else None)
    _append_market_time(buf, fill.executed_at)
    if exact_dirty_price_input is not None:
        dirty_price, face_value = exact_dirty_price_input
        _append_u8(buf, 1)
        _append_fixed(buf, dirty_price)
        _append_fixed(buf, face_value)
    else:
        _append_u8(buf, 0)
    return hashlib.sha256(bytes(buf)).digest()


def trade_fill_payload_hash(
    fill: spi_pb2.TradeFillRecord,
    *,
    exact_dirty_price_input: tuple[Decimal | int | str, Decimal | int | str] | None = None,
) -> bytes:
    """Compute the 32-byte SHA-256 digest identical to Rust ``TradeFillRecord::payload_hash``.

    Includes ``feed_seq``, ``batch_first_feed_seq``, ``batch_last_feed_seq``, and
    length-prefixed ``business_hash()`` bytes.
    """
    biz_hash = trade_fill_business_hash(
        fill, exact_dirty_price_input=exact_dirty_price_input
    )
    buf = bytearray()
    _append_u64(buf, fill.feed_seq)
    _append_u64(buf, fill.batch_first_feed_seq)
    _append_u64(buf, fill.batch_last_feed_seq)
    _append_bytes(buf, biz_hash)
    return hashlib.sha256(bytes(buf)).digest()
