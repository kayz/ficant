[CmdletBinding()]
param(
    [switch]$ListOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'check-common.ps1')

$candidateSha = Get-FicantCommandOutput -FilePath 'git' -ArgumentList @(
    '-C', $script:FicantRoot, 'rev-parse', 'HEAD'
)
if ($candidateSha -notmatch '^[0-9a-f]{40}$') {
    throw 'Release preflight requires a canonical Git commit identity.'
}
$candidateTreeRevision = '{0}^{{tree}}' -f $candidateSha
$candidateTree = Get-FicantCommandOutput -FilePath 'git' -ArgumentList @(
    '-C', $script:FicantRoot, 'rev-parse', $candidateTreeRevision
)
if ($candidateTree -notmatch '^[0-9a-f]{40}$') {
    throw 'Release preflight requires a canonical Git tree identity.'
}
$imagePrefix = 'ficant-preflight/ficant'
$storageLock = Get-Content -LiteralPath (Join-Path $script:FicantRoot 'deploy\storage-runtime.lock.json') `
    -Raw | ConvertFrom-Json
$storageImage = "$($storageLock.image)@$($storageLock.oci.index_digest)"
$storageConfigDigest = [string]$storageLock.oci.config_digest
$images = @(
    "$imagePrefix-server:sha-$candidateSha"
    "$imagePrefix-worker:sha-$candidateSha"
    "$imagePrefix-ui:sha-$candidateSha"
)
$bindingSteps = @(
    New-FicantCheckStep -Name 'License inventory binding regression tests' -FilePath 'python' -ArgumentList @(
        '.github/scripts/tests/test_license_inventory_bindings.py'
    )
    New-FicantCheckStep -Name 'Verify release license inventory input bindings' -FilePath 'python' -ArgumentList @(
        '.github/scripts/verify-license-inventory.py', 'verify-bindings',
        '--inventory', '.github/scripts/license-inventory.lock.json',
        '--cargo-lock', 'Cargo.lock',
        '--uv-lock', 'python/uv.lock',
        '--pnpm-lock', 'web-dm/pnpm-lock.yaml',
        '--supply-lock', '.github/scripts/supply-chain.lock.json',
        '--release-root', '.',
        '--require-first-party'
    )
    New-FicantCheckStep -Name 'Storage runtime lock regression tests' -FilePath 'python' -ArgumentList @(
        '.github/scripts/tests/test_storage_runtime_lock.py'
    )
    New-FicantCheckStep -Name 'Verify storage runtime build-input bindings' -FilePath 'python' -ArgumentList @(
        'deploy/verify-storage-runtime.py', 'verify-lock',
        '--lock', 'deploy/storage-runtime.lock.json', '--root', '.'
    )
    New-FicantCheckStep -Name 'Verify remote storage runtime OCI bindings' -FilePath 'python' -ArgumentList @(
        'deploy/verify-storage-runtime.py', 'verify-remote',
        '--lock', 'deploy/storage-runtime.lock.json'
    )
)
$buildSteps = @(
    New-FicantCheckStep -Name 'Build release server image' -FilePath 'docker' -ArgumentList @(
        'build', '--pull=false', '--file', 'deploy/dev/RustService.Dockerfile',
        '--build-arg', 'BINARY=ficant-server',
        '--build-arg', "FICANT_CODE_COMMIT_SHA=$candidateSha",
        '--build-arg', "FICANT_CODE_TREE_SHA=$candidateTree",
        '--tag', $images[0], '.'
    )
    New-FicantCheckStep -Name 'Build release worker image' -FilePath 'docker' -ArgumentList @(
        'build', '--pull=false', '--file', 'deploy/dev/RustService.Dockerfile',
        '--build-arg', 'BINARY=ficant-worker',
        '--build-arg', "FICANT_CODE_COMMIT_SHA=$candidateSha",
        '--build-arg', "FICANT_CODE_TREE_SHA=$candidateTree",
        '--tag', $images[1], '.'
    )
    New-FicantCheckStep -Name 'Build release UI image' -FilePath 'docker' -ArgumentList @(
        'build', '--pull=false', '--file', 'deploy/test/FicantUi.Dockerfile',
        '--tag', $images[2], '.'
    )
    New-FicantCheckStep -Name 'Pull locked Ceph RGW storage runtime' -FilePath 'docker' -ArgumentList @(
        'pull', $storageImage
    )
)
$scanSteps = @(
    @($images) + @($storageImage) | ForEach-Object {
        New-FicantCheckStep -Name "Scan release image $_" -FilePath 'trivy' -ArgumentList @(
            'image', '--scanners', 'vuln', '--severity', 'HIGH,CRITICAL',
            '--ignore-unfixed', '--skip-db-update', '--exit-code', '1', $_
        )
    }
)
$steps = @($bindingSteps) + @($buildSteps) + @($scanSteps)

function Invoke-ReleaseCompose {
    param(
        [Parameter(Mandatory)]
        [string[]]$ArgumentList
    )

    & docker compose --project-name $script:PreflightProject `
        --file (Join-Path $script:FicantRoot 'deploy\test\compose.test.yml') @ArgumentList
    if ($LASTEXITCODE -ne 0) {
        throw "Release Compose failed with exit code ${LASTEXITCODE}: $($ArgumentList -join ' ')"
    }
}

