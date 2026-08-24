[CmdletBinding()]
param(
    [string]$DatabaseUrl = $env:FICANT_EXPERIMENT_DATABASE_URL,
    [string]$S3Endpoint = $env:FICANT_EXPERIMENT_S3_ENDPOINT,
    [string]$S3Bucket = $env:FICANT_EXPERIMENT_S3_BUCKET,
    [string]$S3AccessKey = $env:FICANT_EXPERIMENT_S3_ACCESS_KEY,
    [string]$S3SecretKey = $env:FICANT_EXPERIMENT_S3_SECRET_KEY,
    [string]$CatalogFixturePath = (Join-Path $PSScriptRoot '..\tests\fixtures\portfolio\catalog-p0.json'),
    [string]$PerformanceFixturePath = (Join-Path $PSScriptRoot '..\tests\fixtures\portfolio\performance\r8b-performance.json')
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Read-EnvironmentFile {
    param([Parameter(Mandatory)][string]$LiteralPath)

    $values = @{}
    foreach ($line in Get-Content -LiteralPath $LiteralPath -ErrorAction Stop) {
        if ([string]::IsNullOrWhiteSpace($line) -or $line.TrimStart().StartsWith('#')) {
            continue
        }
        $separator = $line.IndexOf('=')
        if ($separator -le 0) {
            throw "Invalid local environment entry in $LiteralPath."
        }
        $name = $line.Substring(0, $separator)
        if ($values.ContainsKey($name)) {
            throw "Duplicate local environment key '$name' in $LiteralPath."
        }
        $values[$name] = $line.Substring($separator + 1)
    }
    return $values
}

$localEnvironmentPath = Join-Path $PSScriptRoot '..\deploy\dev\.env.local'
if (Test-Path -LiteralPath $localEnvironmentPath -PathType Leaf) {
    $localEnvironment = Read-EnvironmentFile -LiteralPath $localEnvironmentPath
    if ([string]::IsNullOrWhiteSpace($DatabaseUrl) -and $localEnvironment.ContainsKey('FICANT_POSTGRES_PASSWORD')) {
        $encodedPassword = [uri]::EscapeDataString($localEnvironment['FICANT_POSTGRES_PASSWORD'])
        $postgresPort = if ($env:FICANT_POSTGRES_PORT) { $env:FICANT_POSTGRES_PORT } else { '15432' }
        $DatabaseUrl = "postgres://ficant:$encodedPassword@127.0.0.1:$postgresPort/ficant"
    }
    if ([string]::IsNullOrWhiteSpace($S3Endpoint)) {
        $s3Port = if ($env:FICANT_S3_PORT) { $env:FICANT_S3_PORT } else { '19000' }
        $S3Endpoint = "http://127.0.0.1:$s3Port"
    }
    if ([string]::IsNullOrWhiteSpace($S3Bucket) -and $localEnvironment.ContainsKey('FICANT_S3_BUCKET')) {
        $S3Bucket = $localEnvironment['FICANT_S3_BUCKET']
    }
    if ([string]::IsNullOrWhiteSpace($S3AccessKey) -and $localEnvironment.ContainsKey('FICANT_S3_ACCESS_KEY')) {
        $S3AccessKey = $localEnvironment['FICANT_S3_ACCESS_KEY']
    }
    if ([string]::IsNullOrWhiteSpace($S3SecretKey) -and $localEnvironment.ContainsKey('FICANT_S3_SECRET_KEY')) {
        $S3SecretKey = $localEnvironment['FICANT_S3_SECRET_KEY']
    }
}

if ([string]::IsNullOrWhiteSpace($DatabaseUrl)) {
    throw 'DatabaseUrl or FICANT_EXPERIMENT_DATABASE_URL is required.'
}
foreach ($required in @{
    S3Endpoint = $S3Endpoint
    S3Bucket = $S3Bucket
    S3AccessKey = $S3AccessKey
    S3SecretKey = $S3SecretKey
}.GetEnumerator()) {
    if ([string]::IsNullOrWhiteSpace($required.Value)) {
        throw "$($required.Key) or its FICANT_EXPERIMENT_S3_* environment value is required."
    }
}

$resolvedCatalogFixture = (Resolve-Path -LiteralPath $CatalogFixturePath).Path
$resolvedPerformanceFixture = (Resolve-Path -LiteralPath $PerformanceFixturePath).Path
$repositoryRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$portfolioBootstrap = Join-Path $PSScriptRoot 'bootstrap-portfolio-p0.ps1'

& $portfolioBootstrap `
    -DatabaseUrl $DatabaseUrl `
    -S3Endpoint $S3Endpoint `
    -S3Bucket $S3Bucket `
    -S3AccessKey $S3AccessKey `
    -S3SecretKey $S3SecretKey `
    -FixturePath $resolvedCatalogFixture
if ($LASTEXITCODE -ne 0) {
    throw "R8A Portfolio prerequisite bootstrap failed with exit code $LASTEXITCODE."
}

$previousBootstrapEnvironment = @{
    FICANT_BOOTSTRAP_DATABASE_URL = $env:FICANT_BOOTSTRAP_DATABASE_URL
    FICANT_BOOTSTRAP_S3_ENDPOINT = $env:FICANT_BOOTSTRAP_S3_ENDPOINT
    FICANT_BOOTSTRAP_S3_BUCKET = $env:FICANT_BOOTSTRAP_S3_BUCKET
    FICANT_BOOTSTRAP_S3_ACCESS_KEY = $env:FICANT_BOOTSTRAP_S3_ACCESS_KEY
    FICANT_BOOTSTRAP_S3_SECRET_KEY = $env:FICANT_BOOTSTRAP_S3_SECRET_KEY
}
$env:FICANT_BOOTSTRAP_DATABASE_URL = $DatabaseUrl
$env:FICANT_BOOTSTRAP_S3_ENDPOINT = $S3Endpoint
$env:FICANT_BOOTSTRAP_S3_BUCKET = $S3Bucket
$env:FICANT_BOOTSTRAP_S3_ACCESS_KEY = $S3AccessKey
$env:FICANT_BOOTSTRAP_S3_SECRET_KEY = $S3SecretKey

Push-Location -LiteralPath $repositoryRoot
try {
    & cargo run --locked --offline -p ficant-server --example r8b_portfolio_performance_bootstrap -- `
        --fixture $resolvedPerformanceFixture
    if ($LASTEXITCODE -ne 0) {
        throw "R8B Portfolio performance bootstrap failed with exit code $LASTEXITCODE."
    }
}
finally {
    Pop-Location
    foreach ($entry in $previousBootstrapEnvironment.GetEnumerator()) {
        if ($null -eq $entry.Value) {
            Remove-Item -LiteralPath "Env:$($entry.Key)" -ErrorAction SilentlyContinue
        }
        else {
            Set-Item -LiteralPath "Env:$($entry.Key)" -Value $entry.Value
        }
    }
}
