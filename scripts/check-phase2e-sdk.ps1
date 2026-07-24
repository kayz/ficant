Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$binaryName = if ($IsWindows) { 'ficant-server.exe' } else { 'ficant-server' }
$serverBinary = [System.IO.Path]::GetFullPath((Join-Path $repoRoot "target/debug/$binaryName"))
$workerBinaryName = if ($IsWindows) { 'ficant-worker.exe' } else { 'ficant-worker' }
$workerBinary = [System.IO.Path]::GetFullPath(
    (Join-Path $repoRoot "target/debug/$workerBinaryName")
)
if (-not (Test-Path -LiteralPath $serverBinary -PathType Leaf)) {
    throw "Phase 2E live SDK check requires the already-built candidate binary: $serverBinary"
}
if (-not (Test-Path -LiteralPath $workerBinary -PathType Leaf)) {
    throw "Phase 2E live SDK check requires the already-built Worker binary: $workerBinary"
}

$nativeSourceDigest = (& $workerBinary --print-native-source-digest)
if ($LASTEXITCODE -ne 0 -or $nativeSourceDigest -notmatch '^sha256:[0-9a-f]{64}$') {
    throw 'The candidate Worker did not report a canonical native source digest.'
}
$experimentEnvironment = [ordered]@{
    FICANT_EXPERIMENT_DATABASE_URL = 'postgres://ficant:ficant@127.0.0.1:1/ficant'
    FICANT_EXPERIMENT_S3_ENDPOINT = 'http://127.0.0.1:1'
    FICANT_EXPERIMENT_S3_BUCKET = 'phase2e-local'
    FICANT_EXPERIMENT_S3_ACCESS_KEY = 'phase2e-local'
    FICANT_EXPERIMENT_S3_SECRET_KEY = 'phase2e-local-secret'
    FICANT_EXPERIMENT_CURSOR_KEY_HEX = ('11' * 32)
    FICANT_EXPERIMENT_TENANT_ID = '01J00000000000000000000010'
    FICANT_EXPERIMENT_OWNER_ID = '01J00000000000000000000011'
    FICANT_EXPERIMENT_ACTOR_ID = '01J00000000000000000000012'
    FICANT_EXPERIMENT_RUNTIME_IMAGE_DIGEST = "sha256:$('22' * 32)"
    FICANT_EXPERIMENT_ENVIRONMENT_ATTESTATION = (
        "ficant.worker.environment.v1`n" +
        "arch=amd64`n" +
        "os=windows`n" +
        'profile=phase2e-local'
    )
    FICANT_EXPERIMENT_NATIVE_SOURCE_DIGEST = $nativeSourceDigest.Trim()
}

$previousServerBinary = $env:FICANT_PHASE2E_SERVER_BIN
$previousEnvironment = @{}
try {
    $env:FICANT_PHASE2E_SERVER_BIN = $serverBinary
    foreach ($entry in $experimentEnvironment.GetEnumerator()) {
        $previousEnvironment[$entry.Key] = [System.Environment]::GetEnvironmentVariable(
            $entry.Key,
            [System.EnvironmentVariableTarget]::Process
        )
        [System.Environment]::SetEnvironmentVariable(
            $entry.Key,
            $entry.Value,
            [System.EnvironmentVariableTarget]::Process
        )
    }
    & uv run --offline --locked --project python python -m pytest python/tests/test_rates_sdk_live.py -q
    $exitCode = $LASTEXITCODE
}
finally {
    $env:FICANT_PHASE2E_SERVER_BIN = $previousServerBinary
    foreach ($entry in $previousEnvironment.GetEnumerator()) {
        [System.Environment]::SetEnvironmentVariable(
            $entry.Key,
            $entry.Value,
            [System.EnvironmentVariableTarget]::Process
        )
    }
}

exit $exitCode
