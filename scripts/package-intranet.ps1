[CmdletBinding()]
param(
    [switch]$ListOnly,

    [string]$OutputDir = '',

    [string]$CargoTargetDir = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'check-common.ps1')

$script:ExpectedRulePackLength = 159
$script:ExpectedRulePackSha256 = '8d8a0b944557cf87ed3d1f598ab3e02fa7f0aec125c1c26c17280acbd24367c5'

function Resolve-PackageFullPath {
    param(
        [Parameter(Mandatory)]
        [string]$RawPath
    )

    if ([System.IO.Path]::IsPathRooted($RawPath)) {
        return [System.IO.Path]::GetFullPath($RawPath)
    }
    return [System.IO.Path]::GetFullPath((Join-Path $script:FicantRoot $RawPath))
}

function Get-GitSha40 {
    param(
        [Parameter(Mandatory)]
        [string[]]$RevParseArgs,

        [Parameter(Mandatory)]
        [string]$Label
    )

    $gitArgs = @('-C', $script:FicantRoot, 'rev-parse') + $RevParseArgs
    $raw = & git @gitArgs 2>&1
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        throw "Failed to resolve Git $Label via 'git $($gitArgs -join ' ')' (exit $exitCode): $(($raw | Out-String).Trim())"
    }
    $sha = ($raw | Out-String).Trim()
    if ($sha -cnotmatch '^[0-9a-f]{40}$') {
        throw "Resolved Git $Label is not a 40-character lowercase SHA: '$sha'"
    }
    return $sha
}

function Assert-NoReparsePointAncestors {
    param(
        [Parameter(Mandatory)]
        [string]$Path
    )

    $current = [System.IO.Path]::GetFullPath($Path)
    while (-not [string]::IsNullOrEmpty($current)) {
        if (Test-Path -LiteralPath $current) {
            $item = Get-Item -LiteralPath $current -Force -ErrorAction Stop
            if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Reparse point / symlink rejected in path: $current"
            }
        }
        $parent = [System.IO.Path]::GetDirectoryName($current)
        if ([string]::IsNullOrEmpty($parent) -or $parent -eq $current) {
            break
        }
        $current = $parent
    }
}

