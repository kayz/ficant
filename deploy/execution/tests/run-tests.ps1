# HOQA status: tests for the superseded historical WSL compatibility runner only.
param(
    [string]$RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..\.."))
)

$ErrorActionPreference = "Stop"
$entry = Join-Path $RepositoryRoot "deploy\execution\invoke-wsl.ps1"
$profiles = Join-Path $RepositoryRoot "deploy\execution\profiles.toml"
$capabilities = Join-Path $RepositoryRoot "deploy\execution\environment-capabilities.toml"
$contractSchema = Join-Path $RepositoryRoot "deploy\execution\schemas\contract.schema.json"
$resultSchema = Join-Path $RepositoryRoot "deploy\execution\schemas\result.schema.json"
$wslRunner = Join-Path $RepositoryRoot "deploy\execution\run.sh"
$toolchainSetup = Join-Path $RepositoryRoot "deploy\dev\setup-sit-toolchain.sh"
$contractFixture = Join-Path $RepositoryRoot "deploy\execution\tests\fixtures\contract.json"
$resultFixture = Join-Path $RepositoryRoot "deploy\execution\tests\fixtures\result.json"
$verifiedDiffResultFixture = Join-Path $RepositoryRoot "deploy\execution\tests\fixtures\result-verified-diff.json"
$blockedCandidateResultFixture = Join-Path $RepositoryRoot "deploy\execution\tests\fixtures\result-blocked-with-candidate.json"

foreach ($required in @($entry, $profiles, $capabilities, $contractSchema, $resultSchema, $wslRunner, $toolchainSetup)) {
    if (-not (Test-Path -LiteralPath $required -PathType Leaf)) {
        throw "missing execution artifact: $required"
    }
}

$toolchainSetupText = Get-Content -Raw -LiteralPath $toolchainSetup
foreach ($marker in @('RUSTUP_HOME=/opt/rustup', 'CARGO_HOME=/opt/cargo', 'RUST_TOOLCHAIN_DIR=', 'toolchains/${RUST_VERSION}-x86_64-unknown-linux-gnu', '${RUST_TOOLCHAIN_DIR}/bin/rustc', '${RUST_TOOLCHAIN_DIR}/bin/cargo', '/usr/local/bin/rustc', '/usr/local/bin/cargo')) {
    if (-not $toolchainSetupText.Contains($marker)) {
        throw "toolchain installer must expose locked Rust to the non-root runner identity: $marker"
    }
}
$runnerEnvironmentText = Get-Content -Raw -LiteralPath $wslRunner
if (-not $runnerEnvironmentText.Contains('export RUSTUP_HOME="/opt/rustup"')) {
    throw "WSL runner must use the system Rust toolchain installed by Delivery"
}