function Assert-ReleaseCandidateIdentity {
    Push-Location -LiteralPath $script:FicantRoot
    try {
        $branch = Get-FicantCommandOutput -FilePath 'git' -ArgumentList @(
            'branch', '--show-current'
        )
        $head = Get-FicantCommandOutput -FilePath 'git' -ArgumentList @(
            'rev-parse', 'HEAD'
        )
        $tree = Get-FicantCommandOutput -FilePath 'git' -ArgumentList @(
            'rev-parse', 'HEAD^{tree}'
        )
        $remoteMain = Get-FicantCommandOutput -FilePath 'git' -ArgumentList @(
            'rev-parse', 'origin/main'
        )
        $worktree = @(& git status --porcelain)
        if ($LASTEXITCODE -ne 0) {
            throw 'Unable to inspect the release-candidate worktree.'
        }
        if ($branch -ne 'main' -or $head -ne $candidateSha -or
            $tree -ne $candidateTree -or $head -ne $remoteMain -or
            $worktree.Count -ne 0) {
            throw 'Release preflight requires a clean local main exactly equal to origin/main and the frozen candidate identity.'
        }
    }
    finally {
        Pop-Location
    }
}

try {
    if ($ListOnly) {
        Show-FicantCheckPlan -Steps $steps
        $nextStep = $steps.Count + 1
        Write-Host "[$nextStep] Verify exact locked storage-runtime config and RepoDigest"
        Write-Host "[$($nextStep + 1)] Validate immutable release Compose model"
        Write-Host "[$($nextStep + 2)] Start PostgreSQL and locked Ceph RGW, apply migrations, start all application services"
        Write-Host "[$($nextStep + 3)] Verify health, readiness, UI, and forward-only migration compatibility"
        exit 0
    }

    foreach ($command in @('git', 'docker', 'python', 'trivy')) {
        Assert-FicantCommand $command
    }
    $trivyVersion = Get-FicantCommandOutput -FilePath 'trivy' -ArgumentList @('--version')
    if ($trivyVersion -notmatch '^Version:\s+0\.72\.0(?:\s|$)') {
        throw "Required Trivy version is 0.72.0, but the active output is: $trivyVersion"
    }

    Assert-ReleaseCandidateIdentity
    Invoke-FicantCheckPlan -Steps $bindingSteps
    Assert-ReleaseCandidateIdentity
    foreach ($buildStep in $buildSteps) {
        Assert-ReleaseCandidateIdentity
        Invoke-FicantCheckPlan -Steps @($buildStep)
        Assert-ReleaseCandidateIdentity
    }
    Invoke-FicantCheckPlan -Steps $scanSteps
    Assert-ReleaseCandidateIdentity
    $actualStorageConfig = (& docker image inspect --format '{{.Id}}' $storageImage).Trim()
    $storageIndexDigest = [string]$storageLock.oci.index_digest
    if ($LASTEXITCODE -ne 0 -or
        ($actualStorageConfig -ne $storageConfigDigest -and $actualStorageConfig -ne $storageIndexDigest)) {
        throw "Locked storage runtime identity mismatch: expected config $storageConfigDigest or index $storageIndexDigest, got $actualStorageConfig"
    }
    $repoDigests = @(& docker image inspect --format '{{range .RepoDigests}}{{println .}}{{end}}' $storageImage)
    if ($LASTEXITCODE -ne 0 -or $storageImage -notin $repoDigests) {
        throw "Locked storage runtime RepoDigest is missing: $storageImage"
    }

    $validationEnvironment = @{
        FICANT_DEPLOY_SHA = $candidateSha
        FICANT_STORAGE_RUNTIME_IMAGE = $storageImage
        FICANT_IMAGE_PREFIX = 'ghcr.io/kayz/ficant'
        FICANT_ROOT = '/srv/ficant-test'
        FICANT_POSTGRES_PASSWORD = 'validation-only'
        FICANT_S3_ACCESS_KEY = 'validation-access'
        FICANT_S3_SECRET_KEY = 'validation-only-secret-key-00000000'
        FICANT_S3_BUCKET = 'ficant'
        FICANT_PLATFORM_SIGNING_KEY_HEX = ('00' * 32)
        FICANT_PLATFORM_TRACE_KEY_HEX = ('00' * 32)
        FICANT_BOOTSTRAP_BEARER_TOKEN = 'validation-bootstrap-token-00000000'
        FICANT_EXPERIMENT_CURSOR_KEY_HEX = ('11' * 32)
        FICANT_WORKER_RUNTIME_IMAGE_DIGEST = "sha256:$('22' * 32)"
        FICANT_WORKER_NATIVE_SOURCE_DIGEST = "sha256:$('33' * 32)"
        FICANT_GRPC_WEB_ALLOWED_ORIGINS = 'https://greatquant.com'
    }
    $savedEnvironment = @{}
    foreach ($entry in $validationEnvironment.GetEnumerator()) {
        $savedEnvironment[$entry.Key] = [Environment]::GetEnvironmentVariable($entry.Key)
        [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value)
    }
    try {
        $resolved = & docker compose --file (Join-Path $script:FicantRoot 'deploy\test\compose.test.yml') `
            config --format json
        if ($LASTEXITCODE -ne 0) {
            throw 'Release Compose resolution failed.'
        }
        $resolved | & python (Join-Path $script:FicantRoot 'deploy\test\validate_release.py')
        if ($LASTEXITCODE -ne 0) {
            throw 'Resolved release Compose validation failed.'
        }
    }
    finally {
        foreach ($entry in $savedEnvironment.GetEnumerator()) {
            [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value)
        }
    }

    $temporaryBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
    $temporaryRoot = [IO.Path]::GetFullPath(
        (Join-Path $temporaryBase ("ficant-release-preflight-{0}" -f [guid]::NewGuid().ToString('N')))
    )
    if (-not $temporaryRoot.StartsWith($temporaryBase, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Resolved temporary root escaped the system temporary directory: $temporaryRoot"
    }
    New-Item -ItemType Directory -Path (Join-Path $temporaryRoot 'config') -Force | Out-Null
    New-Item -ItemType Directory -Path (Join-Path $temporaryRoot "releases\$candidateSha\migrations") -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $script:FicantRoot 'deploy\test\config\ficant.toml') `
        -Destination (Join-Path $temporaryRoot 'config\ficant.toml')
    Copy-Item -Path (Join-Path $script:FicantRoot 'migrations\postgresql\*.sql') `
        -Destination (Join-Path $temporaryRoot "releases\$candidateSha\migrations")

    $script:PreflightProject = "ficant-release-preflight-$([guid]::NewGuid().ToString('N').Substring(0, 12))"
    $portBase = Get-Random -Minimum 31000 -Maximum 39000
    $workerRuntimeDigest = (& docker image inspect --format '{{.Id}}' $images[1])
    if ($LASTEXITCODE -ne 0 -or $workerRuntimeDigest -notmatch '^sha256:[0-9a-f]{64}$') {
        throw 'Preflight Worker image has no canonical local digest.'
    }
    $workerSourceDigest = (& docker run --rm --read-only --cap-drop ALL `
        --security-opt no-new-privileges:true --pids-limit 64 --memory 128m `
        $images[1] --print-native-source-digest)
    if ($LASTEXITCODE -ne 0 -or $workerSourceDigest -notmatch '^sha256:[0-9a-f]{64}$') {
        throw 'Preflight Worker image has no canonical native source digest.'
    }
    $runtimeEnvironment = @{
        FICANT_DEPLOY_SHA = $candidateSha
        FICANT_STORAGE_RUNTIME_IMAGE = $storageImage
        FICANT_IMAGE_PREFIX = $imagePrefix
        FICANT_ROOT = $temporaryRoot
        FICANT_POSTGRES_PASSWORD = 'preflight-postgres-password'
        FICANT_S3_ACCESS_KEY = 'preflightaccess'
        FICANT_S3_SECRET_KEY = 'preflight-secret-key-0000000000000000'
        FICANT_S3_BUCKET = 'ficant'
        FICANT_PLATFORM_SIGNING_KEY_HEX = ('11' * 32)
        FICANT_PLATFORM_TRACE_KEY_HEX = ('22' * 32)
        FICANT_BOOTSTRAP_BEARER_TOKEN = 'preflight-bootstrap-token-00000000'
        FICANT_EXPERIMENT_CURSOR_KEY_HEX = ('33' * 32)
        FICANT_WORKER_RUNTIME_IMAGE_DIGEST = $workerRuntimeDigest.Trim()
        FICANT_WORKER_NATIVE_SOURCE_DIGEST = $workerSourceDigest.Trim()
        FICANT_GRPC_WEB_ALLOWED_ORIGINS = 'http://127.0.0.1'
        FICANT_POSTGRES_PORT = [string]$portBase
        FICANT_S3_PORT = [string]($portBase + 1)
        FICANT_SERVER_PORT = [string]($portBase + 2)
        FICANT_WORKER_PORT = [string]($portBase + 3)
        FICANT_UI_PORT = [string]($portBase + 4)
    }
    $savedRuntimeEnvironment = @{}
    foreach ($entry in $runtimeEnvironment.GetEnumerator()) {
        $savedRuntimeEnvironment[$entry.Key] = [Environment]::GetEnvironmentVariable($entry.Key)
        [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value)
    }
    try {
        Invoke-ReleaseCompose -ArgumentList @('up', '-d', '--wait', '--wait-timeout', '180', 'postgres', 'ceph-rgw')
        Invoke-ReleaseCompose -ArgumentList @('run', '--rm', 'migration')
        Invoke-ReleaseCompose -ArgumentList @(
            'up', '-d', '--remove-orphans', '--wait', '--wait-timeout', '180',
            'ficant-server', 'ficant-worker', 'ficant-ui'
        )

        $worker = Invoke-WebRequest -UseBasicParsing -Uri "http://127.0.0.1:$($portBase + 3)/worker-ready"
        $ui = Invoke-WebRequest -UseBasicParsing -Uri "http://127.0.0.1:$($portBase + 4)/ficant/"
        if ($worker.Content.Trim() -ne 'ok' -or $ui.Content -notlike '*<div id="root">*') {
            throw 'Release readiness or UI smoke failed.'
        }
        $requiredMigrations = @(
            Get-ChildItem -LiteralPath (Join-Path $temporaryRoot "releases\$candidateSha\migrations") `
                -Filter '*.sql' -File | Select-Object -ExpandProperty Name
        )
        $appliedOutput = & docker compose --project-name $script:PreflightProject `
            --file (Join-Path $script:FicantRoot 'deploy\test\compose.test.yml') `
            exec -T postgres psql -U ficant -d ficant -At `
            -c 'SELECT version FROM public.ficant_schema_migrations ORDER BY version'
        if ($LASTEXITCODE -ne 0) {
            throw 'Unable to inspect applied migrations.'
        }
        $appliedMigrations = @($appliedOutput | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
        $missingMigrations = @($requiredMigrations | Where-Object { $_ -notin $appliedMigrations })
        if ($missingMigrations.Count -ne 0) {
            throw "Release topology missed required migrations: $($missingMigrations -join ', ')"
        }
    }
    finally {
        try {
            Invoke-ReleaseCompose -ArgumentList @('down', '--volumes', '--remove-orphans')
        }
        catch {
            Write-Warning $_
        }
        foreach ($entry in $savedRuntimeEnvironment.GetEnumerator()) {
            [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value)
        }
        if (Test-Path -LiteralPath $temporaryRoot) {
            Remove-Item -LiteralPath $temporaryRoot -Recurse -Force
        }
    }

    Assert-ReleaseCandidateIdentity

    Write-Host ''
    Write-Host 'FICANT release-candidate preflight passed.'
    exit 0
}
catch {
    Write-Error $_
    exit 1
}
