[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$sourceInterface = Join-Path $repoRoot 'interface'
$temporaryRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("ficant-r5b-coverage-fixtures-{0}" -f [guid]::NewGuid().ToString('N'))
$utf8 = [System.Text.UTF8Encoding]::new($false)

function New-CoverageFixture {
    param([Parameter(Mandatory)][string]$Name)

    $fixtureRoot = Join-Path $temporaryRoot $Name
    New-Item -ItemType Directory -Path $fixtureRoot -Force | Out-Null
    Copy-Item -LiteralPath $sourceInterface -Destination $fixtureRoot -Recurse
    return (Join-Path $fixtureRoot 'interface')
}

function Set-ProtoText {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$Text
    )

    [System.IO.File]::WriteAllText($Path, $Text, $utf8)
}

function Remove-CoverageField {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$Message,
        [Parameter(Mandatory)][int]$Tag,
        [string]$TypeName = 'CoverageDeclaration'
    )

    $text = [System.IO.File]::ReadAllText($Path)
    $escapedTypeName = [regex]::Escape($TypeName)
    $pattern = "(?s)(message\s+$([regex]::Escape($Message))\s*\{.*?)(\r?\n\s*$escapedTypeName\s+coverage\s*=\s*$Tag;)(.*?\r?\n\})"
    $updated = [regex]::Replace($text, $pattern, '$1$3', 1)
    if ($updated -eq $text) {
        throw "Fixture could not remove $Message.coverage = $Tag from $Path."
    }
    Set-ProtoText -Path $Path -Text $updated
}

function Add-ReachableFixture {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][ValidateSet('Composition', 'ScalarComposition', 'Unknown')][string]$Kind
    )

    $text = [System.IO.File]::ReadAllText($Path)
    if ($Kind -eq 'Composition') {
        $messages = @'
message BarePortfolioAggregate {
  repeated PositionView positions = 1;
}

message GetBarePortfolioAggregateRequest {}

message GetBarePortfolioAggregateResponse {
  oneof result {
    BarePortfolioAggregate aggregate = 1;
    ficant.core.v1.ErrorDetail error = 2;
  }
}

'@
        $method = '  rpc GetBarePortfolioAggregate(GetBarePortfolioAggregateRequest) returns (GetBarePortfolioAggregateResponse);'
    }
    elseif ($Kind -eq 'ScalarComposition') {
        $messages = @'
message BareScalarAggregate {
  ficant.core.v1.DecimalValue aggregate_risk = 1;
}

message GetBareScalarAggregateRequest {}

message GetBareScalarAggregateResponse {
  oneof result {
    BareScalarAggregate aggregate = 1;
    ficant.core.v1.ErrorDetail error = 2;
  }
}

'@
        $method = '  rpc GetBareScalarAggregate(GetBareScalarAggregateRequest) returns (GetBareScalarAggregateResponse);'
    }
    else {
        $messages = @'
message UnclassifiedSuccessPayload {
  string note = 1;
}

message GetUnclassifiedSuccessRequest {}

message GetUnclassifiedSuccessResponse {
  oneof result {
    UnclassifiedSuccessPayload payload = 1;
    ficant.core.v1.ErrorDetail error = 2;
  }
}

'@
        $method = '  rpc GetUnclassifiedSuccess(GetUnclassifiedSuccessRequest) returns (GetUnclassifiedSuccessResponse);'
    }
    $marker = 'service PositionSnapshotService {'
    if (-not $text.Contains($marker)) {
        throw "Fixture could not find PositionSnapshotService in $Path."
    }
    $updated = $text.Replace($marker, "$messages$marker`r`n$method")
    Set-ProtoText -Path $Path -Text $updated
}

function Invoke-CoverageFixture {
    param(
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)][string]$InterfaceRoot,
        [Parameter(Mandatory)][bool]$ShouldPass
    )

    $output = & pwsh -NoProfile -NonInteractive -File (Join-Path $PSScriptRoot 'check-coverage.ps1') -InterfaceRoot $InterfaceRoot 2>&1
    $exitCode = $LASTEXITCODE
    if ($ShouldPass -and $exitCode -ne 0) {
        throw "Coverage fixture '$Name' should pass but exited $exitCode.`n$($output -join [Environment]::NewLine)"
    }
    if (-not $ShouldPass -and $exitCode -eq 0) {
        throw "Coverage fixture '$Name' should fail but exited 0."
    }
    Write-Host "coverage fixture '$Name' produced expected exit code $exitCode"
}