# I3-ENV-OMISSION-003: Test Executor commands are runner-managed.  The model
# remains source-read-only while the deterministic runner owns writable scratch
# and durable evidence.
$contractSchemaJson = Get-Content -Raw -LiteralPath $contractSchema | ConvertFrom-Json
if ($contractSchemaJson.properties.schema_version.const -ne 4) {
    throw "test-executor contract must use schema version 4"
}
$testExecutorConditional = $contractSchemaJson.allOf | Where-Object {
    $_.if.properties.permission_profile.const -eq "test-executor"
} | Select-Object -First 1
if ($null -eq $testExecutorConditional) {
    throw "test-executor contract must select runner-managed commands by permission profile"
}
foreach ($command in @("red", "green", "regression")) {
    if ($testExecutorConditional.then.properties.commands.properties.$command.items.'$ref' -ne '#/$defs/structuredCommand') {
        throw "test-executor contract command must resolve to structuredCommand: $command"
    }
}
$structuredCommand = $contractSchemaJson.'$defs'.structuredCommand
if ($structuredCommand.additionalProperties -ne $false) {
    throw "test-executor structuredCommand must be closed"
}
$expectedFields = @("argv", "cwd", "expected_tests", "timeout_seconds")
$actualProperties = @($structuredCommand.properties.PSObject.Properties.Name | Sort-Object)
$actualRequired = @($structuredCommand.required | Sort-Object)
if ((Compare-Object $expectedFields $actualProperties) -or (Compare-Object $expectedFields $actualRequired)) {
    throw "test-executor structuredCommand must define and require exactly argv/cwd/timeout_seconds/expected_tests"
}
if ($structuredCommand.properties.expected_tests.minimum -ne 0) {
    throw "test-executor structuredCommand expected_tests minimum must be zero"
}
foreach ($marker in @('run_managed_test_executor', 'FICANT_EVIDENCE_ROOT', 'source-snapshot', 'original worktree changed during runner-managed execution', 'command-evidence.json')) {
    if (-not $runnerEnvironmentText.Contains($marker)) {
        throw "WSL runner missing Test Executor managed-execution marker: $marker"
    }
}
foreach ($legacyRoot in @('local evidence_dir="/tmp/ficant-execution-evidence/', '"/tmp/ficant-execution-evidence/${run_id}"')) {
    if ($runnerEnvironmentText.Contains($legacyRoot)) {
        throw "runner evidence must survive the CLI sandbox, not use per-CLI /tmp: $legacyRoot"
    }
}
$profilesTextForExecutor = Get-Content -Raw -LiteralPath $profiles
foreach ($marker in @('[managed_executors.test-executor]', 'permission = "test-executor"', 'source_access = "read-only"', 'command_mode = "runner-managed"')) {
    if (-not $profilesTextForExecutor.Contains($marker)) {
        throw "profiles missing runner-managed Test Executor boundary: $marker"
    }
}

$validate = & $entry -Action ValidateConfig 2>&1
if ($LASTEXITCODE -ne 0 -or ($validate -join "`n") -notmatch "EXECUTION_CONFIG_OK") {
    throw "configuration validation failed: $($validate -join "`n")"
}

$fingerprint = & $entry -Action Fingerprint 2>&1
if ($LASTEXITCODE -ne 0) {
    throw "fingerprint failed: $($fingerprint -join "`n")"
}
$fingerprintLine = $fingerprint | Where-Object { $_.ToString().TrimStart().StartsWith("{") } | Select-Object -Last 1
if (-not $fingerprintLine) {
    throw "fingerprint did not emit a JSON object: $($fingerprint -join "`n")"
}
$fingerprintJson = $fingerprintLine | ConvertFrom-Json
foreach ($field in @("fingerprint_id", "captured_at", "distribution", "runner", "tools", "config_hashes", "components")) {
    if ($null -eq $fingerprintJson.$field) {
        throw "fingerprint missing field: $field"
    }
}
$fingerprintAgain = & $entry -Action Fingerprint 2>&1
if ($LASTEXITCODE -ne 0) {
    throw "second fingerprint failed: $($fingerprintAgain -join "`n")"
}
$fingerprintAgainLine = $fingerprintAgain | Where-Object { $_.ToString().TrimStart().StartsWith("{") } | Select-Object -Last 1
$fingerprintAgainJson = $fingerprintAgainLine | ConvertFrom-Json
if ($fingerprintJson.fingerprint_id -ne $fingerprintAgainJson.fingerprint_id) {
    throw "fingerprint identity must not include capture time"
}

$cacheOutput = & $entry -Action PrepareCaches 2>&1
if ($LASTEXITCODE -ne 0) {
    throw "cache preparation failed: $($cacheOutput -join "`n")"
}
$cacheLine = $cacheOutput | Where-Object { $_.ToString().TrimStart().StartsWith("{") } | Select-Object -Last 1
$cacheJson = $cacheLine | ConvertFrom-Json
foreach ($field in @("status", "owner", "cache_root", "build_root", "canary_repo", "distribution")) {
    if ($null -eq $cacheJson.$field) {
        throw "cache preparation missing field: $field"
    }
}
if ($cacheJson.status -ne "ready" -or -not $cacheJson.canary_repo.StartsWith("/")) {
    throw "cache preparation did not return a ready WSL canary"
}

$toolchainOutput = & $entry -Action ToolchainPreflight 2>&1
$toolchainExit = $LASTEXITCODE
$toolchainLine = $toolchainOutput | Where-Object { $_.ToString().TrimStart().StartsWith("{") } | Select-Object -Last 1
if (-not $toolchainLine) {
    throw "toolchain preflight did not emit JSON: $($toolchainOutput -join "`n")"
}
$toolchainJson = $toolchainLine | ConvertFrom-Json
foreach ($field in @("status", "owner", "environment_fingerprint", "checks", "blockers")) {
    if ($null -eq $toolchainJson.$field) {
        throw "toolchain preflight missing field: $field"
    }
}
if (($toolchainJson.status -eq "ready" -and $toolchainExit -ne 0) -or ($toolchainJson.status -eq "blocked" -and $toolchainExit -eq 0)) {
    throw "toolchain preflight status/exit mismatch"
}

