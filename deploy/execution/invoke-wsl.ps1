[CmdletBinding()]
# HOQA status: superseded historical WSL compatibility entry. Ordinary work must use
# deploy/execution/invoke-worker.ps1; this file may run only under an explicit compatibility gate.
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("ValidateConfig", "ValidateContract", "ValidateResult", "Fingerprint", "PrepareCaches", "PrepareWorktree", "IntegrateCandidate", "ToolchainPreflight", "ContainerPreflight", "Preflight", "Run")]
    [string]$Action,

    [ValidateSet("strong", "medium", "fast")]
    [string]$Profile,

    [ValidateSet("test-executor", "test-author", "development", "quality-review", "environment-sit", "release")]
    [string]$Permission,

    [string]$Contract,
    [string]$ResultPath,
    [string]$WorktreePath,
    [string]$BaseSha,
    [string]$BranchName,
    [string]$CommitMessage,
    [string]$Distribution = "ficant-ubuntu-24.04"
)

$ErrorActionPreference = "Stop"
$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\.."))

function ConvertTo-WslPath([string]$WindowsPath) {
    $fullPath = [System.IO.Path]::GetFullPath($WindowsPath)
    if ($fullPath -notmatch '^([A-Za-z]):\\(.*)$') {
        throw "runner only maps absolute Windows drive paths: $fullPath"
    }
    $drive = $Matches[1].ToLowerInvariant()
    $tail = $Matches[2].Replace('\', '/')
    return "/mnt/$drive/$tail"
}

$runner = ConvertTo-WslPath (Join-Path $repoRoot "deploy\execution\run.sh")
$arguments = @("--distribution", $Distribution, "--exec", "bash", $runner)

switch ($Action) {
    "ValidateConfig" {
        $arguments += "validate-config"
    }
    "Fingerprint" {
        $arguments += "fingerprint"
    }
    "PrepareCaches" {
        $arguments += "prepare-caches"
    }
    "PrepareWorktree" {
        if (-not $WorktreePath -or -not $BaseSha -or -not $BranchName) {
            throw "PrepareWorktree requires -WorktreePath, -BaseSha, and -BranchName"
        }
        $arguments += @("prepare-worktree", (ConvertTo-WslPath $WorktreePath), $BranchName, $BaseSha)
    }
    "IntegrateCandidate" {
        if (-not $WorktreePath -or -not $ResultPath -or -not $CommitMessage) {
            throw "IntegrateCandidate requires -WorktreePath, -ResultPath, and -CommitMessage"
        }
        $arguments += @("integrate-candidate", (ConvertTo-WslPath $WorktreePath), (ConvertTo-WslPath $ResultPath), $CommitMessage)
    }
    "ToolchainPreflight" {
        $arguments += "toolchain-preflight"
    }
    "ContainerPreflight" {
        $arguments += "container-preflight"
    }
    "ValidateContract" {
        if (-not $Contract) {
            throw "ValidateContract requires -Contract"
        }
        $arguments += @("validate-contract", (ConvertTo-WslPath $Contract))
    }
    "ValidateResult" {
        if (-not $Contract) {
            throw "ValidateResult requires -Contract"
        }
        $arguments += @("validate-result", (ConvertTo-WslPath $Contract))
    }
    "Preflight" {
        if (-not $Profile -or -not $Permission) {
            throw "Preflight requires -Profile and -Permission"
        }
        if ($Permission -eq "release") {
            throw "release execution is Delivery-only and is not a development preflight"
        }
        $arguments += @("preflight", $Profile, $Permission)
    }
    "Run" {
        if (-not $Contract) {
            throw "Run requires -Contract"
        }
        $arguments += @("run", (ConvertTo-WslPath $Contract))
        if ($ResultPath) {
            $arguments += ConvertTo-WslPath $ResultPath
        }
    }
}

& wsl.exe @arguments
exit $LASTEXITCODE