try {
    New-Item -ItemType Directory -Path $temporaryRoot | Out-Null

    $portfolioMissing = New-CoverageFixture -Name 'portfolio-missing'
    Remove-CoverageField -Path (Join-Path $portfolioMissing 'proto\ficant\research\v1\exposure.proto') -Message 'PortfolioKeyRateExposure' -Tag 10
    Invoke-CoverageFixture -Name 'portfolio coverage removed' -InterfaceRoot $portfolioMissing -ShouldPass $false

    $viewsMissing = New-CoverageFixture -Name 'views-missing'
    Remove-CoverageField -Path (Join-Path $viewsMissing 'proto\ficant\research\v1\position.proto') -Message 'PositionViews' -Tag 5
    Invoke-CoverageFixture -Name 'position views coverage removed' -InterfaceRoot $viewsMissing -ShouldPass $false

    $capitalMissing = New-CoverageFixture -Name 'capital-missing'
    Remove-CoverageField -Path (Join-Path $capitalMissing 'proto\ficant\research\v1\position.proto') -Message 'CapitalUse' -Tag 5
    Invoke-CoverageFixture -Name 'capital coverage removed' -InterfaceRoot $capitalMissing -ShouldPass $false

    $healthMissing = New-CoverageFixture -Name 'health-missing'
    Remove-CoverageField -Path (Join-Path $healthMissing 'proto\ficant\research\v1\health.proto') -Message 'DataHealthReport' -Tag 14
    Invoke-CoverageFixture -Name 'data health coverage removed' -InterfaceRoot $healthMissing -ShouldPass $false

    $portfolioOverviewMissing = New-CoverageFixture -Name 'portfolio-overview-missing'
    Remove-CoverageField -Path (Join-Path $portfolioOverviewMissing 'proto\ficant\portfolio\v1\portfolio.proto') -Message 'PortfolioOverview' -Tag 8 -TypeName 'PortfolioCoverage'
    Invoke-CoverageFixture -Name 'PortfolioOverview coverage removed' -InterfaceRoot $portfolioOverviewMissing -ShouldPass $false

    $pageEnvelopeMissing = New-CoverageFixture -Name 'portfolio-page-envelope-missing'
    Remove-CoverageField -Path (Join-Path $pageEnvelopeMissing 'proto\ficant\portfolio\v1\portfolio.proto') -Message 'PortfolioPageEnvelope' -Tag 10 -TypeName 'PortfolioCoverage'
    Invoke-CoverageFixture -Name 'PortfolioPageEnvelope coverage removed' -InterfaceRoot $pageEnvelopeMissing -ShouldPass $false

    $bareComposition = New-CoverageFixture -Name 'bare-composition'
    Add-ReachableFixture -Path (Join-Path $bareComposition 'proto\ficant\research\v1\position.proto') -Kind 'Composition'
    Invoke-CoverageFixture -Name 'new reachable bare composition output' -InterfaceRoot $bareComposition -ShouldPass $false

    $scalarComposition = New-CoverageFixture -Name 'scalar-composition'
    Add-ReachableFixture -Path (Join-Path $scalarComposition 'proto\ficant\research\v1\position.proto') -Kind 'ScalarComposition'
    Invoke-CoverageFixture -Name 'scalar bare composition regression' -InterfaceRoot $scalarComposition -ShouldPass $false

    $unknownSuccess = New-CoverageFixture -Name 'unknown-success'
    Add-ReachableFixture -Path (Join-Path $unknownSuccess 'proto\ficant\research\v1\position.proto') -Kind 'Unknown'
    Invoke-CoverageFixture -Name 'success arm outside the closed inventory' -InterfaceRoot $unknownSuccess -ShouldPass $false

    Invoke-CoverageFixture -Name 'all explicitly classified success arms' -InterfaceRoot $sourceInterface -ShouldPass $true

    Write-Host 'Coverage gate fixture tests passed: 9 real violations fail, including both R8A composition carriers, and the explicitly classified 68/6/62 base inventory passes.'
    exit 0
}
finally {
    if (Test-Path -LiteralPath $temporaryRoot) {
        $resolvedTemporaryRoot = (Resolve-Path -LiteralPath $temporaryRoot).Path
        $systemTemporaryRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd('\')
        if (-not $resolvedTemporaryRoot.StartsWith("$systemTemporaryRoot\ficant-r5b-coverage-fixtures-", [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "Refusing to remove unexpected fixture root: $resolvedTemporaryRoot"
        }
        Remove-Item -LiteralPath $resolvedTemporaryRoot -Recurse -Force
    }
}
