[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$AuthorityRoot,

    [Parameter(Mandatory)]
    [ValidatePattern('^[0-9a-f]{40}$')]
    [string]$ExpectedAuthorityCommit,

    [ValidatePattern('^$|^[0-9a-f]{40}$')]
    [string]$ExpectedPublicCommit = '',

    [switch]$ListOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$expectedBlockOrder = @(
    'dev-up',
    'dev-down',
    'check-fast',
    'check-full',
    'check-integration',
    'recovery-proof'
)

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

    $output = @(& $FilePath @ArgumentList)
    if ($LASTEXITCODE -ne 0) {
        throw "$FilePath failed with exit code $LASTEXITCODE."
    }
    return ($output -join "`n").Trim()
}

function Assert-AuthoritySnapshot {
    param(
        [Parameter(Mandatory)]
        [string]$Root,

        [Parameter(Mandatory)]
        [string]$ExpectedCommit
    )

    $resolvedRoot = [System.IO.Path]::GetFullPath($Root)
    if (-not (Test-Path -LiteralPath $resolvedRoot -PathType Container)) {
        throw "Authority root is not a directory: '$resolvedRoot'."
    }
    $actualCommit = Get-NativeOutput -FilePath 'git' -ArgumentList @(
        '-C', $resolvedRoot, 'rev-parse', 'HEAD'
    )
    if ($actualCommit -cne $ExpectedCommit) {
        throw "Authority commit mismatch: expected '$ExpectedCommit', actual '$actualCommit'."
    }
    $status = Get-NativeOutput -FilePath 'git' -ArgumentList @(
        '-C', $resolvedRoot, 'status', '--porcelain', '--untracked-files=all'
    )
    if (-not [string]::IsNullOrEmpty($status)) {
        throw 'Authority repository must be clean.'
    }

    $manifestPath = Join-Path $resolvedRoot 'authority-manifest.json'
    $manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
    if ($manifest.schema -cne 'ficant.authority.snapshot.v1') {
        throw "Unsupported authority manifest schema '$($manifest.schema)'."
    }
    if ($manifest.public_code_commit -notmatch '^[0-9a-f]{40}$') {
        throw 'Authority manifest public code binding is not an exact commit.'
    }
    $required = @('SPEC.md', 'ACCEPTANCE.md', 'MANUAL.md')
    $actualDocuments = @($manifest.documents | ForEach-Object { [string]$_.path })
    if ($actualDocuments.Count -ne $required.Count) {
        throw 'Authority manifest must contain exactly three documents.'
    }
    foreach ($path in $required) {
        $document = @($manifest.documents | Where-Object { $_.path -ceq $path })
        if ($document.Count -ne 1 -or [string]$document[0].sha256 -notmatch '^[0-9A-F]{64}$') {
            throw "Authority manifest does not bind '$path' exactly once."
        }
        $documentPath = [System.IO.Path]::GetFullPath((Join-Path $resolvedRoot $path))
        $rootPrefix = $resolvedRoot.TrimEnd(
            [System.IO.Path]::DirectorySeparatorChar,
            [System.IO.Path]::AltDirectorySeparatorChar
        ) + [System.IO.Path]::DirectorySeparatorChar
        if (-not $documentPath.StartsWith(
            $rootPrefix,
            [System.StringComparison]::OrdinalIgnoreCase
        )) {
            throw "Authority document escapes the repository: '$path'."
        }
        $actualHash = (Get-FileHash -LiteralPath $documentPath -Algorithm SHA256).Hash
        if ($actualHash -cne [string]$document[0].sha256) {
            throw "Authority document hash mismatch: '$path'."
        }
    }
    return [pscustomobject]@{
        Root = $resolvedRoot
        ManualPath = Join-Path $resolvedRoot 'MANUAL.md'
        PublicBase = [string]$manifest.public_code_commit
    }
}