function Copy-CleanWebDistTree {
    param(
        [Parameter(Mandatory)]
        [string]$SourceDir,

        [Parameter(Mandatory)]
        [string]$DestinationDir
    )

    if (-not (Test-Path -LiteralPath $SourceDir -PathType Container)) {
        throw "Built web dist directory is missing: $SourceDir"
    }

    Assert-NoReparsePointAncestors -Path $SourceDir
    [void](New-Item -ItemType Directory -Path $DestinationDir -Force)
    Assert-NoReparsePointAncestors -Path $DestinationDir

    $entries = @(Get-ChildItem -LiteralPath $SourceDir -Recurse -Force -ErrorAction Stop)
    foreach ($entry in $entries) {
        if (($entry.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or $null -ne $entry.LinkType) {
            throw "Symlink or reparse point rejected in web dist: $($entry.FullName)"
        }
        $relative = [System.IO.Path]::GetRelativePath($SourceDir, $entry.FullName)
        $targetPath = Join-Path $DestinationDir $relative
        if ($entry.PSIsContainer) {
            [void](New-Item -ItemType Directory -Path $targetPath -Force)
        }
        else {
            $parentDir = Split-Path -Parent $targetPath
            if (-not (Test-Path -LiteralPath $parentDir -PathType Container)) {
                [void](New-Item -ItemType Directory -Path $parentDir -Force)
            }
            [System.IO.File]::Copy($entry.FullName, $targetPath, $false)
        }
    }
}

function Assert-StagedWebDistValid {
    param(
        [Parameter(Mandatory)]
        [string]$StagingDir
    )

    $indexPath = Join-Path $StagingDir 'index.html'
    if (-not (Test-Path -LiteralPath $indexPath -PathType Leaf)) {
        throw "Staged web dist is missing required index.html: $indexPath"
    }
    $indexInfo = Get-Item -LiteralPath $indexPath -ErrorAction Stop
    if ($indexInfo.Length -le 0) {
        throw "Staged web dist index.html must be non-empty: $indexPath"
    }

    $rulepackPath = Join-Path $StagingDir 'rulepacks\cgb-futures-v2.bin'
    if (-not (Test-Path -LiteralPath $rulepackPath -PathType Leaf)) {
        throw "Staged web dist is missing pinned RulePack asset: $rulepackPath"
    }
    $rulepackInfo = Get-Item -LiteralPath $rulepackPath -ErrorAction Stop
    if ($rulepackInfo.Length -ne $script:ExpectedRulePackLength) {
        throw "Pinned RulePack length mismatch: got $($rulepackInfo.Length), expected $script:ExpectedRulePackLength"
    }
    $rulepackHash = (Get-FileHash -LiteralPath $rulepackPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($rulepackHash -cne $script:ExpectedRulePackSha256) {
        throw "Pinned RulePack SHA-256 mismatch: got $rulepackHash, expected $script:ExpectedRulePackSha256"
    }
}

function Invoke-NativeChecked {
    param(
        [Parameter(Mandatory)]
        [string]$Label,

        [Parameter(Mandatory)]
        [string]$FilePath,

        [Parameter(Mandatory)]
        [string[]]$ArgumentList,

        [Parameter(Mandatory)]
        [string]$WorkingDirectory
    )

    Write-Host ''
    Write-Host ("==> {0}" -f $Label)
    Push-Location -LiteralPath $WorkingDirectory
    try {
        & $FilePath @ArgumentList
        $exitCode = $LASTEXITCODE
    }
    finally {
        Pop-Location
    }

    if ($exitCode -ne 0) {
        throw "Step failed with exit code ${exitCode}: $Label"
    }
}

$resolvedOutputDir = if ([string]::IsNullOrWhiteSpace($OutputDir)) {
    Join-Path $script:FicantRoot 'target\intranet-package'
}
else {
    Resolve-PackageFullPath -RawPath $OutputDir
}

$resolvedCargoTargetDir = if ([string]::IsNullOrWhiteSpace($CargoTargetDir)) {
    Join-Path $script:FicantRoot 'target\intranet-embed-release'
}
else {
    Resolve-PackageFullPath -RawPath $CargoTargetDir
}

$binaryName = if ($IsWindows) { 'ficant-server.exe' } else { 'ficant-server' }

$planSteps = @(
    New-FicantCheckStep -Name 'Desk Web frozen install' -FilePath 'pnpm' -ArgumentList @('--dir', 'apps/desk-web', 'install', '--frozen-lockfile')
    New-FicantCheckStep -Name 'Desk Web production build' -FilePath 'pnpm' -ArgumentList @('--dir', 'apps/desk-web', 'run', 'build')
    New-FicantCheckStep -Name 'Rust embedded release build (with isolated staged FICANT_EMBED_WEB_DIST_DIR)' -FilePath 'cargo' -ArgumentList @('build', '--release', '--locked', '--offline', '--bin', 'ficant-server')
)

try {
    if ($ListOnly) {
        Show-FicantCheckPlan -Steps $planSteps
        Write-Host ("Output directory: {0}" -f $resolvedOutputDir)
        Write-Host ("Isolated Cargo target directory: {0}" -f $resolvedCargoTargetDir)
        Write-Host ("Artifacts emitted: {0}, {0}.sha256" -f $binaryName)
        Write-Host 'Staging: apps/desk-web/dist copied into a private non-reparse temp directory and removed in finally.'
        exit 0
    }

    Assert-FicantRustCapability
    [void](Assert-FicantMinimumVersion -Name 'node' -ArgumentList @('--version') -Minimum '22.13.0')
    [void](Assert-FicantMinimumVersion -Name 'pnpm' -ArgumentList @('--version') -Minimum '11.0.0')
    Assert-FicantCommand 'git'

    $commitSha = Get-GitSha40 -RevParseArgs @('HEAD') -Label 'commit SHA'
    $treeSha = Get-GitSha40 -RevParseArgs @('HEAD^{tree}') -Label 'tree SHA'

    Invoke-NativeChecked -Label 'Desk Web frozen install' -FilePath 'pnpm' -ArgumentList @('--dir', 'apps/desk-web', 'install', '--frozen-lockfile') -WorkingDirectory $script:FicantRoot
    Invoke-NativeChecked -Label 'Desk Web production build' -FilePath 'pnpm' -ArgumentList @('--dir', 'apps/desk-web', 'run', 'build') -WorkingDirectory $script:FicantRoot

    $tempRoot = (Get-Item -LiteralPath ([System.IO.Path]::GetTempPath()) -ErrorAction Stop).FullName
    $runId = [Guid]::NewGuid().ToString('N')
    $stagingDir = [System.IO.Path]::GetFullPath((Join-Path $tempRoot "ficant-intranet-embed-dist-$runId"))
    if (-not $stagingDir.StartsWith($tempRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to create staging directory outside OS temp directory: $stagingDir"
    }

    $envNames = @(
        'FICANT_CODE_COMMIT_SHA',
        'FICANT_CODE_TREE_SHA',
        'FICANT_EMBED_WEB_DIST_DIR',
        'CARGO_TARGET_DIR'
    )
    $savedEnv = @{}
    foreach ($name in $envNames) {
        $savedEnv[$name] = [System.Environment]::GetEnvironmentVariable($name, 'Process')
    }

    try {
        $sourceDistDir = Join-Path $script:FicantRoot 'apps\desk-web\dist'
        Copy-CleanWebDistTree -SourceDir $sourceDistDir -DestinationDir $stagingDir
        Assert-StagedWebDistValid -StagingDir $stagingDir

        [System.Environment]::SetEnvironmentVariable('FICANT_CODE_COMMIT_SHA', $commitSha, 'Process')
        [System.Environment]::SetEnvironmentVariable('FICANT_CODE_TREE_SHA', $treeSha, 'Process')
        [System.Environment]::SetEnvironmentVariable('FICANT_EMBED_WEB_DIST_DIR', $stagingDir, 'Process')
        [System.Environment]::SetEnvironmentVariable('CARGO_TARGET_DIR', $resolvedCargoTargetDir, 'Process')

        Invoke-NativeChecked -Label 'Rust single-binary embedded release build' -FilePath 'cargo' -ArgumentList @('build', '--release', '--locked', '--offline', '--bin', 'ficant-server') -WorkingDirectory $script:FicantRoot

        $builtBinaryPath = Join-Path (Join-Path $resolvedCargoTargetDir 'release') $binaryName
        if (-not (Test-Path -LiteralPath $builtBinaryPath -PathType Leaf)) {
            throw "Expected built ficant-server binary was not found: $builtBinaryPath"
        }

        [void](New-Item -ItemType Directory -Path $resolvedOutputDir -Force)
        $outputBinaryPath = Join-Path $resolvedOutputDir $binaryName
        $outputShaPath = "$outputBinaryPath.sha256"

        [System.IO.File]::Copy($builtBinaryPath, $outputBinaryPath, $true)
        $binaryHash = (Get-FileHash -LiteralPath $outputBinaryPath -Algorithm SHA256).Hash.ToLowerInvariant()
        $binarySize = (Get-Item -LiteralPath $outputBinaryPath -ErrorAction Stop).Length

        $utf8NoBom = [System.Text.UTF8Encoding]::new($false)
        [System.IO.File]::WriteAllText($outputShaPath, "$binaryHash  $binaryName`n", $utf8NoBom)

        Write-Host ''
        Write-Host 'FICANT intranet single-binary package created.'
        Write-Host ("  binary:     {0} ({1} bytes)" -f $outputBinaryPath, $binarySize)
        Write-Host ("  sha256:     {0}" -f $binaryHash)
        Write-Host ("  manifest:   {0}" -f $outputShaPath)
        Write-Host ("  commit_sha: {0}" -f $commitSha)
        Write-Host ("  tree_sha:   {0}" -f $treeSha)
    }
    finally {
        foreach ($name in $envNames) {
            [System.Environment]::SetEnvironmentVariable($name, $savedEnv[$name], 'Process')
        }
        if (-not [string]::IsNullOrWhiteSpace($stagingDir) -and (Test-Path -LiteralPath $stagingDir -PathType Container)) {
            $resolvedStaging = (Resolve-Path -LiteralPath $stagingDir).Path
            if ($resolvedStaging.StartsWith($tempRoot, [System.StringComparison]::OrdinalIgnoreCase) -and
                (Split-Path -Leaf $resolvedStaging) -match '^ficant-intranet-embed-dist-[0-9a-f]{32}$') {
                Remove-Item -LiteralPath $resolvedStaging -Recurse -Force
            }
        }
    }

    exit 0
}
catch {
    Write-Error $_
    exit 1
}
