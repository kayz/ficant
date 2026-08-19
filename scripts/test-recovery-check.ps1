[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$checker = Join-Path $PSScriptRoot 'check-recovery.ps1'
$scratchRoot = Join-Path (
    [System.IO.Path]::GetTempPath()
) ("ficant-recovery-check-{0}" -f [Guid]::NewGuid().ToString('N'))
$codeCommit = '1111111111111111111111111111111111111111'
$codeTree = '2222222222222222222222222222222222222222'
$runtimeDigest = 'sha256:' + ('3' * 64)

function Get-LowerSha256 {
    param([Parameter(Mandatory)][string]$LiteralPath)

    return (Get-FileHash -LiteralPath $LiteralPath -Algorithm SHA256).Hash.ToLowerInvariant()
}

function New-ManifestFixture {
    param([Parameter(Mandatory)][string]$Name)

    $root = Join-Path $scratchRoot $Name
    $objectsRoot = Join-Path $root 'objects'
    New-Item -ItemType Directory -Path $objectsRoot -Force | Out-Null
    [System.IO.File]::WriteAllBytes(
        (Join-Path $root 'database.dump'),
        [System.Text.Encoding]::UTF8.GetBytes('fixture-postgres-dump')
    )
    $objects = foreach ($bytes in @(
        [System.Text.Encoding]::UTF8.GetBytes('fixture-object-a'),
        [System.Text.Encoding]::UTF8.GetBytes('fixture-object-b')
    )) {
        $temporary = Join-Path $root ([Guid]::NewGuid().ToString('N') + '.tmp')
        [System.IO.File]::WriteAllBytes($temporary, $bytes)
        $hash = Get-LowerSha256 -LiteralPath $temporary
        $relative = "objects/$hash.blob"
        Move-Item -LiteralPath $temporary -Destination (Join-Path $root $relative)
        [ordered]@{
            key = "immutable/$hash"
            file = $relative
            size = [long]$bytes.Length
            sha256 = $hash
        }
    }
    $database = Get-Item -LiteralPath (Join-Path $root 'database.dump')
    $manifest = [ordered]@{
        schema = 'ficant.recovery.bundle.v1'
        code = [ordered]@{
            git_commit_sha = $codeCommit
            git_tree_sha = $codeTree
        }
        runtime = [ordered]@{
            image_config_digest = $runtimeDigest
        }
        postgres = [ordered]@{
            file = 'database.dump'
            size = $database.Length
            sha256 = Get-LowerSha256 -LiteralPath $database.FullName
        }
        immutable_objects = @($objects | Sort-Object { $_.key } -CaseSensitive)
        proofs = [ordered]@{
            graph_artifact_id = '01ARZ3NDEKTSV4RRFFQ69G5R04'
            graph_output_identity = '4' * 64
            analytics_output_identity = '5' * 64
        }
    }
    $manifestPath = Join-Path $root 'backup-manifest.json'
    [System.IO.File]::WriteAllText(
        $manifestPath,
        ($manifest | ConvertTo-Json -Depth 8) + "`n",
        [System.Text.UTF8Encoding]::new($false)
    )
    return $manifestPath
}

function Copy-ManifestFixture {
    param(
        [Parameter(Mandatory)][string]$SourceManifest,
        [Parameter(Mandatory)][string]$Name
    )

    $sourceRoot = Split-Path -Parent $SourceManifest
    $destination = Join-Path $scratchRoot $Name
    Copy-Item -LiteralPath $sourceRoot -Destination $destination -Recurse
    return Join-Path $destination 'backup-manifest.json'
}

function Invoke-Checker {
    param(
        [Parameter(Mandatory)][string]$Manifest,
        [string]$ExpectedCommit = $codeCommit,
        [string]$ExpectedTree = $codeTree,
        [string]$ExpectedRuntime = $runtimeDigest
    )

    $output = @(& pwsh -NoProfile -NonInteractive -File $checker `
        -ValidateManifest $Manifest `
        -ExpectedCodeCommit $ExpectedCommit `
        -ExpectedCodeTree $ExpectedTree `
        -ExpectedRuntimeImageDigest $ExpectedRuntime 2>&1)
    return [pscustomobject]@{
        ExitCode = $LASTEXITCODE
        Output = $output -join "`n"
    }
}

function Assert-Pass {
    param([Parameter(Mandatory)][string]$Manifest)

    $result = Invoke-Checker -Manifest $Manifest
    if ($result.ExitCode -ne 0) {
        throw "Expected exact recovery manifest to pass: $($result.Output)"
    }
}

function Assert-Fail {
    param(
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)][object]$Result
    )

    if ($Result.ExitCode -eq 0) {
        throw "Expected recovery fixture '$Name' to fail."
    }
}

