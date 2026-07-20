Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$binaryName = if ($IsWindows) { 'ficant-server.exe' } else { 'ficant-server' }
$serverBinary = [System.IO.Path]::GetFullPath((Join-Path $repoRoot "target/debug/$binaryName"))
if (-not (Test-Path -LiteralPath $serverBinary -PathType Leaf)) {
    throw "Phase 2E live SDK check requires the already-built candidate binary: $serverBinary"
}

$previousServerBinary = $env:FICANT_PHASE2E_SERVER_BIN
try {
    $env:FICANT_PHASE2E_SERVER_BIN = $serverBinary
    & uv run --offline --locked --project python python -m pytest python/tests/test_rates_sdk_live.py -q
    $exitCode = $LASTEXITCODE
}
finally {
    $env:FICANT_PHASE2E_SERVER_BIN = $previousServerBinary
}

exit $exitCode
