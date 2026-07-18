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
    Write-Host 'Network installation: disabled'
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

function Assert-FicantRustCapability {
    Assert-FicantExactVersion -Name 'cargo' -ArgumentList @('--version') -Expected 'cargo 1.96.1 (356927216 2026-06-26)'
    Assert-FicantExactVersion -Name 'rustc' -ArgumentList @('--version') -Expected 'rustc 1.96.1 (31fca3adb 2026-06-26)'
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