try {
    New-Item -ItemType Directory -Path $scratchRoot | Out-Null
    $good = New-ManifestFixture -Name 'good'
    Assert-Pass -Manifest $good

    $missing = Copy-ManifestFixture -SourceManifest $good -Name 'missing'
    $missingManifest = Get-Content -Raw -LiteralPath $missing | ConvertFrom-Json
    $missingFile = Join-Path (Split-Path -Parent $missing) $missingManifest.immutable_objects[0].file
    Remove-Item -LiteralPath $missingFile -Force
    Assert-Fail -Name 'missing immutable object' -Result (Invoke-Checker -Manifest $missing)

    $extra = Copy-ManifestFixture -SourceManifest $good -Name 'extra'
    [System.IO.File]::WriteAllText(
        (Join-Path (Split-Path -Parent $extra) 'objects/extra.blob'),
        'extra',
        [System.Text.UTF8Encoding]::new($false)
    )
    Assert-Fail -Name 'extra immutable object' -Result (Invoke-Checker -Manifest $extra)

    $tampered = Copy-ManifestFixture -SourceManifest $good -Name 'tampered'
    $tamperedManifest = Get-Content -Raw -LiteralPath $tampered | ConvertFrom-Json
    $tamperedFile = Join-Path (
        Split-Path -Parent $tampered
    ) $tamperedManifest.immutable_objects[0].file
    [System.IO.File]::WriteAllText(
        $tamperedFile,
        'tampered',
        [System.Text.UTF8Encoding]::new($false)
    )
    Assert-Fail -Name 'tampered immutable object' -Result (Invoke-Checker -Manifest $tampered)

    Assert-Fail -Name 'Code identity drift' -Result (
        Invoke-Checker -Manifest $good -ExpectedCommit ('6' * 40)
    )
    Assert-Fail -Name 'Runtime identity drift' -Result (
        Invoke-Checker -Manifest $good -ExpectedRuntime ('sha256:' + ('7' * 64))
    )

    Write-Output 'Recovery checker fixture tests passed: 1 positive, 5 negative.'
    exit 0
}
catch {
    Write-Error $_
    exit 1
}
finally {
    if (Test-Path -LiteralPath $scratchRoot) {
        $resolvedScratch = [System.IO.Path]::GetFullPath($scratchRoot)
        $tempPrefix = [System.IO.Path]::GetFullPath(
            [System.IO.Path]::GetTempPath()
        ).TrimEnd([System.IO.Path]::DirectorySeparatorChar) +
            [System.IO.Path]::DirectorySeparatorChar
        if (-not $resolvedScratch.StartsWith(
            $tempPrefix,
            [System.StringComparison]::OrdinalIgnoreCase
        ) -or [System.IO.Path]::GetFileName($resolvedScratch) -notlike 'ficant-recovery-check-*') {
            throw "Refusing to remove unexpected recovery fixture root '$resolvedScratch'."
        }
        Remove-Item -LiteralPath $resolvedScratch -Recurse -Force
    }
}
