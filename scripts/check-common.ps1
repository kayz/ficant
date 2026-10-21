Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$script:FicantRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))

function New-FicantCheckStep {
    param(
        [Parameter(Mandatory)]
        [string]$Name,

        [Parameter(Mandatory)]
        [string]$FilePath,

        [Parameter(Mandatory)]
        [string[]]$ArgumentList,

        [string]$WorkingDirectory = $script:FicantRoot
    )

    [pscustomobject]@{
        Name = $Name
        FilePath = $FilePath
        ArgumentList = $ArgumentList
        WorkingDirectory = [System.IO.Path]::GetFullPath($WorkingDirectory)
    }
}

function Format-FicantArgument {
    param([Parameter(Mandatory)][string]$Value)

    if ($Value -match '[\s`"]') {
        return '"{0}"' -f $Value.Replace('"', '`"')
    }

    return $Value
}

function Show-FicantCheckPlan {
    param([Parameter(Mandatory)][object[]]$Steps)

    Write-Host ('Repository: {0}' -f $script:FicantRoot)
    Write-Host 'Toolchain: family/minimum version checked; exact versions are pinned only in CI and release'
    Write-Host 'Dependencies: lockfile-bound (--locked); missing packages may be fetched, tools are never installed'
    for ($index = 0; $index -lt $Steps.Count; $index++) {
        $step = $Steps[$index]
        $relativeDirectory = [System.IO.Path]::GetRelativePath($script:FicantRoot, $step.WorkingDirectory)
        if ($relativeDirectory -eq '.') {
            $relativeDirectory = '<repo>'
        }
        $arguments = ($step.ArgumentList | ForEach-Object { Format-FicantArgument $_ }) -join ' '
        Write-Host ('[{0}] {1}' -f ($index + 1), $step.Name)
        Write-Host ('    cwd: {0}' -f $relativeDirectory)
        Write-Host ('    run: {0} {1}' -f $step.FilePath, $arguments)
    }
}

function Assert-FicantCommand {
    param([Parameter(Mandatory)][string]$Name)

    if ($null -eq (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "Required command '$Name' was not found. No dependency will be installed automatically."
    }
}

function Get-FicantCommandOutput {
    param(
        [Parameter(Mandatory)][string]$FilePath,
        [Parameter(Mandatory)][string[]]$ArgumentList
    )

    $output = & $FilePath @ArgumentList 2>&1
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        throw "Version probe failed (${exitCode}): $FilePath $($ArgumentList -join ' ')"
    }
    return (($output | Out-String).Trim())
}

function Assert-FicantExactVersion {
    param(
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)][string[]]$ArgumentList,
        [Parameter(Mandatory)][string]$Expected
    )

    Assert-FicantCommand $Name
    $actual = Get-FicantCommandOutput -FilePath $Name -ArgumentList $ArgumentList
    if ($actual -ne $Expected) {
        throw "Required version for '$Name' is '$Expected', but the active version is '$actual'."
    }
}

function Get-FicantVersionNumber {
    param(
        [Parameter(Mandatory)][string]$Text,
        [Parameter(Mandatory)][string]$Name
    )

    $match = [regex]::Match($Text, '(\d+)\.(\d+)(?:\.(\d+))?')
    if (-not $match.Success) {
        throw "Could not read a version number for '$Name' from '$Text'."
    }
    $patch = if ($match.Groups[3].Success) { [int]$match.Groups[3].Value } else { 0 }
    return [version]::new([int]$match.Groups[1].Value, [int]$match.Groups[2].Value, $patch)
}

# Development machines only need the right toolchain family at or above a minimum version.
# The exact versions that make release artifacts reproducible stay pinned in CI and release
# scripts (`.github/**`, `rust-toolchain.toml`, `packageManager`); see ADR-0020.
function Assert-FicantMinimumVersion {
    param(
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)][string[]]$ArgumentList,
        [Parameter(Mandatory)][version]$Minimum,
        [string]$FilePath,
        [string]$Pinned
    )

    $command = if ([string]::IsNullOrWhiteSpace($FilePath)) { $Name } else { $FilePath }
    Assert-FicantCommand $command
    $output = Get-FicantCommandOutput -FilePath $command -ArgumentList $ArgumentList
    $firstLine = ($output -split "`r?`n")[0].Trim()
    $actual = Get-FicantVersionNumber -Text $firstLine -Name $Name
    if ($actual -lt $Minimum) {
        throw "'$Name' must be at least $Minimum, but the active version is '$firstLine'."
    }
    $note = if ([string]::IsNullOrWhiteSpace($Pinned)) { '' } else { " (CI pin: $Pinned)" }
    Write-Host ("Toolchain {0}: {1}{2}" -f $Name, $firstLine, $note)
    return $actual
}

function Assert-FicantRustCapability {
    [void](Assert-FicantMinimumVersion -Name 'cargo' -ArgumentList @('--version') -Minimum '1.96.0' -Pinned '1.96.1')
    [void](Assert-FicantMinimumVersion -Name 'rustc' -ArgumentList @('--version') -Minimum '1.96.0' -Pinned '1.96.1')
}

function Invoke-FicantCheckPlan {
    param([Parameter(Mandatory)][object[]]$Steps)

    foreach ($step in $Steps) {
        Write-Host ''
        Write-Host ('==> {0}' -f $step.Name)
        Push-Location -LiteralPath $step.WorkingDirectory
        try {
            & $step.FilePath @($step.ArgumentList)
            $exitCode = $LASTEXITCODE
        }
        finally {
            Pop-Location
        }

        if ($exitCode -ne 0) {
            throw "Check failed with exit code ${exitCode}: $($step.Name)"
        }
    }
}
