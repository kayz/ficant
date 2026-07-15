"""Normalization helpers for restricted third-party source facts.

Only derived fixtures and immutable source hashes are retained.  Raw source
records are deliberately neither accepted nor written by the build workflow.
"""

from datetime import datetime, timezone
from zoneinfo import ZoneInfo


MARKET_TIMEZONE = ZoneInfo("Asia/Shanghai")


def market_date_from_epoch_ms(epoch_ms: int):
    instant = datetime.fromtimestamp(epoch_ms / 1000, tz=timezone.utc)
    return instant.astimezone(MARKET_TIMEZONE).date()
