[CmdletBinding()]
param(
    [switch]$IncludeIntegration,
    [switch]$ListOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'check-common.ps1')

$cppCompiler = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\Llvm\x64\bin\clang++.exe'
$cppBuildDirectory = Join-Path $script:FicantRoot 'build\local-cpp-vs-llvm-19'
$webDirectory = Join-Path $script:FicantRoot 'web-dm'

$steps = @(
    New-FicantCheckStep -Name 'Layering gate' -FilePath 'pwsh' -ArgumentList @('-NoProfile', '-File', 'scripts/check-layering.ps1')
    New-FicantCheckStep -Name 'Layering gate fixture tests' -FilePath 'pwsh' -ArgumentList @('-NoProfile', '-File', 'scripts/test-layering-check.ps1')
    New-FicantCheckStep -Name 'Coverage descriptor gate' -FilePath 'pwsh' -ArgumentList @('-NoProfile', '-NonInteractive', '-File', 'scripts/check-coverage.ps1')
    New-FicantCheckStep -Name 'Coverage gate fixture tests' -FilePath 'pwsh' -ArgumentList @('-NoProfile', '-NonInteractive', '-File', 'scripts/test-coverage-check.ps1')
    New-FicantCheckStep -Name 'Rust formatting' -FilePath 'cargo' -ArgumentList @('fmt', '--all', '--', '--check')
    New-FicantCheckStep -Name 'Rust strict Clippy' -FilePath 'cargo' -ArgumentList @('clippy', '--offline', '--workspace', '--all-targets', '--locked', '--exclude', 'ficant-contracts', '--exclude', 'ficant-contract-tests', '--no-deps', '--', '-D', 'warnings')
    New-FicantCheckStep -Name 'Rust workspace build' -FilePath 'cargo' -ArgumentList @('build', '--offline', '--workspace', '--all-targets', '--locked')
    New-FicantCheckStep -Name 'Rust non-environment tests' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--workspace', '--locked', '--exclude', 'ficant-acceptance', '--exclude', 'ficant-data', '--exclude', 'ficant-storage', '--exclude', 'ficant-contract-tests')
    New-FicantCheckStep -Name 'Rust storage library tests' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-storage', '--lib')
    New-FicantCheckStep -Name 'Rust generated-contract tests' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-contract-tests')
    New-FicantCheckStep -Name 'CGB futures RulePack payload drift' -FilePath 'pwsh' -ArgumentList @('-NoProfile', '-File', 'scripts/generate-cgb-futures-pack.ps1', '-Check')
    New-FicantCheckStep -Name 'R5E CGB interest-tax RulePack payload drift' -FilePath 'pwsh' -ArgumentList @('-NoProfile', '-NonInteractive', '-File', 'scripts/generate-cgb-interest-tax-pack.ps1', '-Check')
    New-FicantCheckStep -Name 'C++ configure' -FilePath 'cmake' -ArgumentList @('-S', 'cpp/fixed-income-kernel', '-B', $cppBuildDirectory, '-G', 'Ninja', "-DCMAKE_CXX_COMPILER=$cppCompiler", '-DCMAKE_BUILD_TYPE=Release')
    New-FicantCheckStep -Name 'C++ build' -FilePath 'cmake' -ArgumentList @('--build', $cppBuildDirectory, '--parallel')
    New-FicantCheckStep -Name 'C++ tests' -FilePath 'ctest' -ArgumentList @('--test-dir', $cppBuildDirectory, '--output-on-failure')
    New-FicantCheckStep -Name 'Acceptance-matrix integrity' -FilePath 'uv' -ArgumentList @('run', '--offline', '--locked', '--project', 'python', 'python', 'tests/iteration-3/verify_acceptance_matrix.py')
    New-FicantCheckStep -Name 'Phase 2B acceptance-matrix integrity' -FilePath 'uv' -ArgumentList @('run', '--offline', '--locked', '--project', 'python', 'python', 'tests/phase2b/verify_acceptance_matrix.py')
    New-FicantCheckStep -Name 'Phase 2C acceptance-matrix integrity' -FilePath 'uv' -ArgumentList @('run', '--offline', '--locked', '--project', 'python', 'python', 'tests/phase2c/verify_acceptance_matrix.py')
    New-FicantCheckStep -Name 'Phase 2C independent Oracle' -FilePath 'uv' -ArgumentList @('run', '--offline', '--locked', '--project', 'python', 'python', '-m', 'pytest', 'tests/oracle/china-rates/test_phase2c_manual_oracle.py', '-q')
    New-FicantCheckStep -Name 'Phase 2C deterministic artifact tests' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-storage', '--test', 'futures_delivery_arrow')
    New-FicantCheckStep -Name 'Phase 2D acceptance-matrix integrity' -FilePath 'uv' -ArgumentList @('run', '--offline', '--locked', '--project', 'python', 'python', 'tests/phase2d/verify_acceptance_matrix.py')
    New-FicantCheckStep -Name 'Phase 2D independent Oracle' -FilePath 'uv' -ArgumentList @('run', '--offline', '--locked', '--project', 'python', 'python', '-m', 'pytest', 'tests/oracle/china-rates/test_phase2d_manual_oracle.py', '-q')
    New-FicantCheckStep -Name 'Phase 2D deterministic artifact tests' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-storage', '--test', 'futures_hedge_arrow')
    New-FicantCheckStep -Name 'R5D independent portfolio KRD Oracle' -FilePath 'uv' -ArgumentList @('run', '--offline', '--locked', '--project', 'python', 'python', '-m', 'pytest', 'tests/oracle/china-rates/test_r5d_portfolio_krd_decimal_oracle.py', '-q')
    New-FicantCheckStep -Name 'R5E independent tax-adjusted Decimal Oracle' -FilePath 'uv' -ArgumentList @('run', '--offline', '--locked', '--project', 'python', 'python', '-m', 'pytest', 'tests/oracle/china-rates/test_r5e_tax_adjusted_decimal_oracle.py', '-q')
    New-FicantCheckStep -Name 'R5D first-party license bindings' -FilePath 'uv' -ArgumentList @('run', '--offline', '--locked', '--project', 'python', 'python', '.github/scripts/verify-license-inventory.py', 'verify-bindings', '--inventory', '.github/scripts/license-inventory.lock.json', '--release-root', '.', '--cargo-lock', 'Cargo.lock', '--uv-lock', 'python/uv.lock', '--pnpm-lock', 'web-dm/pnpm-lock.yaml', '--supply-lock', '.github/scripts/supply-chain.lock.json')
    New-FicantCheckStep -Name 'Python generated-contract tests' -FilePath 'uv' -ArgumentList @('run', '--offline', '--locked', '--project', 'python', 'python', '-m', 'pytest', 'python/tests')
    New-FicantCheckStep -Name 'Phase 2E live Python SDK parity' -FilePath 'pwsh' -ArgumentList @('-NoProfile', '-File', 'scripts/check-phase2e-sdk.ps1')
    New-FicantCheckStep -Name 'Phase 3A canonical data tests' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-data', '--test', 'canonical_ingestion')
    New-FicantCheckStep -Name 'Phase 3B deterministic snapshot codec' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-data', '--test', 'snapshot_codec')
    New-FicantCheckStep -Name 'Web type check' -FilePath 'corepack' -ArgumentList @('pnpm@10.12.4', 'typecheck') -WorkingDirectory $webDirectory
    New-FicantCheckStep -Name 'Web production build' -FilePath 'corepack' -ArgumentList @('pnpm@10.12.4', 'build') -WorkingDirectory $webDirectory
    New-FicantCheckStep -Name 'Web unit and component tests' -FilePath 'corepack' -ArgumentList @('pnpm@10.12.4', 'test', '--', '--run') -WorkingDirectory $webDirectory
)