function Read-LiteralBlocks {
    param(
        [Parameter(Mandatory)]
        [string]$ManualPath
    )

    $lines = @(Get-Content -LiteralPath $ManualPath)
    $blocks = [System.Collections.Generic.List[object]]::new()
    $seen = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    $pendingMarker = $null
    for ($index = 0; $index -lt $lines.Count; $index++) {
        $line = $lines[$index]
        if ($line -match '^<!-- ficant-manual-literal: ([a-z0-9][a-z0-9-]*) -->$') {
            if ($null -ne $pendingMarker) {
                throw "Manual marker '$pendingMarker' is not followed by a PowerShell block."
            }
            $pendingMarker = $Matches[1]
            if (-not $seen.Add($pendingMarker)) {
                throw "Duplicate manual block id '$pendingMarker'."
            }
            continue
        }
        if ($line -ceq '```powershell') {
            if ($null -eq $pendingMarker) {
                throw "Unmarked PowerShell block at MANUAL line $($index + 1)."
            }
            $body = [System.Collections.Generic.List[string]]::new()
            $closed = $false
            for ($bodyIndex = $index + 1; $bodyIndex -lt $lines.Count; $bodyIndex++) {
                if ($lines[$bodyIndex] -ceq '```') {
                    $closed = $true
                    $index = $bodyIndex
                    break
                }
                $body.Add($lines[$bodyIndex])
            }
            if (-not $closed -or $body.Count -eq 0) {
                throw "Manual block '$pendingMarker' is empty or unterminated."
            }
            $text = $body -join "`n"
            Assert-LiteralBlockSafe -Id $pendingMarker -Text $text
            $blocks.Add([pscustomobject]@{
                Id = $pendingMarker
                Text = $text
            })
            $pendingMarker = $null
            continue
        }
        if ($null -ne $pendingMarker -and -not [string]::IsNullOrWhiteSpace($line)) {
            throw "Manual marker '$pendingMarker' must immediately precede its PowerShell fence."
        }
    }
    if ($null -ne $pendingMarker) {
        throw "Manual marker '$pendingMarker' has no PowerShell block."
    }
    if ($blocks.Count -ne $expectedBlockOrder.Count) {
        throw "Expected $($expectedBlockOrder.Count) literal blocks, found $($blocks.Count)."
    }
    for ($index = 0; $index -lt $expectedBlockOrder.Count; $index++) {
        if ($blocks[$index].Id -cne $expectedBlockOrder[$index]) {
            throw (
                "Manual block order mismatch at index ${index}: expected " +
                "'$($expectedBlockOrder[$index])', found '$($blocks[$index].Id)'."
            )
        }
    }
    return @($blocks)
}

