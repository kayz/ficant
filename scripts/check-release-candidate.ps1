[CmdletBinding()]
param(
    [switch]$ListOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'check-common.ps1')

$candidateSha = '0000000000000000000000000000000000000000'
$imagePrefix = 'ficant-preflight/ficant'
$images = @(
    "$imagePrefix-server:sha-$candidateSha"
    "$imagePrefix-worker:sha-$candidateSha"
    "$imagePrefix-web:sha-$candidateSha"
    "$imagePrefix-ui:sha-$candidateSha"
    "$imagePrefix-ceph-rgw:sha-$candidateSha"
)
$buildSteps = @(
    New-FicantCheckStep -Name 'Build release server image' -FilePath 'docker' -ArgumentList @(
        'build', '--pull=false', '--file', 'deploy/dev/RustService.Dockerfile',
        '--build-arg', 'BINARY=ficant-server', '--tag', $images[0], '.'
    )
    New-FicantCheckStep -Name 'Build release worker image' -FilePath 'docker' -ArgumentList @(
        'build', '--pull=false', '--file', 'deploy/dev/RustService.Dockerfile',
        '--build-arg', 'BINARY=ficant-worker', '--tag', $images[1], '.'
    )
    New-FicantCheckStep -Name 'Build release web image' -FilePath 'docker' -ArgumentList @(
        'build', '--pull=false', '--file', 'deploy/dev/RustService.Dockerfile',
        '--build-arg', 'BINARY=ficant-web', '--tag', $images[2], '.'
    )
    New-FicantCheckStep -Name 'Build release UI image' -FilePath 'docker' -ArgumentList @(
        'build', '--pull=false', '--file', 'deploy/test/FicantUi.Dockerfile',
        '--tag', $images[3], '.'
    )
    New-FicantCheckStep -Name 'Build release Ceph RGW image' -FilePath 'docker' -ArgumentList @(
        'build', '--pull=false', '--file', 'deploy/dev/Ceph.Dockerfile',
        '--tag', $images[4], '.'
    )
)
$scanSteps = @(
    $images | ForEach-Object {
        New-FicantCheckStep -Name "Scan release image $_" -FilePath 'trivy' -ArgumentList @(
            'image', '--scanners', 'vuln', '--severity', 'HIGH,CRITICAL',
            '--ignore-unfixed', '--skip-db-update', '--exit-code', '1', $_
        )
    }
)
$steps = @($buildSteps) + @($scanSteps)

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

try {
    if ($ListOnly) {
        Show-FicantCheckPlan -Steps $steps
        Write-Host '[11] Validate immutable release Compose model'
        Write-Host '[12] Start PostgreSQL and Ceph RGW, apply migrations, start all application services'
        Write-Host '[13] Verify health, readiness, UI, and forward-only migration compatibility'
        exit 0
    }

    foreach ($command in @('git', 'docker', 'python', 'trivy')) {
        Assert-FicantCommand $command
    }
    $trivyVersion = Get-FicantCommandOutput -FilePath 'trivy' -ArgumentList @('--version')
    if ($trivyVersion -notmatch '(?m)^Version:\s+0\.72\.0$') {
        throw "Required Trivy version is 0.72.0, but the active output is: $trivyVersion"
    }

    Push-Location -LiteralPath $script:FicantRoot
    try {
        $branch = (& git branch --show-current).Trim()
        $head = (& git rev-parse HEAD).Trim()
        $remoteMain = (& git rev-parse origin/main).Trim()
        $worktree = @(& git status --porcelain)
        if ($branch -ne 'main' -or $head -ne $remoteMain -or $worktree.Count -ne 0) {
            throw 'Release preflight requires a clean local main exactly equal to origin/main.'
        }
    }
    finally {
        Pop-Location
    }

    Invoke-FicantCheckPlan -Steps $steps

    $validationEnvironment = @{
        FICANT_DEPLOY_SHA = $candidateSha
        FICANT_STORAGE_SHA = $candidateSha
        FICANT_IMAGE_PREFIX = 'ghcr.io/kayz/ficant'
        FICANT_ROOT = '/srv/ficant-test'
        FICANT_POSTGRES_PASSWORD = 'validation-only'
        FICANT_S3_ACCESS_KEY = 'validation-access'
        FICANT_S3_SECRET_KEY = 'validation-only-secret-key-00000000'
        FICANT_S3_BUCKET = 'ficant'
        FICANT_PLATFORM_SIGNING_KEY_HEX = ('00' * 32)
        FICANT_PLATFORM_TRACE_KEY_HEX = ('00' * 32)
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
    $runtimeEnvironment = @{
        FICANT_DEPLOY_SHA = $candidateSha
        FICANT_STORAGE_SHA = $candidateSha
        FICANT_IMAGE_PREFIX = $imagePrefix
        FICANT_ROOT = $temporaryRoot
        FICANT_POSTGRES_PASSWORD = 'preflight-postgres-password'
        FICANT_S3_ACCESS_KEY = 'preflightaccess'
        FICANT_S3_SECRET_KEY = 'preflight-secret-key-0000000000000000'
        FICANT_S3_BUCKET = 'ficant'
        FICANT_PLATFORM_SIGNING_KEY_HEX = ('11' * 32)
        FICANT_PLATFORM_TRACE_KEY_HEX = ('22' * 32)
        FICANT_GRPC_WEB_ALLOWED_ORIGINS = 'http://127.0.0.1'
        FICANT_POSTGRES_PORT = [string]$portBase
        FICANT_S3_PORT = [string]($portBase + 1)
        FICANT_SERVER_PORT = [string]($portBase + 2)
        FICANT_WORKER_PORT = [string]($portBase + 3)
        FICANT_WEB_PORT = [string]($portBase + 4)
        FICANT_UI_PORT = [string]($portBase + 5)
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
            'ficant-server', 'ficant-worker', 'ficant-web', 'ficant-ui'
        )

        $worker = Invoke-WebRequest -UseBasicParsing -Uri "http://127.0.0.1:$($portBase + 3)/worker-ready"
        $web = Invoke-WebRequest -UseBasicParsing -Uri "http://127.0.0.1:$($portBase + 4)/web-ready"
        $ui = Invoke-WebRequest -UseBasicParsing -Uri "http://127.0.0.1:$($portBase + 5)/ficant/"
        if ($worker.Content.Trim() -ne 'ok' -or $web.Content.Trim() -ne 'ok' -or
            $ui.Content -notlike '*<div id="root">*') {
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

    Write-Host ''
    Write-Host 'FICANT release-candidate preflight passed.'
    exit 0
}
catch {
    Write-Error $_
    exit 1
}
