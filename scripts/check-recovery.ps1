[CmdletBinding(DefaultParameterSetName = 'Run')]
param(
    [Parameter(ParameterSetName = 'Run')]
    [switch]$ListOnly,

    [Parameter(ParameterSetName = 'Run')]
    [ValidatePattern('^$|^sha256:[0-9a-f]{64}$')]
    [string]$RuntimeImageDigest = '',

    [Parameter(Mandatory, ParameterSetName = 'Validate')]
    [string]$ValidateManifest,

    [Parameter(Mandatory, ParameterSetName = 'Validate')]
    [ValidatePattern('^[0-9a-f]{40}$')]
    [string]$ExpectedCodeCommit,

    [Parameter(Mandatory, ParameterSetName = 'Validate')]
    [ValidatePattern('^[0-9a-f]{40}$')]
    [string]$ExpectedCodeTree,

    [Parameter(Mandatory, ParameterSetName = 'Validate')]
    [ValidatePattern('^sha256:[0-9a-f]{64}$')]
    [string]$ExpectedRuntimeImageDigest
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Invoke-Native {
    param(
        [Parameter(Mandatory)]
        [string]$FilePath,

        [Parameter(Mandatory)]
        [string[]]$ArgumentList,

        [string]$WorkingDirectory = ''
    )

    $previous = [Environment]::CurrentDirectory
    try {
        if (-not [string]::IsNullOrWhiteSpace($WorkingDirectory)) {
            [Environment]::CurrentDirectory = $WorkingDirectory
            Push-Location -LiteralPath $WorkingDirectory
        }
        & $FilePath @ArgumentList
        if ($LASTEXITCODE -ne 0) {
            throw "$FilePath failed with exit code $LASTEXITCODE."
        }
    }
    finally {
        if (-not [string]::IsNullOrWhiteSpace($WorkingDirectory)) {
            Pop-Location
            [Environment]::CurrentDirectory = $previous
        }
    }
}

function Get-NativeOutput {
    param(
        [Parameter(Mandatory)]
        [string]$FilePath,

        [Parameter(Mandatory)]
        [string[]]$ArgumentList
    )

    $output = @(& $FilePath @ArgumentList 2>&1)
    if ($LASTEXITCODE -ne 0) {
        throw "$FilePath failed with exit code $LASTEXITCODE."
    }
    return ($output -join "`n").Trim()
}

function Get-FreeTcpPort {
    $listener = [System.Net.Sockets.TcpListener]::new(
        [System.Net.IPAddress]::Loopback,
        0
    )
    try {
        $listener.Start()
        return ([System.Net.IPEndPoint]$listener.LocalEndpoint).Port
    }
    finally {
        $listener.Stop()
    }
}

function Resolve-BoundFile {
    param(
        [Parameter(Mandatory)]
        [string]$Root,

        [Parameter(Mandatory)]
        [string]$RelativePath
    )

    if ([System.IO.Path]::IsPathRooted($RelativePath) -or
        $RelativePath.Contains('\') -or
        $RelativePath -match '(^|/)\.\.(/|$)') {
        throw "Recovery manifest path is not canonical: '$RelativePath'."
    }
    $resolvedRoot = [System.IO.Path]::GetFullPath($Root)
    $resolved = [System.IO.Path]::GetFullPath((Join-Path $resolvedRoot $RelativePath))
    $prefix = $resolvedRoot.TrimEnd(
        [System.IO.Path]::DirectorySeparatorChar,
        [System.IO.Path]::AltDirectorySeparatorChar
    ) + [System.IO.Path]::DirectorySeparatorChar
    if (-not $resolved.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Recovery manifest path escapes the backup root: '$RelativePath'."
    }
    return $resolved
}

function Assert-ExactProperties {
    param(
        [Parameter(Mandatory)]
        [object]$Value,

        [Parameter(Mandatory)]
        [string[]]$Expected,

        [Parameter(Mandatory)]
        [string]$Label
    )

    $actual = @($Value.PSObject.Properties.Name | Sort-Object -CaseSensitive)
    $wanted = @($Expected | Sort-Object -CaseSensitive)
    if (($actual -join "`n") -cne ($wanted -join "`n")) {
        throw "$Label properties differ from the frozen schema."
    }
}

function Assert-RecoveryManifest {
    param(
        [Parameter(Mandatory)]
        [string]$ManifestPath,

        [Parameter(Mandatory)]
        [string]$CodeCommit,

        [Parameter(Mandatory)]
        [string]$CodeTree,

        [Parameter(Mandatory)]
        [string]$RuntimeDigest
    )

    $resolvedManifest = [System.IO.Path]::GetFullPath($ManifestPath)
    if (-not (Test-Path -LiteralPath $resolvedManifest -PathType Leaf)) {
        throw "Recovery manifest is missing: '$resolvedManifest'."
    }
    $root = Split-Path -Parent $resolvedManifest
    $manifest = Get-Content -Raw -LiteralPath $resolvedManifest | ConvertFrom-Json
    Assert-ExactProperties -Value $manifest -Expected @(
        'schema', 'code', 'runtime', 'postgres', 'immutable_objects', 'proofs'
    ) -Label 'Recovery manifest'
    Assert-ExactProperties -Value $manifest.code -Expected @(
        'git_commit_sha', 'git_tree_sha'
    ) -Label 'Recovery Code binding'
    Assert-ExactProperties -Value $manifest.runtime -Expected @(
        'image_config_digest'
    ) -Label 'Recovery Runtime binding'
    Assert-ExactProperties -Value $manifest.postgres -Expected @(
        'file', 'size', 'sha256'
    ) -Label 'Recovery PostgreSQL binding'
    Assert-ExactProperties -Value $manifest.proofs -Expected @(
        'graph_artifact_id', 'graph_output_identity', 'analytics_output_identity'
    ) -Label 'Recovery proof identities'

    if ($manifest.schema -cne 'ficant.recovery.bundle.v1') {
        throw "Unsupported recovery manifest schema '$($manifest.schema)'."
    }
    if ([string]$manifest.code.git_commit_sha -cne $CodeCommit -or
        [string]$manifest.code.git_tree_sha -cne $CodeTree) {
        throw 'Recovery manifest Code identity drifted.'
    }
    if ([string]$manifest.runtime.image_config_digest -cne $RuntimeDigest) {
        throw 'Recovery manifest Runtime image identity drifted.'
    }
    if ([string]$manifest.postgres.file -cne 'database.dump' -or
        [string]$manifest.postgres.sha256 -notmatch '^[0-9a-f]{64}$') {
        throw 'Recovery PostgreSQL binding is not canonical.'
    }
    $databasePath = Resolve-BoundFile -Root $root -RelativePath ([string]$manifest.postgres.file)
    if (-not (Test-Path -LiteralPath $databasePath -PathType Leaf)) {
        throw 'Recovery PostgreSQL dump is missing.'
    }
    $database = Get-Item -LiteralPath $databasePath
    $databaseHash = (Get-FileHash -LiteralPath $databasePath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ([long]$manifest.postgres.size -ne $database.Length -or
        [string]$manifest.postgres.sha256 -cne $databaseHash) {
        throw 'Recovery PostgreSQL dump size or hash drifted.'
    }

    $objects = @($manifest.immutable_objects)
    if ($objects.Count -eq 0) {
        throw 'Recovery manifest must bind at least one immutable object.'
    }
    $seenKeys = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    $seenFiles = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    $previousKey = ''
    foreach ($object in $objects) {
        Assert-ExactProperties -Value $object -Expected @(
            'key', 'file', 'size', 'sha256'
        ) -Label 'Immutable object binding'
        $key = [string]$object.key
        $file = [string]$object.file
        $hash = [string]$object.sha256
        if ($key -notmatch '^immutable/([0-9a-f]{64})$' -or
            $file -notmatch '^objects/([0-9a-f]{64})\.blob$' -or
            $hash -notmatch '^[0-9a-f]{64}$' -or
            $key.Substring('immutable/'.Length) -cne $hash -or
            [System.IO.Path]::GetFileNameWithoutExtension($file) -cne $hash) {
            throw "Immutable object binding is not content addressed: '$key'."
        }
        if (($previousKey.Length -gt 0 -and
            [string]::CompareOrdinal($previousKey, $key) -ge 0) -or
            -not $seenKeys.Add($key) -or
            -not $seenFiles.Add($file)) {
            throw 'Recovery immutable objects must be unique and strictly sorted by key.'
        }
        $previousKey = $key
        $objectPath = Resolve-BoundFile -Root $root -RelativePath $file
        if (-not (Test-Path -LiteralPath $objectPath -PathType Leaf)) {
            throw "Recovery immutable object is missing: '$file'."
        }
        $item = Get-Item -LiteralPath $objectPath
        $actualHash = (Get-FileHash -LiteralPath $objectPath -Algorithm SHA256).Hash.ToLowerInvariant()
        if ([long]$object.size -ne $item.Length -or $actualHash -cne $hash) {
            throw "Recovery immutable object size or hash drifted: '$file'."
        }
    }
    $objectRoot = Join-Path $root 'objects'
    $actualFiles = if (Test-Path -LiteralPath $objectRoot -PathType Container) {
        @(Get-ChildItem -LiteralPath $objectRoot -File -Recurse | ForEach-Object {
            'objects/' + [System.IO.Path]::GetRelativePath($objectRoot, $_.FullName).Replace('\', '/')
        } | Sort-Object -CaseSensitive)
    }
    else {
        @()
    }
    $expectedFiles = @($seenFiles | Sort-Object -CaseSensitive)
    if (($actualFiles -join "`n") -cne ($expectedFiles -join "`n")) {
        throw 'Recovery object directory contains a missing or extra object.'
    }
    if ([string]$manifest.proofs.graph_artifact_id -notmatch '^[0-9A-HJKMNP-TV-Z]{26}$' -or
        [string]$manifest.proofs.graph_output_identity -notmatch '^[0-9a-f]{64}$' -or
        [string]$manifest.proofs.analytics_output_identity -notmatch '^[0-9a-f]{64}$') {
        throw 'Recovery proof identities are malformed.'
    }
    return (Get-FileHash -LiteralPath $resolvedManifest -Algorithm SHA256).Hash
}

function Invoke-RecoveryCompose {
    param(
        [Parameter(Mandatory)]
        [string]$ComposeFile,

        [Parameter(Mandatory)]
        [string]$Project,

        [Parameter(Mandatory)]
        [string[]]$Arguments
    )

    Invoke-Native -FilePath 'docker' -ArgumentList @(
        'compose', '--project-directory', (Split-Path -Parent $ComposeFile),
        '--file', $ComposeFile, '--project-name', $Project
    ) + $Arguments
}

function Stop-RecoveryProject {
    param(
        [Parameter(Mandatory)]
        [string]$ComposeFile,

        [Parameter(Mandatory)]
        [string]$Project
    )

    if ($Project -notmatch '^ficant-r7b-(source|restore)-[0-9a-f]{8}$') {
        throw "Refusing to remove unexpected Compose project '$Project'."
    }
    & docker compose `
        --project-directory (Split-Path -Parent $ComposeFile) `
        --file $ComposeFile `
        --project-name $Project `
        down --volumes --remove-orphans
    if ($LASTEXITCODE -ne 0) {
        throw "Unable to remove recovery Compose project '$Project'."
    }
}

function Assert-RecoveryProjectDestroyed {
    param([Parameter(Mandatory)][string]$Project)

    $containers = Get-NativeOutput -FilePath 'docker' -ArgumentList @(
        'ps', '--all', '--quiet', '--filter', "label=com.docker.compose.project=$Project"
    )
    $volumes = Get-NativeOutput -FilePath 'docker' -ArgumentList @(
        'volume', 'ls', '--quiet', '--filter', "label=com.docker.compose.project=$Project"
    )
    if (-not [string]::IsNullOrWhiteSpace($containers) -or
        -not [string]::IsNullOrWhiteSpace($volumes)) {
        throw "Recovery source state was not fully destroyed for '$Project'."
    }
}

function Set-RecoveryTestEnvironment {
    param(
        [Parameter(Mandatory)]
        [int]$PostgresPort,

        [Parameter(Mandatory)]
        [int]$S3Port,

        [Parameter(Mandatory)]
        [string]$CodeCommit,

        [Parameter(Mandatory)]
        [string]$CodeTree,

        [Parameter(Mandatory)]
        [string]$RuntimeDigest,

        [Parameter(Mandatory)]
        [string]$BackupRoot
    )

    $env:FICANT_TEST_DATABASE_URL = (
        'postgres://ficant:{0}@127.0.0.1:{1}/ficant' -f
        $env:FICANT_RECOVERY_POSTGRES_PASSWORD,
        $PostgresPort
    )
    $env:FICANT_TEST_S3_ENDPOINT = "http://127.0.0.1:$S3Port"
    $env:FICANT_TEST_S3_BUCKET = $env:FICANT_RECOVERY_S3_BUCKET
    $env:FICANT_TEST_S3_ACCESS_KEY = $env:FICANT_RECOVERY_S3_ACCESS_KEY
    $env:FICANT_TEST_S3_SECRET_KEY = $env:FICANT_RECOVERY_S3_SECRET_KEY
    $env:FICANT_RECOVERY_CODE_COMMIT_SHA = $CodeCommit
    $env:FICANT_RECOVERY_CODE_TREE_SHA = $CodeTree
    $env:FICANT_RECOVERY_RUNTIME_IMAGE_DIGEST = $RuntimeDigest
    $env:FICANT_RECOVERY_BACKUP_ROOT = $BackupRoot
}

function Invoke-RecoveryPhase {
    param(
        [Parameter(Mandatory)]
        [string]$Phase,

        [Parameter(Mandatory)]
        [string]$RepositoryRoot
    )

    $env:FICANT_RECOVERY_PHASE = $Phase
    Invoke-Native -FilePath 'cargo' -ArgumentList @(
        'test', '--offline', '--locked', '-p', 'ficant-storage',
        '--test', 'r7b_backup_restore_sit', '--', '--test-threads=1'
    ) -WorkingDirectory $RepositoryRoot
}

function New-RecoveryManifest {
    param(
        [Parameter(Mandatory)]
        [string]$BackupRoot,

        [Parameter(Mandatory)]
        [string]$CodeCommit,

        [Parameter(Mandatory)]
        [string]$CodeTree,

        [Parameter(Mandatory)]
        [string]$RuntimeDigest
    )

    $objects = foreach ($line in Get-Content -LiteralPath (Join-Path $BackupRoot 'objects.tsv')) {
        $fields = $line -split "`t"
        if ($fields.Count -ne 4) {
            throw 'Recovery object index is malformed.'
        }
        [ordered]@{
            key = $fields[0]
            file = $fields[1]
            size = [long]$fields[2]
            sha256 = $fields[3]
        }
    }
    $proofs = @{}
    foreach ($line in Get-Content -LiteralPath (Join-Path $BackupRoot 'proofs.tsv')) {
        $fields = $line -split "`t"
        if ($fields.Count -ne 2 -or $proofs.ContainsKey($fields[0])) {
            throw 'Recovery proof index is malformed.'
        }
        $proofs[$fields[0]] = $fields[1]
    }
    $databasePath = Join-Path $BackupRoot 'database.dump'
    $database = Get-Item -LiteralPath $databasePath
    $manifest = [ordered]@{
        schema = 'ficant.recovery.bundle.v1'
        code = [ordered]@{
            git_commit_sha = $CodeCommit
            git_tree_sha = $CodeTree
        }
        runtime = [ordered]@{
            image_config_digest = $RuntimeDigest
        }
        postgres = [ordered]@{
            file = 'database.dump'
            size = $database.Length
            sha256 = (Get-FileHash -LiteralPath $databasePath -Algorithm SHA256).Hash.ToLowerInvariant()
        }
        immutable_objects = @($objects | Sort-Object { $_.key } -CaseSensitive)
        proofs = [ordered]@{
            graph_artifact_id = [string]$proofs['graph_artifact_id']
            graph_output_identity = [string]$proofs['graph_output_identity']
            analytics_output_identity = [string]$proofs['analytics_output_identity']
        }
    }
    $manifestPath = Join-Path $BackupRoot 'backup-manifest.json'
    [System.IO.File]::WriteAllText(
        $manifestPath,
        ($manifest | ConvertTo-Json -Depth 8) + "`n",
        [System.Text.UTF8Encoding]::new($false)
    )
    return $manifestPath
}

if ($PSCmdlet.ParameterSetName -ceq 'Validate') {
    try {
        $digest = Assert-RecoveryManifest `
            -ManifestPath $ValidateManifest `
            -CodeCommit $ExpectedCodeCommit `
            -CodeTree $ExpectedCodeTree `
            -RuntimeDigest $ExpectedRuntimeImageDigest
        Write-Output "Recovery manifest verified: $digest"
        exit 0
    }
    catch {
        Write-Error $_
        exit 1
    }
}

$repositoryRoot = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$composeFile = Join-Path $repositoryRoot 'tests\recovery\docker-compose.yml'
if ($ListOnly) {
    Write-Output 'R7B isolated recovery proof:'
    Write-Output '  1. require a clean exact public Git commit/tree and canonical Runtime image digest'
    Write-Output '  2. start isolated source PostgreSQL 16 + Ceph RGW'
    Write-Output '  3. seed Graph Artifact and synchronous Analytics formal output'
    Write-Output '  4. create PG custom dump and complete immutable-object manifest'
    Write-Output '  5. destroy source containers, database volume, Ceph volume, and bucket'
    Write-Output '  6. start a distinct fresh restore project and restore DB + exact object set'
    Write-Output '  7. required-read both outputs and compare bytes/evidence/identity'
    exit 0
}

$scratchRoot = $null
$sourceProject = ''
$restoreProject = ''
$sourceStarted = $false
$restoreStarted = $false
$savedEnvironment = @{}
try {
    foreach ($command in @('git', 'docker', 'cargo')) {
        if ($null -eq (Get-Command $command -ErrorAction SilentlyContinue)) {
            throw "Required command '$command' was not found."
        }
    }
    $commit = Get-NativeOutput -FilePath 'git' -ArgumentList @(
        '-C', $repositoryRoot, 'rev-parse', 'HEAD'
    )
    $tree = Get-NativeOutput -FilePath 'git' -ArgumentList @(
        '-C', $repositoryRoot, 'rev-parse', 'HEAD^{tree}'
    )
    if ($commit -notmatch '^[0-9a-f]{40}$' -or $tree -notmatch '^[0-9a-f]{40}$') {
        throw 'Public Git Code identity is not canonical.'
    }
    $status = Get-NativeOutput -FilePath 'git' -ArgumentList @(
        '-C', $repositoryRoot, 'status', '--porcelain', '--untracked-files=all'
    )
    if (-not [string]::IsNullOrWhiteSpace($status)) {
        throw 'Recovery proof requires a clean public worktree.'
    }
    if ([string]::IsNullOrWhiteSpace($RuntimeImageDigest)) {
        $RuntimeImageDigest = [string]$env:FICANT_TEST_RUNTIME_IMAGE_DIGEST
    }
    if ([string]::IsNullOrWhiteSpace($RuntimeImageDigest)) {
        $RuntimeImageDigest = Get-NativeOutput -FilePath 'docker' -ArgumentList @(
            'image', 'inspect', '--format', '{{.Id}}', 'ficant/worker:dev'
        )
    }
    if ($RuntimeImageDigest -notmatch '^sha256:[0-9a-f]{64}$') {
        throw 'Recovery proof requires a canonical Worker image config digest.'
    }

    $scratchRoot = Join-Path (
        [System.IO.Path]::GetTempPath()
    ) ("ficant-recovery-{0}" -f [Guid]::NewGuid().ToString('N'))
    $backupRoot = Join-Path $scratchRoot 'backup'
    New-Item -ItemType Directory -Path $backupRoot | Out-Null
    $token = [Guid]::NewGuid().ToString('N').Substring(0, 8)
    $sourceProject = "ficant-r7b-source-$token"
    $restoreProject = "ficant-r7b-restore-$token"
    $sourcePostgresPort = Get-FreeTcpPort
    $sourceS3Port = Get-FreeTcpPort
    $restorePostgresPort = Get-FreeTcpPort
    $restoreS3Port = Get-FreeTcpPort

    $environmentNames = @(
        'FICANT_RECOVERY_ARTIFACT_ROOT', 'FICANT_RECOVERY_POSTGRES_PASSWORD',
        'FICANT_RECOVERY_POSTGRES_PORT', 'FICANT_RECOVERY_S3_ACCESS_KEY',
        'FICANT_RECOVERY_S3_SECRET_KEY', 'FICANT_RECOVERY_S3_BUCKET',
        'FICANT_RECOVERY_S3_PORT', 'FICANT_TEST_DATABASE_URL',
        'FICANT_TEST_S3_ENDPOINT', 'FICANT_TEST_S3_BUCKET',
        'FICANT_TEST_S3_ACCESS_KEY', 'FICANT_TEST_S3_SECRET_KEY',
        'FICANT_RECOVERY_CODE_COMMIT_SHA', 'FICANT_RECOVERY_CODE_TREE_SHA',
        'FICANT_RECOVERY_RUNTIME_IMAGE_DIGEST', 'FICANT_RECOVERY_BACKUP_ROOT',
        'FICANT_RECOVERY_PHASE'
    )
    foreach ($name in $environmentNames) {
        $savedEnvironment[$name] = [Environment]::GetEnvironmentVariable($name)
    }
    $env:FICANT_RECOVERY_ARTIFACT_ROOT = $backupRoot
    $env:FICANT_RECOVERY_POSTGRES_PASSWORD = "r7bpg$token"
    $env:FICANT_RECOVERY_S3_ACCESS_KEY = "R7BACCESS$($token.ToUpperInvariant())"
    $env:FICANT_RECOVERY_S3_SECRET_KEY = "r7b-secret-$token-recovery-only"
    $env:FICANT_RECOVERY_S3_BUCKET = "ficant-r7b-$token"
    $env:FICANT_RECOVERY_POSTGRES_PORT = [string]$sourcePostgresPort
    $env:FICANT_RECOVERY_S3_PORT = [string]$sourceS3Port

    Invoke-RecoveryCompose -ComposeFile $composeFile -Project $sourceProject `
        -Arguments @('up', '--detach', '--build', '--wait')
    $sourceStarted = $true
    Set-RecoveryTestEnvironment `
        -PostgresPort $sourcePostgresPort `
        -S3Port $sourceS3Port `
        -CodeCommit $commit `
        -CodeTree $tree `
        -RuntimeDigest $RuntimeImageDigest `
        -BackupRoot $backupRoot
    Invoke-RecoveryPhase -Phase 'seed' -RepositoryRoot $repositoryRoot
    Invoke-RecoveryPhase -Phase 'export-objects' -RepositoryRoot $repositoryRoot
    Invoke-RecoveryCompose -ComposeFile $composeFile -Project $sourceProject -Arguments @(
        'exec', '-T', 'postgres', 'pg_dump', '--username=ficant', '--dbname=ficant',
        '--format=custom', '--no-owner', '--no-privileges', '--file=/backup/database.dump'
    )
    $manifestPath = New-RecoveryManifest `
        -BackupRoot $backupRoot `
        -CodeCommit $commit `
        -CodeTree $tree `
        -RuntimeDigest $RuntimeImageDigest
    $manifestDigest = Assert-RecoveryManifest `
        -ManifestPath $manifestPath `
        -CodeCommit $commit `
        -CodeTree $tree `
        -RuntimeDigest $RuntimeImageDigest

    Stop-RecoveryProject -ComposeFile $composeFile -Project $sourceProject
    $sourceStarted = $false
    Assert-RecoveryProjectDestroyed -Project $sourceProject

    $env:FICANT_RECOVERY_POSTGRES_PORT = [string]$restorePostgresPort
    $env:FICANT_RECOVERY_S3_PORT = [string]$restoreS3Port
    Invoke-RecoveryCompose -ComposeFile $composeFile -Project $restoreProject `
        -Arguments @('up', '--detach', '--build', '--wait')
    $restoreStarted = $true
    Invoke-RecoveryCompose -ComposeFile $composeFile -Project $restoreProject -Arguments @(
        'exec', '-T', 'postgres', 'pg_restore', '--username=ficant', '--dbname=ficant',
        '--exit-on-error', '--no-owner', '--no-privileges', '/backup/database.dump'
    )
    Set-RecoveryTestEnvironment `
        -PostgresPort $restorePostgresPort `
        -S3Port $restoreS3Port `
        -CodeCommit $commit `
        -CodeTree $tree `
        -RuntimeDigest $RuntimeImageDigest `
        -BackupRoot $backupRoot
    Invoke-RecoveryPhase -Phase 'restore-objects' -RepositoryRoot $repositoryRoot
    Invoke-RecoveryPhase -Phase 'verify' -RepositoryRoot $repositoryRoot
    Write-Output "R7B recovery proof passed. Manifest SHA-256: $manifestDigest"
    exit 0
}
catch {
    Write-Error $_
    exit 1
}
finally {
    if ($restoreStarted) {
        Stop-RecoveryProject -ComposeFile $composeFile -Project $restoreProject
    }
    if ($sourceStarted) {
        Stop-RecoveryProject -ComposeFile $composeFile -Project $sourceProject
    }
    foreach ($name in $savedEnvironment.Keys) {
        [Environment]::SetEnvironmentVariable($name, $savedEnvironment[$name])
    }
    if ($null -ne $scratchRoot -and (Test-Path -LiteralPath $scratchRoot)) {
        $resolvedScratch = [System.IO.Path]::GetFullPath($scratchRoot)
        $tempPrefix = [System.IO.Path]::GetFullPath(
            [System.IO.Path]::GetTempPath()
        ).TrimEnd([System.IO.Path]::DirectorySeparatorChar) +
            [System.IO.Path]::DirectorySeparatorChar
        if (-not $resolvedScratch.StartsWith(
            $tempPrefix,
            [System.StringComparison]::OrdinalIgnoreCase
        ) -or [System.IO.Path]::GetFileName($resolvedScratch) -notlike 'ficant-recovery-*') {
            throw "Refusing to remove unexpected recovery scratch '$resolvedScratch'."
        }
        Remove-Item -LiteralPath $resolvedScratch -Recurse -Force
    }
}
