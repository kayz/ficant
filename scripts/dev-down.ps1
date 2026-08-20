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

# Compose interpolates every service before processing `down`. Deployment identities are derived
# transiently by dev-up and are intentionally not persisted beside local credentials, so provide
# syntax-valid non-secret placeholders only for this non-starting operation. The bootstrap identity
# placeholders also let down clean an older local environment file created before those fields were
# required; none of these values are used to start a service.
$placeholderEnvironment = @{
    FICANT_CODE_COMMIT_SHA = '0' * 40
    FICANT_CODE_TREE_SHA = '0' * 40
    FICANT_SERVER_RUNTIME_IMAGE_DIGEST = "sha256:$('00' * 32)"
    FICANT_SERVER_ENVIRONMENT_ATTESTATION = "sha256:$('00' * 32)"
    FICANT_WORKER_RUNTIME_IMAGE_DIGEST = "sha256:$('00' * 32)"
    FICANT_WORKER_NATIVE_SOURCE_DIGEST = "sha256:$('00' * 32)"
    FICANT_BOOTSTRAP_ACTOR_ID = '01J00000000000000000000012'
    FICANT_BOOTSTRAP_TENANT_ID = '01J00000000000000000000010'
    FICANT_BOOTSTRAP_ALLOWED_OWNER_IDS = '01J00000000000000000000011'
    FICANT_BOOTSTRAP_ACTIVE_ROLE = 'PLATFORM_ADMIN'
}
$previousEnvironment = @{}
try {
    foreach ($name in $placeholderEnvironment.Keys) {
        $previousEnvironment[$name] = [Environment]::GetEnvironmentVariable($name)
        if ([string]::IsNullOrWhiteSpace($previousEnvironment[$name])) {
            [Environment]::SetEnvironmentVariable(
                $name,
                $placeholderEnvironment[$name],
                [EnvironmentVariableTarget]::Process
            )
        }
    }
    & docker @arguments
    if ($LASTEXITCODE -ne 0) {
        throw "docker compose down failed with exit code $LASTEXITCODE."
    }
}
finally {
    foreach ($name in $previousEnvironment.Keys) {
        [Environment]::SetEnvironmentVariable(
            $name,
            $previousEnvironment[$name],
            [EnvironmentVariableTarget]::Process
        )
    }
}

Write-Output 'FICANT development containers stopped. Named PostgreSQL and Ceph volumes were preserved.'
