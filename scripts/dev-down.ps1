[CmdletBinding()]
param(
    [switch]$ListOnly
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = Split-Path -Parent $PSScriptRoot
$composeDirectory = Join-Path $repoRoot 'deploy\dev'
$composeFile = Join-Path $composeDirectory 'docker-compose.yml'
$environmentFile = Join-Path $composeDirectory '.env.local'

if ($ListOnly) {
    Write-Output "docker compose --project-directory `"$composeDirectory`" --env-file `"$environmentFile`" --file `"$composeFile`" --profile dev --profile ui down --remove-orphans"
    Write-Output 'Named PostgreSQL and Ceph volumes are preserved.'
    exit 0
}

if (-not (Test-Path -LiteralPath $environmentFile -PathType Leaf)) {
    throw "Local environment file does not exist: $environmentFile"
}

$arguments = @(
    'compose',
    '--project-directory', $composeDirectory,
    '--env-file', $environmentFile,
    '--file', $composeFile,
    '--profile', 'dev',
    '--profile', 'ui',
    'down',
    '--remove-orphans'
)

# Compose interpolates every service before processing `down`. The two deployment identities are
# derived transiently by dev-up and are intentionally not persisted beside local credentials, so
# provide syntax-valid non-secret placeholders only for this non-starting operation.
$previousRuntimeDigest = $env:FICANT_WORKER_RUNTIME_IMAGE_DIGEST
$previousSourceDigest = $env:FICANT_WORKER_NATIVE_SOURCE_DIGEST
try {
    if ([string]::IsNullOrWhiteSpace($env:FICANT_WORKER_RUNTIME_IMAGE_DIGEST)) {
        $env:FICANT_WORKER_RUNTIME_IMAGE_DIGEST = "sha256:$('00' * 32)"
    }
    if ([string]::IsNullOrWhiteSpace($env:FICANT_WORKER_NATIVE_SOURCE_DIGEST)) {
        $env:FICANT_WORKER_NATIVE_SOURCE_DIGEST = "sha256:$('00' * 32)"
    }
    & docker @arguments
    if ($LASTEXITCODE -ne 0) {
        throw "docker compose down failed with exit code $LASTEXITCODE."
    }
}
finally {
    $env:FICANT_WORKER_RUNTIME_IMAGE_DIGEST = $previousRuntimeDigest
    $env:FICANT_WORKER_NATIVE_SOURCE_DIGEST = $previousSourceDigest
}

Write-Output 'FICANT development containers stopped. Named PostgreSQL and Ceph volumes were preserved.'