$contractValidation = & $entry -Action ValidateContract -Contract $contractFixture 2>&1
if ($LASTEXITCODE -ne 0 -or ($contractValidation -join "`n") -notmatch "EXECUTION_CONTRACT_OK") {
    throw "contract validation failed: $($contractValidation -join "`n")"
}
$staleAdmission = & $entry -Action Run -Contract $contractFixture 2>&1
if ($LASTEXITCODE -eq 0 -or ($staleAdmission -join "`n") -notmatch "model admission fingerprint is stale") {
    throw "stale model admission must fail closed before worktree or model execution: $($staleAdmission -join "`n")"
}
$resultValidation = & $entry -Action ValidateResult -Contract $resultFixture 2>&1
if ($LASTEXITCODE -ne 0 -or ($resultValidation -join "`n") -notmatch "EXECUTION_RESULT_OK") {
    throw "result validation failed: $($resultValidation -join "`n")"
}
$verifiedDiffResultValidation = & $entry -Action ValidateResult -Contract $verifiedDiffResultFixture 2>&1
if ($LASTEXITCODE -ne 0 -or ($verifiedDiffResultValidation -join "`n") -notmatch "EXECUTION_RESULT_OK") {
    throw "verified-diff result validation failed: $($verifiedDiffResultValidation -join "`n")"
}
$blockedCandidateResultValidation = & $entry -Action ValidateResult -Contract $blockedCandidateResultFixture 2>&1
if ($LASTEXITCODE -ne 0 -or ($blockedCandidateResultValidation -join "`n") -notmatch "EXECUTION_RESULT_OK") {
    throw "blocked-with-candidate result validation failed: $($blockedCandidateResultValidation -join "`n")"
}
$integrationGuardFailed = $false
try {
    $integrationGuard = & $entry -Action IntegrateCandidate 2>&1
} catch {
    $integrationGuardFailed = $true
    $integrationGuard = $_.Exception.Message
}
if (-not $integrationGuardFailed -or ($integrationGuard -join "`n") -notmatch "IntegrateCandidate requires -WorktreePath, -ResultPath, and -CommitMessage") {
    throw "stable runner must expose a guarded Orchestrator-only IntegrateCandidate action: $($integrationGuard -join "`n")"
}

$resultSchemaText = Get-Content -Raw -LiteralPath $resultSchema
foreach ($marker in @('candidate_state', 'candidate_tree', 'candidate_diff_sha256', 'candidate.patch', 'verified-diff', 'blocked-with-candidate', 'recovery')) {
    if (-not $resultSchemaText.Contains($marker) -and -not $runnerEnvironmentText.Contains($marker)) {
        throw "verified uncommitted candidate handoff is missing marker: $marker"
    }
}
if ($runnerEnvironmentText.Contains('trusted_blockers.append("worker left uncommitted worktree changes")')) {
    throw "allowlisted verified diff must not be rejected merely because the Worker cannot write Git metadata"
}

$profilesText = Get-Content -Raw -LiteralPath $profiles
$capabilitiesText = Get-Content -Raw -LiteralPath $capabilities
foreach ($marker in @('managed_executors.environment-sit', 'permission_profiles.environment-sit', 'owner = "Delivery"', 'docker_socket = true')) {
    if (-not $profilesText.Contains($marker)) {
        throw "profiles missing Delivery environment executor marker: $marker"
    }
}
foreach ($marker in @('git_metadata_write = false', 'candidate_handoff = "verified-diff"', 'integration_owner = "Orchestrator"', 'fast_correction_cycles = 1', 'medium_correction_cycles = 2', 'strong_correction_cycles = 2', 'preserve_recoverable_candidate = true')) {
    if (-not $profilesText.Contains($marker)) {
        throw "profiles missing Worker/Git integration boundary marker: $marker"
    }
}
foreach ($marker in @('quality_interventions = ["test_contract_freeze", "completed_test_batch"]', 'review_interventions = ["design_freeze", "iteration_exit"]')) {
    if (-not $profilesText.Contains($marker)) {
        throw "profiles missing role intervention cadence marker: $marker"
    }
}
foreach ($marker in @('correction cycle', 'Do not blindly retry', 'blocked-with-candidate', 'recovery')) {
    if (-not $runnerEnvironmentText.Contains($marker)) {
        throw "runner missing bounded self-recovery marker: $marker"
    }
}
foreach ($marker in @('model_invocation_revision = 2', 'test_executor_invocation_revision = 3', 'preflight_canary', 'invalidation')) {
    if (-not $profilesText.Contains($marker) -and -not $capabilitiesText.Contains($marker)) {
        throw "execution configuration missing admission marker: $marker"
    }
}
foreach ($marker in @("container_runtime", "Docker Desktop", "ficant-ubuntu-24.04", "compose_project_template", "human_operator")) {
    if (-not $capabilitiesText.Contains($marker)) {
        throw "capability configuration missing container marker: $marker"
    }
}

$containerOutput = & $entry -Action ContainerPreflight 2>&1
$containerExit = $LASTEXITCODE
$containerLine = $containerOutput | Where-Object { $_.ToString().TrimStart().StartsWith("{") } | Select-Object -Last 1
if (-not $containerLine) {
    throw "container preflight did not emit JSON: $($containerOutput -join "`n")"
}
$containerJson = $containerLine | ConvertFrom-Json
foreach ($field in @("status", "owner", "host_runtime", "distribution", "environment_fingerprint", "checks", "blockers")) {
    if ($null -eq $containerJson.$field) {
        throw "container preflight missing field: $field"
    }
}
if (($containerJson.status -eq "ready" -and $containerExit -ne 0) -or ($containerJson.status -eq "blocked" -and $containerExit -eq 0)) {
    throw "container preflight status/exit mismatch"
}

