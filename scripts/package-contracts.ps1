[CmdletBinding()]
param(
    [string]$OutputDirectory = '',
    [string]$DescriptorPath = '',
    # Release version without the leading 'v', for example '0.1.0-alpha.13'. When given, the package
    # is packed from a staged copy whose package.json carries this version; the committed
    # package.json (bound by the license inventory) stays at 0.0.0.
    [string]$Version = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$ficantRoot = Split-Path -Parent $PSScriptRoot
$packageRoot = Join-Path $ficantRoot 'web-dm\packages\contracts-generated'
$sourceRoot = Join-Path $packageRoot 'src'
$packageVersion = if ([string]::IsNullOrWhiteSpace($Version)) { '0.0.0' } else { $Version }
if ($packageVersion -notmatch '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z][0-9A-Za-z.-]*)?$') {
    throw "Package version must be semantic, without a leading 'v': '$packageVersion'."
}
$expectedPackageName = "ficant-contracts-generated-$packageVersion.tgz"
$utf8NoBom = [System.Text.UTF8Encoding]::new($false)

function Invoke-CapturedNative {
    param(
        [Parameter(Mandatory)]
        [string]$FilePath,
        [Parameter(Mandatory)]
        [string[]]$ArgumentList,
        [Parameter(Mandatory)]
        [string]$WorkingDirectory
    )

    Push-Location -LiteralPath $WorkingDirectory
    try {
        $output = @(& $FilePath @ArgumentList 2>&1)
        $exitCode = $LASTEXITCODE
    }
    finally {
        Pop-Location
    }

    if ($exitCode -ne 0) {
        $renderedOutput = ($output | ForEach-Object { $_.ToString() }) -join [Environment]::NewLine
        throw "Native command failed with exit $exitCode`: $FilePath $($ArgumentList -join ' ')$([Environment]::NewLine)$renderedOutput"
    }

    return @($output | ForEach-Object { $_.ToString() })
}

function Assert-ExactToolVersion {
    param(
        [Parameter(Mandatory)]
        [string]$FilePath,
        [Parameter(Mandatory)]
        [string[]]$ArgumentList,
        [Parameter(Mandatory)]
        [string]$Expected
    )

    $versionOutput = @(Invoke-CapturedNative -FilePath $FilePath -ArgumentList $ArgumentList -WorkingDirectory $ficantRoot)
    $actual = ($versionOutput -join [Environment]::NewLine).Trim()
    if ($actual -ne $Expected) {
        throw "Required '$FilePath $($ArgumentList -join ' ')' version '$Expected', but the active version is '$actual'."
    }
}

function Get-GeneratedTreeSha256 {
    param(
        [Parameter(Mandatory)]
        [string]$Root
    )

    [string[]]$relativePaths = @(
        Get-ChildItem -LiteralPath $Root -Recurse -File -Filter '*.ts' |
            ForEach-Object {
                [System.IO.Path]::GetRelativePath($Root, $_.FullName).Replace('\', '/')
            }
    )
    [Array]::Sort($relativePaths, [StringComparer]::Ordinal)
    if ($relativePaths.Count -eq 0) {
        throw "No generated TypeScript sources were found under '$Root'."
    }

    $hasher = [System.Security.Cryptography.IncrementalHash]::CreateHash(
        [System.Security.Cryptography.HashAlgorithmName]::SHA256
    )
    try {
        foreach ($relativePath in $relativePaths) {
            $relativePathBytes = $utf8NoBom.GetBytes($relativePath)
            $absolutePath = Join-Path $Root $relativePath.Replace('/', [System.IO.Path]::DirectorySeparatorChar)
            $fileBytes = [System.IO.File]::ReadAllBytes($absolutePath)
            [byte[]]$relativePathLength = [BitConverter]::GetBytes([uint64]$relativePathBytes.Length)
            [byte[]]$fileLength = [BitConverter]::GetBytes([uint64]$fileBytes.Length)
            if ([BitConverter]::IsLittleEndian) {
                [Array]::Reverse($relativePathLength)
                [Array]::Reverse($fileLength)
            }
            $hasher.AppendData($relativePathLength)
            $hasher.AppendData($relativePathBytes)
            $hasher.AppendData($fileLength)
            $hasher.AppendData($fileBytes)
        }
        $digest = [Convert]::ToHexString($hasher.GetHashAndReset()).ToLowerInvariant()
    }
    finally {
        $hasher.Dispose()
    }

    return [pscustomobject]@{
        digest = $digest
        count = $relativePaths.Count
    }
}

function Assert-MinimumToolVersion {
    param(
        [Parameter(Mandatory)]
        [string]$FilePath,
        [Parameter(Mandatory)]
        [string[]]$ArgumentList,
        [Parameter(Mandatory)]
        [version]$Minimum
    )

    # Determinism is checked by comparing fresh runs with each other, so only the tool family and a
    # minimum version are required here (ADR-0020); CI pins the exact release toolchain.
    $versionOutput = @(Invoke-CapturedNative -FilePath $FilePath -ArgumentList $ArgumentList -WorkingDirectory $ficantRoot)
    $actual = ($versionOutput -join [Environment]::NewLine).Trim()
    $match = [regex]::Match($actual, '(\d+)\.(\d+)(?:\.(\d+))?')
    if (-not $match.Success) {
        throw "Could not read a version from '$FilePath $($ArgumentList -join ' ')': '$actual'."
    }
    $patch = if ($match.Groups[3].Success) { [int]$match.Groups[3].Value } else { 0 }
    $version = [version]::new([int]$match.Groups[1].Value, [int]$match.Groups[2].Value, $patch)
    if ($version -lt $Minimum) {
        throw "'$FilePath' must be at least $Minimum, but the active version is '$actual'."
    }
}

if (-not (Test-Path -LiteralPath $packageRoot -PathType Container)) {
    throw "Contract package root does not exist: '$packageRoot'."
}
if (-not (Test-Path -LiteralPath $sourceRoot -PathType Container)) {
    throw "Generated TypeScript source root does not exist: '$sourceRoot'."
}

Assert-ExactToolVersion -FilePath 'corepack' -ArgumentList @('pnpm@10.12.4', '--version') -Expected '10.12.4'

$resolvedOutputDirectory = if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    Join-Path $packageRoot 'dist'
}
elseif ([System.IO.Path]::IsPathRooted($OutputDirectory)) {
    [System.IO.Path]::GetFullPath($OutputDirectory)
}
else {
    [System.IO.Path]::GetFullPath((Join-Path $ficantRoot $OutputDirectory))
}
[System.IO.Directory]::CreateDirectory($resolvedOutputDirectory) | Out-Null

$temporaryDescriptor = $null
$temporaryStage = $null
try {
    $resolvedDescriptor = if ([string]::IsNullOrWhiteSpace($DescriptorPath)) {
        $bufCommand = if ([string]::IsNullOrWhiteSpace($env:FICANT_BUF)) {
            'buf'
        }
        else {
            $env:FICANT_BUF
        }
        Assert-MinimumToolVersion -FilePath $bufCommand -ArgumentList @('--version') -Minimum '1.56.0'
        $temporaryDescriptor = Join-Path (
            [System.IO.Path]::GetTempPath()
        ) ("ficant-r8a-descriptor-$([Guid]::NewGuid().ToString('N')).bin")
        Invoke-CapturedNative -FilePath $bufCommand -ArgumentList @(
            'build',
            'interface',
            '--as-file-descriptor-set',
            '-o',
            $temporaryDescriptor
        ) -WorkingDirectory $ficantRoot | Out-Null
        $temporaryDescriptor
    }
    elseif ([System.IO.Path]::IsPathRooted($DescriptorPath)) {
        [System.IO.Path]::GetFullPath($DescriptorPath)
    }
    else {
        [System.IO.Path]::GetFullPath((Join-Path $ficantRoot $DescriptorPath))
    }

    if (-not (Test-Path -LiteralPath $resolvedDescriptor -PathType Leaf)) {
        throw "Descriptor file does not exist: '$resolvedDescriptor'."
    }

    $treeIdentity = Get-GeneratedTreeSha256 -Root $sourceRoot
    $packagePath = Join-Path $resolvedOutputDirectory $expectedPackageName
    if (Test-Path -LiteralPath $packagePath -PathType Leaf) {
        Remove-Item -LiteralPath $packagePath -Force
    }

    $packWorkingDirectory = $packageRoot
    if ($packageVersion -ne '0.0.0') {
        $stagingRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("ficant-contracts-stage-$([Guid]::NewGuid().ToString('N'))")
        $temporaryStage = $stagingRoot
        [System.IO.Directory]::CreateDirectory($stagingRoot) | Out-Null
        Copy-Item -LiteralPath $sourceRoot -Destination (Join-Path $stagingRoot 'src') -Recurse
        Copy-Item -LiteralPath (Join-Path $packageRoot 'README.md') -Destination $stagingRoot
        $manifest = Get-Content -LiteralPath (Join-Path $packageRoot 'package.json') -Raw | ConvertFrom-Json -AsHashtable
        $manifest['version'] = $packageVersion
        [System.IO.File]::WriteAllText(
            (Join-Path $stagingRoot 'package.json'),
            (($manifest | ConvertTo-Json -Depth 20) + "`n"),
            $utf8NoBom
        )
        $packWorkingDirectory = $stagingRoot
    }

    Invoke-CapturedNative -FilePath 'corepack' -ArgumentList @(
        'pnpm@10.12.4',
        'pack',
        '--pack-destination',
        $resolvedOutputDirectory
    ) -WorkingDirectory $packWorkingDirectory | Out-Null

    if (-not (Test-Path -LiteralPath $packagePath -PathType Leaf)) {
        throw "pnpm pack did not create the expected artifact '$packagePath'."
    }

    [ordered]@{
        schema = 'ficant.contract-package-evidence.v1'
        package = "@ficant/contracts-generated@$packageVersion"
        descriptor_sha256 = (Get-FileHash -LiteralPath $resolvedDescriptor -Algorithm SHA256).Hash.ToLowerInvariant()
        source_tree_sha256 = $treeIdentity.digest
        source_file_count = $treeIdentity.count
        package_sha256 = (Get-FileHash -LiteralPath $packagePath -Algorithm SHA256).Hash.ToLowerInvariant()
        package_path = [System.IO.Path]::GetFullPath($packagePath)
    } | ConvertTo-Json -Compress
}
finally {
    if ($null -ne $temporaryDescriptor -and (Test-Path -LiteralPath $temporaryDescriptor -PathType Leaf)) {
        Remove-Item -LiteralPath $temporaryDescriptor -Force
    }
    if ($null -ne $temporaryStage -and (Test-Path -LiteralPath $temporaryStage -PathType Container)) {
        Remove-Item -LiteralPath $temporaryStage -Recurse -Force
    }
}
