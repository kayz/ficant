[CmdletBinding()]
param(
    [switch]$ListOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if ($ListOnly) {
    Write-Host 'Stop any local ficant-dev Docker Compose stack if running.'
    exit 0
}

if (Get-Command docker -ErrorAction SilentlyContinue) {
    & docker compose -f deploy/dev/docker-compose.yml down --remove-orphans
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
}