$scriptText = (Get-Content -Raw -LiteralPath $entry) + (Get-Content -Raw -LiteralPath $wslRunner)
foreach ($marker in @('PrepareWorktree', 'prepare-worktree', 'IntegrateCandidate', 'integrate-candidate', 'WORKTREE_PATH', 'BASE_SHA', 'BRANCH_NAME', 'candidate tree identity mismatch', 'candidate patch identity mismatch')) {
    if (-not $scriptText.Contains($marker)) {
        throw "stable runner is missing WSL-native worktree preparation marker: $marker"
    }
}
if ($scriptText.Contains('worktrees/execution-preflight-') -or -not $scriptText.Contains('preflight-canary')) {
    throw "model preflight must use the WSL canary instead of a full project worktree"
}
foreach ($forbidden in @("C:\git\key", "C:/git/key", "/mnt/c/git/key", "47.100.66.40", "greatquant.com")) {
    if ($scriptText.Contains($forbidden)) {
        throw "runner must not contain UAT or credential locator: $forbidden"
    }
}

foreach ($marker in @('managed_executors.worktree', 'owner = "Orchestrator"', 'wsl-native-git-worktree')) {
    if (-not $profilesText.Contains($marker)) {
        throw "profiles missing Orchestrator worktree executor marker: $marker"
    }
}

$tracked = git -C $RepositoryRoot diff --name-only -- .
if ($LASTEXITCODE -ne 0) {
    throw "unable to inspect repository diff"
}
foreach ($path in $tracked) {
    if ($path -match '^(binaries|crates|cpp|interface|migrations|python|web-dm)/') {
        throw "execution architecture change touched business/source path: $path"
    }
}

Write-Output "EXECUTION_TESTS_OK"