function Assert-LiteralBlockSafe {
    param(
        [Parameter(Mandatory)]
        [string]$Id,

        [Parameter(Mandatory)]
        [string]$Text
    )

    $tokens = $null
    $parseErrors = $null
    [System.Management.Automation.Language.Parser]::ParseInput(
        $Text,
        [ref]$tokens,
        [ref]$parseErrors
    ) | Out-Null
    if ($parseErrors.Count -gt 0) {
        throw "Manual block '$Id' is not valid PowerShell: $($parseErrors[0].Message)"
    }
    if ($Text -match '<[^>]+>') {
        throw "Manual block '$Id' contains a placeholder."
    }
    $forbidden = @(
        '(?im)^\s*git\s+(push|tag|reset)\b',
        '(?im)^\s*gh\s+',
        '(?im)^\s*docker\s+(push|login)\b',
        '(?im)^\s*(ssh|scp|kubectl|helm)\b',
        '(?im)check-release-candidate',
        '(?im)^\s*Remove-Item\b.*\s-Recurse\b',
        '(?im)^\s*docker\s+compose\b.*\sdown\b.*--volumes\b'
    )
    foreach ($pattern in $forbidden) {
        if ($Text -match $pattern) {
            throw "Manual block '$Id' contains a forbidden operation."
        }
    }
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

function Read-EnvironmentFile {
    param(
        [Parameter(Mandatory)]
        [string]$LiteralPath
    )

    $values = @{}
    foreach ($line in Get-Content -LiteralPath $LiteralPath) {
        if ([string]::IsNullOrWhiteSpace($line) -or $line.TrimStart().StartsWith('#')) {
            continue
        }
        $separator = $line.IndexOf('=')
        if ($separator -le 0) {
            throw "Invalid environment entry in '$LiteralPath'."
        }
        $values[$line.Substring(0, $separator)] = $line.Substring($separator + 1)
    }
    return $values
}

function Invoke-LiteralBlock {
    param(
        [Parameter(Mandatory)]
        [object]$Block,

        [Parameter(Mandatory)]
        [string]$Checkout,

        [Parameter(Mandatory)]
        [string]$ScratchRoot
    )

    $blockPath = Join-Path $ScratchRoot ("manual-{0}.ps1" -f $Block.Id)
    [System.IO.File]::WriteAllText(
        $blockPath,
        $Block.Text + "`n",
        [System.Text.UTF8Encoding]::new($false)
    )
    Write-Host "MANUAL literal block: $($Block.Id)"
    Invoke-Native -FilePath 'pwsh' -ArgumentList @(
        '-NoProfile', '-NonInteractive', '-File', $blockPath
    ) -WorkingDirectory $Checkout
}

function Set-IntegrationEnvironment {
    param(
        [Parameter(Mandatory)]
        [string]$Checkout
    )

    $environmentPath = Join-Path $Checkout 'deploy\dev\.env.local'
    $values = Read-EnvironmentFile -LiteralPath $environmentPath
    foreach ($required in @(
        'FICANT_POSTGRES_PASSWORD',
        'FICANT_S3_ACCESS_KEY',
        'FICANT_S3_SECRET_KEY',
        'FICANT_S3_BUCKET'
    )) {
        if (-not $values.ContainsKey($required)) {
            throw "Generated development environment omits '$required'."
        }
    }
    $env:FICANT_TEST_DATABASE_URL = (
        'postgres://ficant:{0}@127.0.0.1:{1}/ficant' -f
        $values['FICANT_POSTGRES_PASSWORD'],
        $env:FICANT_POSTGRES_PORT
    )
    $env:FICANT_TEST_S3_ENDPOINT = "http://127.0.0.1:$env:FICANT_S3_PORT"
    $env:FICANT_TEST_S3_BUCKET = $values['FICANT_S3_BUCKET']
    $env:FICANT_TEST_S3_ACCESS_KEY = $values['FICANT_S3_ACCESS_KEY']
    $env:FICANT_TEST_S3_SECRET_KEY = $values['FICANT_S3_SECRET_KEY']
    $digest = Get-NativeOutput -FilePath 'docker' -ArgumentList @(
        'image', 'inspect', '--format', '{{.Id}}', 'ficant/worker:dev'
    )
    if ($digest -notmatch '^sha256:[0-9a-f]{64}$') {
        throw 'Built Worker image has no canonical config digest.'
    }
    $env:FICANT_TEST_RUNTIME_IMAGE_DIGEST = $digest
}

$scratchRoot = $null
$checkout = $null
$worktreeAdded = $false
$repositoryRoot = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$savedEnvironment = @{}

try {
    $authority = Assert-AuthoritySnapshot -Root $AuthorityRoot -ExpectedCommit $ExpectedAuthorityCommit
    $blocks = Read-LiteralBlocks -ManualPath $authority.ManualPath
    if ($ListOnly) {
        Write-Output "Authority: $ExpectedAuthorityCommit"
        Write-Output "Authority public base: $($authority.PublicBase)"
        foreach ($block in $blocks) {
            Write-Output "[$($block.Id)]"
            Write-Output $block.Text
        }
        exit 0
    }
    if ($ExpectedPublicCommit -notmatch '^[0-9a-f]{40}$') {
        throw '-ExpectedPublicCommit is required for literal execution.'
    }
    $actualPublicCommit = Get-NativeOutput -FilePath 'git' -ArgumentList @(
        '-C', $repositoryRoot, 'rev-parse', 'HEAD'
    )
    if ($actualPublicCommit -cne $ExpectedPublicCommit) {
        throw "Public commit mismatch: expected '$ExpectedPublicCommit', actual '$actualPublicCommit'."
    }
    $publicStatus = Get-NativeOutput -FilePath 'git' -ArgumentList @(
        '-C', $repositoryRoot, 'status', '--porcelain', '--untracked-files=all'
    )
    if (-not [string]::IsNullOrEmpty($publicStatus)) {
        throw 'Public repository must be clean before clean-checkout MANUAL execution.'
    }

    $scratchRoot = Join-Path (
        [System.IO.Path]::GetTempPath()
    ) ("ficant-manual-{0}" -f [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $scratchRoot | Out-Null
    $checkout = Join-Path $scratchRoot 'checkout'
    Invoke-Native -FilePath 'git' -ArgumentList @(
        '-C', $repositoryRoot, 'worktree', 'add', '--detach', $checkout, $ExpectedPublicCommit
    )
    $worktreeAdded = $true

    foreach ($name in @(
        'COMPOSE_PROJECT_NAME',
        'FICANT_POSTGRES_PORT',
        'FICANT_S3_PORT',
        'FICANT_SERVER_PORT',
        'FICANT_WORKER_PORT',
        'FICANT_UI_PORT',
        'FICANT_TEST_DATABASE_URL',
        'FICANT_TEST_S3_ENDPOINT',
        'FICANT_TEST_S3_BUCKET',
        'FICANT_TEST_S3_ACCESS_KEY',
        'FICANT_TEST_S3_SECRET_KEY',
        'FICANT_TEST_RUNTIME_IMAGE_DIGEST'
    )) {
        $savedEnvironment[$name] = [Environment]::GetEnvironmentVariable($name)
    }
    $env:COMPOSE_PROJECT_NAME = "ficant-manual-$($ExpectedPublicCommit.Substring(0, 8))"
    $env:FICANT_POSTGRES_PORT = [string](Get-FreeTcpPort)
    $env:FICANT_S3_PORT = [string](Get-FreeTcpPort)
    $env:FICANT_SERVER_PORT = [string](Get-FreeTcpPort)
    $env:FICANT_WORKER_PORT = [string](Get-FreeTcpPort)
    $env:FICANT_UI_PORT = [string](Get-FreeTcpPort)

    foreach ($block in $blocks) {
        if ($block.Id -ceq 'check-integration') {
            Invoke-Native -FilePath 'pwsh' -ArgumentList @(
                '-NoProfile', '-NonInteractive', '-File',
                (Join-Path $checkout 'scripts\dev-up.ps1')
            ) -WorkingDirectory $checkout
            Set-IntegrationEnvironment -Checkout $checkout
        }
        Invoke-LiteralBlock -Block $block -Checkout $checkout -ScratchRoot $scratchRoot
    }
    Write-Host 'FICANT MANUAL literal clean-checkout execution passed.'
    exit 0
}
catch {
    Write-Error $_
    exit 1
}
finally {
    if ($null -ne $checkout -and (Test-Path -LiteralPath $checkout -PathType Container)) {
        $composeFile = Join-Path $checkout 'deploy\dev\docker-compose.yml'
        $environmentFile = Join-Path $checkout 'deploy\dev\.env.local'
        if ((Test-Path -LiteralPath $composeFile -PathType Leaf) -and
            (Test-Path -LiteralPath $environmentFile -PathType Leaf)) {
            & docker compose `
                --project-directory (Split-Path -Parent $composeFile) `
                --env-file $environmentFile `
                --file $composeFile `
                --profile dev `
                --profile ui `
                down --volumes --remove-orphans
        }
    }
    foreach ($name in $savedEnvironment.Keys) {
        [Environment]::SetEnvironmentVariable($name, $savedEnvironment[$name])
    }
    if ($worktreeAdded) {
        & git -C $repositoryRoot worktree remove --force $checkout
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
        ) -or [System.IO.Path]::GetFileName($resolvedScratch) -notlike 'ficant-manual-*') {
            throw "Refusing to remove unexpected scratch path '$resolvedScratch'."
        }
        Remove-Item -LiteralPath $resolvedScratch -Recurse -Force
    }
}