if ($IncludeIntegration) {
    $steps += @(
        New-FicantCheckStep -Name 'PostgreSQL migration integration' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-storage', '--test', 'migration_acceptance', '--', '--test-threads=1')
        New-FicantCheckStep -Name 'Phase 4C PostgreSQL lease queue integration' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-storage', '--test', 'lease_queue_sit', '--', '--test-threads=1')
        New-FicantCheckStep -Name 'Phase 4 PostgreSQL execution closure integration' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-storage', '--test', 'phase4_execution_sit', '--', '--test-threads=1')
        New-FicantCheckStep -Name 'Phase 4 production worker integration' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-worker', '--features', 'integration-tests', '--test', 'phase4_worker_sit', '--', '--test-threads=1')
        New-FicantCheckStep -Name 'Phase 1 business-loop integration' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-acceptance', '--test', 'phase1_business_loop', '--', '--test-threads=1')
        New-FicantCheckStep -Name 'Negative-invariant integration' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-acceptance', '--test', 'negative_invariants', '--', '--test-threads=1')
        New-FicantCheckStep -Name 'Phase 2B carry-roll integration' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-storage', '--test', 'carry_roll_sit', '--', '--test-threads=1')
        New-FicantCheckStep -Name 'Phase 2C futures-delivery integration' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-storage', '--test', 'futures_delivery_sit', '--', '--test-threads=1')
        New-FicantCheckStep -Name 'Phase 2D futures-hedge integration' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-storage', '--test', 'futures_hedge_sit', '--', '--test-threads=1')
        New-FicantCheckStep -Name 'Phase 3A data-source integration' -FilePath 'pwsh' -ArgumentList @('-NoProfile', '-File', 'scripts/check-phase3a.ps1')
        New-FicantCheckStep -Name 'Phase 3B immutable snapshot integration' -FilePath 'pwsh' -ArgumentList @('-NoProfile', '-File', 'scripts/check-phase3b.ps1')
        New-FicantCheckStep -Name 'R6A governed input-plane production integration' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-server', '--test', 'r6a_governed_input_sit', '--', '--ignored', '--test-threads=1')
        New-FicantCheckStep -Name 'R6B Artifact production topology integration' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-server', '--test', 'r6b_artifact_service_sit', '--', '--ignored', '--test-threads=1')
    )
}

