[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('ValidateConfig','CapabilityPreflight','ValidateContract','ValidateResult','Preflight','Run','FixedIncomeWave1')][string]$Action,
    [string]$Contract,
    [string]$ResultPath,
    [string]$RequestedCapabilities,
    [ValidateSet('strong','medium','fast')][string]$Profile = 'strong',
    [ValidateSet('test-executor','test-author','development','quality','audit')][string]$Permission = 'development',
    [string]$ConfigRoot,
    [string]$TaskId,
    [string]$BaseSha,
    [string]$Worktree,
    [string]$CapabilityEvidencePath
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'windows-runner.ps1')
try {
    Invoke-WindowsWorkerAction @PSBoundParameters
} catch {
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
}
