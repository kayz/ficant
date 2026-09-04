[CmdletBinding()]
param(
    [switch]$RemoveRepositoryOutput
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$ficantRoot = Split-Path -Parent $PSScriptRoot
$packageRoot = Join-Path $ficantRoot 'web-dm\packages\contracts-generated'
$portfolioSource = Join-Path $packageRoot 'src\ficant\portfolio\v1\portfolio_pb.ts'
$packageScript = Join-Path $PSScriptRoot 'package-contracts.ps1'
$expectedPackageName = 'ficant-contracts-generated-0.0.0.tgz'
$repositoryOutput = Join-Path $packageRoot 'dist'
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

function Invoke-PackageBuild {
    param(
        [Parameter(Mandatory)]
        [string]$OutputDirectory
    )

    $output = @(& $packageScript -OutputDirectory $OutputDirectory)
    if ($output.Count -ne 1) {
        throw "Expected one package evidence record, received $($output.Count)."
    }
    return ($output[0] | ConvertFrom-Json)
}

function Remove-VerifiedTemporaryRoot {
    param(
        [Parameter(Mandatory)]
        [string]$Path
    )

    $resolvedPath = [System.IO.Path]::GetFullPath($Path)
    $resolvedSystemTemp = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
    if (-not $resolvedPath.StartsWith($resolvedSystemTemp, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to remove temporary path outside the system temp directory: '$resolvedPath'."
    }
    if (-not ([System.IO.Path]::GetFileName($resolvedPath)).StartsWith(
        'ficant-r8a-contract-package-',
        [StringComparison]::Ordinal
    )) {
        throw "Refusing to remove unexpected temporary directory '$resolvedPath'."
    }
    if (Test-Path -LiteralPath $resolvedPath -PathType Container) {
        Remove-Item -LiteralPath $resolvedPath -Recurse -Force
    }
}

function Remove-VerifiedRepositoryOutput {
    $expectedPath = [System.IO.Path]::GetFullPath(
        (Join-Path $ficantRoot 'web-dm\packages\contracts-generated\dist')
    )
    $candidatePath = [System.IO.Path]::GetFullPath($repositoryOutput)
    if (-not [string]::Equals($candidatePath, $expectedPath, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to remove unexpected repository output '$candidatePath'."
    }
    if (-not (Test-Path -LiteralPath $candidatePath)) {
        return
    }

    $resolvedPath = [System.IO.Path]::GetFullPath(
        (Resolve-Path -LiteralPath $candidatePath).ProviderPath
    )
    if (-not [string]::Equals($resolvedPath, $expectedPath, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to remove repository output resolved outside the expected path: '$resolvedPath'."
    }
    $item = Get-Item -LiteralPath $resolvedPath -Force
    if (-not $item.PSIsContainer) {
        throw "Refusing to remove repository output because it is not a directory: '$resolvedPath'."
    }
    if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Refusing to remove repository output because it is a reparse point: '$resolvedPath'."
    }

    Remove-Item -LiteralPath $resolvedPath -Recurse -Force
}

if (-not (Test-Path -LiteralPath $portfolioSource -PathType Leaf)) {
    throw "R8A Portfolio generated source is required before package verification: '$portfolioSource'."
}

$temporaryRoot = Join-Path (
    [System.IO.Path]::GetTempPath()
) ("ficant-r8a-contract-package-$([Guid]::NewGuid().ToString('N'))")
[System.IO.Directory]::CreateDirectory($temporaryRoot) | Out-Null

$previousCorepackNetwork = $env:COREPACK_ENABLE_NETWORK
$env:COREPACK_ENABLE_NETWORK = '0'
try {
    $firstOutput = Join-Path $temporaryRoot 'package-a'
    $secondOutput = Join-Path $temporaryRoot 'package-b'
    $first = Invoke-PackageBuild -OutputDirectory $firstOutput
    $second = Invoke-PackageBuild -OutputDirectory $secondOutput

    foreach ($property in @('descriptor_sha256', 'source_tree_sha256', 'source_file_count', 'package_sha256')) {
        if ($first.$property -ne $second.$property) {
            throw "Fresh package runs disagree on '$property': '$($first.$property)' versus '$($second.$property)'."
        }
    }

    $firstBytes = [System.IO.File]::ReadAllBytes($first.package_path)
    $secondBytes = [System.IO.File]::ReadAllBytes($second.package_path)
    $bytesAreEqual = [System.Collections.StructuralComparisons]::StructuralEqualityComparer.Equals(
        $firstBytes,
        $secondBytes
    )
    if (-not $bytesAreEqual) {
        throw 'Fresh contract package bytes are not identical.'
    }

    $finalOutput = $repositoryOutput
    if ($RemoveRepositoryOutput) {
        Remove-VerifiedRepositoryOutput
    }
    $final = Invoke-PackageBuild -OutputDirectory $finalOutput
    if ($final.package_sha256 -ne $first.package_sha256) {
        throw 'Final ignored package digest differs from the two fresh package runs.'
    }

    $relativeFinalPath = [System.IO.Path]::GetRelativePath($ficantRoot, $final.package_path)
    Invoke-CapturedNative -FilePath 'git' -ArgumentList @(
        'check-ignore',
        '--quiet',
        '--',
        $relativeFinalPath
    ) -WorkingDirectory $ficantRoot | Out-Null

    $consumerRoot = Join-Path $temporaryRoot 'consumer'
    [System.IO.Directory]::CreateDirectory($consumerRoot) | Out-Null
    $packageFileSpec = 'file:' + $first.package_path.Replace('\', '/')
    $consumerPackage = [ordered]@{
        name = 'ficant-r8a-contract-package-consumer'
        version = '0.0.0'
        private = $true
        type = 'module'
        packageManager = 'pnpm@10.12.4'
        dependencies = [ordered]@{
            '@bufbuild/protobuf' = '2.5.2'
            '@ficant/contracts-generated' = $packageFileSpec
        }
        devDependencies = [ordered]@{
            typescript = '5.8.3'
        }
    }
    [System.IO.File]::WriteAllText(
        (Join-Path $consumerRoot 'package.json'),
        ($consumerPackage | ConvertTo-Json -Depth 6) + "`n",
        $utf8NoBom
    )

    $tsconfig = @'
{
  "compilerOptions": {
    "target": "ES2022",
    "lib": ["ES2022", "DOM"],
    "module": "ESNext",
    "moduleResolution": "Bundler",
    "strict": true,
    "skipLibCheck": false,
    "noEmit": true
  },
  "include": ["consumer.ts"]
}
'@
    [System.IO.File]::WriteAllText(
        (Join-Path $consumerRoot 'tsconfig.json'),
        $tsconfig + "`n",
        $utf8NoBom
    )

    $consumerSource = @'
import { create } from "@bufbuild/protobuf";
import { DecimalValueSchema } from "@ficant/contracts-generated/ficant/core/v1/common_pb";
import {
  GetPortfolioPageRequestSchema,
  PortfolioWorkbenchPageId,
  PortfolioWorkbenchService,
} from "@ficant/contracts-generated/ficant/portfolio/v1/portfolio_pb";

const decimal = create(DecimalValueSchema, { coefficient: "123", scale: 2 });
const request = create(GetPortfolioPageRequestSchema, {
  pageId: PortfolioWorkbenchPageId.P02,
});

if (decimal.coefficient !== "123" || request.pageId !== PortfolioWorkbenchPageId.P02) {
  throw new Error("generated schema consumption failed");
}
if (!PortfolioWorkbenchService.methods.some((method) => method.name === "GetPage")) {
  throw new Error("Portfolio Workbench service descriptor is missing GetPage");
}
'@
    [System.IO.File]::WriteAllText(
        (Join-Path $consumerRoot 'consumer.ts'),
        $consumerSource + "`n",
        $utf8NoBom
    )

    Invoke-CapturedNative -FilePath 'corepack' -ArgumentList @(
        'pnpm@10.12.4',
        'install',
        '--offline',
        '--ignore-scripts',
        '--strict-peer-dependencies'
    ) -WorkingDirectory $consumerRoot | Out-Null
    Invoke-CapturedNative -FilePath 'corepack' -ArgumentList @(
        'pnpm@10.12.4',
        'exec',
        'tsc',
        '--project',
        'tsconfig.json'
    ) -WorkingDirectory $consumerRoot | Out-Null

    $installedManifestPath = Join-Path $consumerRoot 'node_modules\@ficant\contracts-generated\package.json'
    $installedManifest = Get-Content -LiteralPath $installedManifestPath -Raw | ConvertFrom-Json
    if ($installedManifest.name -ne '@ficant/contracts-generated' -or $installedManifest.version -ne '0.0.0') {
        throw 'Offline consumer installed an unexpected package identity.'
    }

    [ordered]@{
        schema = 'ficant.contract-package-test-evidence.v1'
        tests_passed = 6
        package = '@ficant/contracts-generated@0.0.0'
        descriptor_sha256 = $final.descriptor_sha256
        source_tree_sha256 = $final.source_tree_sha256
        source_file_count = $final.source_file_count
        package_sha256 = $final.package_sha256
        package_path = [System.IO.Path]::GetFullPath((Join-Path $finalOutput $expectedPackageName))
        offline_consumer = 'typescript-5.8.3'
    } | ConvertTo-Json -Compress
}
finally {
    $env:COREPACK_ENABLE_NETWORK = $previousCorepackNetwork
    try {
        Remove-VerifiedTemporaryRoot -Path $temporaryRoot
    }
    finally {
        if ($RemoveRepositoryOutput) {
            Remove-VerifiedRepositoryOutput
        }
    }
}