try {
    if ($ListOnly) {
        Show-FicantCheckPlan -Steps $steps
        exit 0
    }

    Assert-FicantRustCapability
    foreach ($command in @('pwsh', 'cmake', 'ctest', 'ninja', 'uv', 'node', 'corepack')) {
        Assert-FicantCommand $command
    }
    Assert-FicantCommand $cppCompiler
    $cppVersionOutput = Get-FicantCommandOutput -FilePath $cppCompiler -ArgumentList @('--version')
    $cppVersion = ($cppVersionOutput -split "`r?`n")[0]
    if ($cppVersion -ne 'clang version 19.1.5') {
        throw "Required C++ compiler is 'clang version 19.1.5', but the active version is '$cppVersion'."
    }
    Assert-FicantExactVersion -Name 'uv' -ArgumentList @('--version') -Expected 'uv 0.7.13 (62ed17b23 2025-06-12)'
    Assert-FicantExactVersion -Name 'node' -ArgumentList @('--version') -Expected 'v22.17.0'

    $bufCommandName = if ([string]::IsNullOrWhiteSpace($env:FICANT_BUF)) {
        'buf'
    }
    else {
        $env:FICANT_BUF
    }
    Assert-FicantExactVersion -Name $bufCommandName -ArgumentList @('--version') -Expected '1.56.0'
    $resolvedBuf = (Get-Command $bufCommandName -ErrorAction Stop).Source

    $previousCorepackNetwork = $env:COREPACK_ENABLE_NETWORK
    $previousFicantBuf = $env:FICANT_BUF
    $env:COREPACK_ENABLE_NETWORK = '0'
    $env:FICANT_BUF = $resolvedBuf
    try {
        Assert-FicantExactVersion -Name 'corepack' -ArgumentList @('pnpm@10.12.4', '--version') -Expected '10.12.4'

        if ($IncludeIntegration) {
            $requiredVariables = @(
                'FICANT_TEST_DATABASE_URL',
                'FICANT_TEST_S3_ENDPOINT',
                'FICANT_TEST_S3_BUCKET',
                'FICANT_TEST_S3_ACCESS_KEY',
                'FICANT_TEST_S3_SECRET_KEY',
                'FICANT_TEST_RUNTIME_IMAGE_DIGEST'
            )
            $missingVariables = @($requiredVariables | Where-Object {
                [string]::IsNullOrWhiteSpace([System.Environment]::GetEnvironmentVariable($_))
            })
            if ($missingVariables.Count -gt 0) {
                throw "Integration checks require a disposable local PostgreSQL/Ceph RGW environment. Missing environment variables: $($missingVariables -join ', ')"
            }
        }

        Invoke-FicantCheckPlan -Steps $steps
    }
    finally {
        $env:COREPACK_ENABLE_NETWORK = $previousCorepackNetwork
        $env:FICANT_BUF = $previousFicantBuf
    }

    Write-Host ''
    Write-Host 'FICANT complete local checks passed.'
    exit 0
}
catch {
    Write-Error $_
    exit 1
}
