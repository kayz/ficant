"""Module entrypoint for ``python -m ficant_sdk.sidecar``."""

from __future__ import annotations

from ficant_sdk.sidecar.cli import main

if __name__ == "__main__":
    raise SystemExit(main())
