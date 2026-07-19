[CmdletBinding()]
param([string]$RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")))
$ErrorActionPreference = "Stop"
$script:Passed = 0
$script:Skipped = 0
function Assert-True([bool]$Condition, [string]$Message) { if (-not $Condition) { throw $Message }; $script:Passed++ }
function Invoke-Entry([string[]]$Arguments, [hashtable]$Environment = @{}) {
  if (-not $Environment.ContainsKey('FICANT_EVIDENCE_ROOT')) { $Environment['FICANT_EVIDENCE_ROOT'] = 'C:\tmp\ficant-evidence' }
  $saved = @{}
  foreach ($name in $Environment.Keys) { $saved[$name] = [Environment]::GetEnvironmentVariable($name); [Environment]::SetEnvironmentVariable($name, $Environment[$name]) }
  $entryArgs = @('-NoProfile', '-File', $entry) + $Arguments
  try {
    if($Arguments -contains '-TestDoublePath') {
      $parameters=@{}
      for($i=0;$i -lt $Arguments.Count;$i+=2){$parameters[$Arguments[$i].TrimStart('-')]=$Arguments[$i+1]}
      $payload=[Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes(($parameters|ConvertTo-Json -Compress)))
      $harness='& { param($runner,$payload) . $runner; $json=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($payload)); $parameters=$json|ConvertFrom-Json -AsHashtable; Invoke-WindowsWorkerTestAction @parameters }'
      $output=& pwsh -NoProfile -Command $harness $runner $payload 2>&1
    } else {$output = & pwsh @entryArgs 2>&1}
    [pscustomobject]@{ Output = ($output -join "`n"); Exit = $LASTEXITCODE }
  }
  finally { foreach ($name in $Environment.Keys) { [Environment]::SetEnvironmentVariable($name, $saved[$name]) } }
}

$entry = Join-Path $RepositoryRoot "deploy\execution\invoke-worker.ps1"
$runner = Join-Path $RepositoryRoot "deploy\execution\windows-runner.ps1"
$profiles = Join-Path $RepositoryRoot "deploy\execution\profiles.toml"
$capabilities = Join-Path $RepositoryRoot "deploy\execution\environment-capabilities.toml"
$contract = Join-Path $PSScriptRoot "fixtures\windows-contract.json"
$result = Join-Path $PSScriptRoot "fixtures\windows-result.json"
$contractFixtureRoot = Join-Path $env:TEMP "ficant-contract-fixture-$PID-$([guid]::NewGuid().ToString('n'))"
& git -C $RepositoryRoot worktree add --quiet --detach $contractFixtureRoot HEAD
if ($LASTEXITCODE -ne 0) { throw "failed to create linked contract fixture worktree: $contractFixtureRoot" }

function Get-BoundWindowsContract {
  $headOutput = @(& git -C $contractFixtureRoot rev-parse HEAD 2>&1)
  $headExit = $LASTEXITCODE
  if ($headExit -ne 0) { throw "git rev-parse HEAD failed with exit code ${headExit}: $($headOutput -join "`n")" }
  if ($headOutput.Count -ne 1 -or [string]::IsNullOrWhiteSpace([string]$headOutput[0])) { throw 'git rev-parse HEAD returned empty or multiple output' }
  $head = ([string]$headOutput[0]).Trim()
  if ($head -notmatch '^[0-9a-fA-F]{40}$') { throw "git rev-parse HEAD returned an invalid SHA: $head" }

  $bound = Get-Content -Raw -LiteralPath $contract | ConvertFrom-Json
  $bound.base_sha = $head
  $bound.worktree = $contractFixtureRoot
  foreach ($commandGroup in $bound.commands.PSObject.Properties.Name) {
    foreach ($command in @($bound.commands.$commandGroup)) {
      if ($null -ne $command) { $command | Add-Member -NotePropertyName cwd -NotePropertyValue $contractFixtureRoot -Force }
    }
  }
  return $bound
}

try {
Assert-True (Test-Path -LiteralPath $entry) "stable Windows entry is missing"
Assert-True (Test-Path -LiteralPath $runner) "Windows implementation module is missing"
Assert-True (-not((Get-Content -Raw -LiteralPath $entry).Contains('TestDoublePath'))) 'stable Windows entry must not expose arbitrary test-double execution'
Assert-True ((Get-Content -Raw -LiteralPath $entry).Contains("'FixedIncomeWave1'")) 'stable Windows entry must expose the specialized runner-owned Wave 1 action'
$config = Invoke-Entry @("-Action", "ValidateConfig")
Assert-True ($config.Exit -eq 0 -and $config.Output.Contains("WINDOWS_EXECUTION_CONFIG_OK")) "Windows configuration validation failed: $($config.Output)"
$contractTemplate = Get-BoundWindowsContract
$contractPath = Join-Path $env:TEMP "ficant-windows-contract-$([guid]::NewGuid().ToString('n')).json"
try {
  $contractTemplate | ConvertTo-Json -Depth 30 | Set-Content -LiteralPath $contractPath
  $contractCheck = Invoke-Entry @("-Action", "ValidateContract", "-Contract", $contractPath)
  Assert-True ($contractCheck.Exit -eq 0 -and $contractCheck.Output.Contains("WINDOWS_EXECUTION_CONTRACT_OK")) "Windows contract fixture failed"
} finally {
  Remove-Item -LiteralPath $contractPath -Force -ErrorAction SilentlyContinue
}
$resultCheck = Invoke-Entry @("-Action", "ValidateResult", "-ResultPath", $result)
Assert-True ($resultCheck.Exit -eq 0 -and $resultCheck.Output.Contains("WINDOWS_EXECUTION_RESULT_OK")) "Windows result fixture failed"

$text = (Get-Content -Raw -LiteralPath $entry) + (Get-Content -Raw -LiteralPath $runner)
$runnerText = Get-Content -Raw -LiteralPath $runner
$hasWslInvocation = $runnerText.Contains('&wsl') -or $runnerText.Contains('& wsl') -or $runnerText.Contains('&''wsl') -or $runnerText.Contains('&"wsl') -or `
  $runnerText.Contains('wsl.exe') -or `
  $runnerText -match "(?im)^\s*Start-Process\b[^\r\n]*\bwsl(?:\.exe)?\b"
Assert-True (-not $hasWslInvocation) "current runner must not call WSL"
Assert-True ($text.Contains('-c') -and $text.Contains('windows.sandbox="elevated"') -and $text.Contains('--ignore-user-config')) "Codex must explicitly request elevated Windows sandbox configuration"
Assert-True ($runnerText.Contains('--skip-git-repo-check')) "Codex preflight must allow its isolated non-repository canary"
Assert-True ($runnerText.Contains('model preflight failed (exit code') -and $runnerText.Contains('$out.Error.Trim()')) "model preflight failures must preserve CLI exit and stderr context"
Assert-True ($runnerText.Contains("Use only your file-edit operation to write exact text ficant-canary to canary.tmp. Do not use shell commands, run tests, retry, or take unrelated actions.")) "workspace-write preflight must require an edit-only canary without shell, tests, retries, or unrelated actions"
Assert-True ($runnerText.Contains('.TrimEnd("`r","`n")')) "workspace-write canary must normalize terminal CR/LF only"
$terminalNewlineCases = @("ficant-canary", "ficant-canary`n", "ficant-canary`r`n")
Assert-True (-not ($terminalNewlineCases | Where-Object { $_.TrimEnd("`r","`n") -ne 'ficant-canary' })) "workspace-write canary must accept no newline, terminal LF, and terminal CRLF"
$nonTerminalContentCases = @("ficant-canary ", "ficant-canary`t", "ficant`ncanary")
Assert-True (-not ($nonTerminalContentCases | Where-Object { $_.TrimEnd("`r","`n") -eq 'ficant-canary' })) "workspace-write canary must preserve spaces, tabs, and internal newlines"
Assert-True (-not ($text.Contains('C:\git\key') -or $text.Contains('47.100.66.40') -or $text.Contains('greatquant.com'))) "runner contains UAT or credential location"
Assert-True ($text.Contains('System.Diagnostics.Process')) "runner must use System.Diagnostics.Process"
Assert-True ($text.Contains('runner-owned') -and $text.Contains('result')) "runner result-channel ownership marker missing"

$profilesText = Get-Content -Raw -LiteralPath $profiles
$capsText = Get-Content -Raw -LiteralPath $capabilities
Assert-True ($profilesText.Contains('schema_version = 3') -and $capsText.Contains('schema_version = 3')) "configuration must use schema version 3"
Assert-True ($profilesText.Contains('environment = "windows"')) "ordinary profiles must use Windows"
Assert-True ($profilesText.Contains('gpt-5.6-sol') -and $profilesText.Contains('gpt-5.3-codex-spark')) "frozen Codex models missing"
Assert-True ($profilesText.Contains('provider-reported-identity-set-required')) "Claude actual model identity set policy missing"
Assert-True ($profilesText.Contains('sandbox = "read-only"') -and $profilesText.Contains('sandbox = "workspace-write"')) "read/write permissions must remain separate"
Assert-True ($profilesText.Contains('participants = ["Human", "Orchestrator", "Quality", "Audit"]') -and $profilesText.Contains('[judgment.audit]')) "HOQA participants and Audit profile missing"
Assert-True ($profilesText.Contains('specialized_actions = ["fixed-income-wave1"]')) 'Test Executor must declare the exact fixed-income Wave 1 specialized action without widening its generic command catalog'
Assert-True ($capsText.Contains('task-local') -and $capsText.Contains('phase = "test-operate"') -and $capsText.Contains('blocking = false')) "capabilities must be task-local and phase-specific"

$first = Invoke-Entry @("-Action", "CapabilityPreflight", "-RequestedCapabilities", "git,pwsh", "-Profile", "strong", "-Permission", "development")
$second = Invoke-Entry @("-Action", "CapabilityPreflight", "-RequestedCapabilities", "git,pwsh", "-Profile", "strong", "-Permission", "development")
Assert-True ($first.Exit -eq 0 -and $second.Exit -eq 0) "requested local capabilities should pass"
$firstJson = $first.Output | ConvertFrom-Json; $secondJson = $second.Output | ConvertFrom-Json
Assert-True ($firstJson.environment -eq 'windows' -and $firstJson.capability_evidence_id -eq $secondJson.capability_evidence_id) "capability identity must be stable and Windows task-local"
Assert-True ($firstJson.captured_at -ne $secondJson.captured_at -or $firstJson.capability_evidence_id -eq $secondJson.capability_evidence_id) "capture time must not affect identity"
Assert-True ($firstJson.status -eq 'ready' -and $firstJson.blockers.Count -eq 0 -and $firstJson.profile -eq 'strong' -and $firstJson.permission -eq 'development') "capability result must contain ready status and identity dimensions"
$unsupported = Invoke-Entry @("-Action", "CapabilityPreflight", "-RequestedCapabilities", "git,definitely-missing-tool", "-Profile", "fast", "-Permission", "test-author")
$unsupportedJson = $unsupported.Output | ConvertFrom-Json
Assert-True ($unsupported.Exit -ne 0 -and $unsupportedJson.status -eq 'blocked' -and $unsupportedJson.tools.PSObject.Properties.Name -contains 'git' -and $unsupportedJson.tools.PSObject.Properties.Name -notcontains 'pwsh') "capability preflight must check only requested tools and return one blocked JSON result"

$configRoot = Join-Path $env:TEMP "ficant-config-$PID"
Copy-Item -LiteralPath (Split-Path $profiles) -Destination $configRoot -Recurse
try {
  Add-Content -LiteralPath (Join-Path $configRoot 'profiles.toml') -Value "`ninvalid = ["
  $badToml = Invoke-Entry @("-Action", "ValidateConfig", "-ConfigRoot", $configRoot)
  Assert-True ($badToml.Exit -ne 0) "ValidateConfig must reject invalid TOML through a real parser"
  Copy-Item -LiteralPath $profiles -Destination (Join-Path $configRoot 'profiles.toml') -Force
  $schemaObject = Get-Content -Raw -LiteralPath (Join-Path $configRoot 'schemas\contract.schema.json') | ConvertFrom-Json
  $schemaObject.properties | Add-Member -NotePropertyName broken -NotePropertyValue ([pscustomobject]@{ type = 'not-a-json-schema-type' }) -Force
  $schemaObject | ConvertTo-Json -Depth 100 | Set-Content -LiteralPath (Join-Path $configRoot 'schemas\contract.schema.json')
  $badSchema = Invoke-Entry @("-Action", "ValidateConfig", "-ConfigRoot", $configRoot)
  Assert-True ($badSchema.Exit -ne 0) "ValidateConfig must self-validate JSON Schemas"
} finally { Remove-Item -LiteralPath $configRoot -Recurse -Force -ErrorAction SilentlyContinue }

$bad = Get-BoundWindowsContract
$bad.worktree = '/mnt/c/git/ficant'
$badPath = Join-Path $env:TEMP "ficant-bad-windows-contract.json"
$bad | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath $badPath
try { $badCheck = Invoke-Entry @("-Action", "ValidateContract", "-Contract", $badPath); Assert-True ($badCheck.Exit -ne 0) "POSIX worktree must be rejected" } finally { Remove-Item -LiteralPath $badPath -Force -ErrorAction SilentlyContinue }

function Assert-InvalidContract([scriptblock]$Mutate, [string]$Message) {
  $candidate = Get-BoundWindowsContract
  & $Mutate $candidate
  $path = Join-Path $env:TEMP "ficant-contract-$([guid]::NewGuid().ToString('n')).json"
  try { $candidate | ConvertTo-Json -Depth 30 | Set-Content -LiteralPath $path; $check = Invoke-Entry @('-Action','ValidateContract','-Contract',$path); Assert-True ($check.Exit -ne 0) $Message }
  finally { Remove-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue }
}
Assert-InvalidContract { param($c) $c | Add-Member unexpected $true } "contract schema must reject additional properties"
Assert-InvalidContract { param($c) $c.model.requested = 'wrong' } "contract must reject profile/model drift"
Assert-InvalidContract { param($c) $c.commands.green[0].argv = 'pwsh -NoProfile' } "contract must reject shell command strings"
Assert-InvalidContract { param($c) $c.commands.green[0].argv = @('wsl','true') } "contract must reject WSL execution"
Assert-InvalidContract { param($c) $c.result_path = Join-Path $c.worktree 'result.json' } "contract must keep result paths outside worktree"
Assert-InvalidContract { param($c) $c.base_sha = '0000000000000000000000000000000000000000' } "contract must validate exact worktree HEAD/base"
} finally {
  & git -C $RepositoryRoot worktree remove --force $contractFixtureRoot 2>$null
  if ($LASTEXITCODE -ne 0 -and (Test-Path -LiteralPath $contractFixtureRoot)) { throw "failed to remove linked contract fixture worktree: $contractFixtureRoot" }
}

function Assert-InvalidResult([scriptblock]$Mutate, [string]$Message) {
  $candidate = Get-Content -Raw -LiteralPath $result | ConvertFrom-Json
  & $Mutate $candidate
  $path = Join-Path $env:TEMP "ficant-result-$([guid]::NewGuid().ToString('n')).json"
  try { $candidate | ConvertTo-Json -Depth 30 | Set-Content -LiteralPath $path; $check = Invoke-Entry @('-Action','ValidateResult','-ResultPath',$path); Assert-True ($check.Exit -ne 0) $Message }
  finally { Remove-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue }
}
Assert-InvalidResult { param($r) $r.model_identity_source = 'unverified' } "result must reject unverified model identity"
Assert-InvalidResult { param($r) $r.effective_sandbox = 'read-only' } "result must reject sandbox drift"
Assert-InvalidResult { param($r) $r.tests.total = 2 } "result must reject inconsistent test totals"
Assert-InvalidResult { param($r) $r.candidate_sha = '0000000000000000000000000000000000000000' } "result must reject candidate/base inconsistency"
Assert-InvalidResult { param($r) $r.evidence[0].path = 'C:\outside\command.json' } "result must reject paths outside evidence scope"

$doubleRoot = Join-Path $env:TEMP "ficant-runner-double-$PID"
New-Item -ItemType Directory -Path $doubleRoot | Out-Null
try {
  $canary = Invoke-Entry @("-Action", "Preflight", "-Profile", "strong", "-Permission", "development", "-TestDoublePath", $doubleRoot)
  Assert-True ($canary.Exit -eq 0 -and $canary.Output.Contains('workspace-write')) "workspace-write test-double canary failed"
  Assert-True (-not (Test-Path -LiteralPath (Join-Path $doubleRoot 'canary.tmp'))) "workspace-write canary cleanup failed"
  $readCanary = Invoke-Entry @("-Action", "Preflight", "-Profile", "fast", "-Permission", "test-executor", "-TestDoublePath", $doubleRoot)
  Assert-True ($readCanary.Exit -eq 0 -and $readCanary.Output.Contains('read-only')) "read-only test-double canary failed"
  $codexWrite = $canary.Output | ConvertFrom-Json; $codexRead = $readCanary.Output | ConvertFrom-Json
  $claudeWrite = Invoke-Entry @('-Action','Preflight','-Profile','medium','-Permission','development','-TestDoublePath',$doubleRoot)
  $claudeJson = $claudeWrite.Output | ConvertFrom-Json
  Assert-True ($codexWrite.actual_model_identity -eq 'gpt-5.6-sol' -and $codexRead.actual_model_identity -eq 'gpt-5.3-codex-spark' -and $claudeJson.actual_model_identity -eq 'claude-sonnet-test-double') "preflight doubles must return exact model identities"
  Assert-True ($codexWrite.requested_sandbox -eq $codexWrite.effective_sandbox -and $claudeJson.requested_sandbox -eq $claudeJson.effective_sandbox -and -not (Test-Path (Join-Path $doubleRoot 'canary.tmp'))) "preflight must prove sandbox and remove canary state"
} finally { Remove-Item -LiteralPath $doubleRoot -Recurse -Force -ErrorAction SilentlyContinue }

$runRoot = Join-Path $env:TEMP "ficant 路径 run-$PID"
$primaryRoot = Join-Path $env:TEMP "ficant-primary-$PID"
$evidenceRoot = Join-Path $env:TEMP "ficant-evidence-$PID"
New-Item -ItemType Directory -Path $primaryRoot,$evidenceRoot | Out-Null
try {
  & git -C $primaryRoot init --quiet; & git -C $primaryRoot config user.email test@example.invalid; & git -C $primaryRoot config user.name Test
  Set-Content -LiteralPath (Join-Path $primaryRoot 'allowed.txt') -Value 'base'; & git -C $primaryRoot add allowed.txt; & git -C $primaryRoot commit --quiet -m base
  $base = (& git -C $primaryRoot rev-parse HEAD).Trim(); & git -C $primaryRoot worktree add --quiet --detach $runRoot $base
  $runContract = Get-Content -Raw -LiteralPath $contract | ConvertFrom-Json
  $runContract.base_sha = $base; $runContract.worktree = $runRoot; $runContract.allowed_paths = @('allowed.txt'); $runContract.forbidden_paths = @('forbidden.txt')
  $taskEvidence = Join-Path $evidenceRoot $runContract.task_id
  $runContract.result_path = Join-Path $taskEvidence 'result.json'; $runContract.evidence_path = $taskEvidence
  $capabilityEvidencePath = Join-Path (Join-Path $evidenceRoot 'capabilities') ("run-$([guid]::NewGuid().ToString('n')).json")
  $capabilityPreflight = Invoke-Entry @('-Action','CapabilityPreflight','-RequestedCapabilities','git,pwsh','-Profile','strong','-Permission','development','-ResultPath',$capabilityEvidencePath) @{FICANT_EVIDENCE_ROOT=$evidenceRoot}
  Assert-True ($capabilityPreflight.Exit -eq 0) "Run capability preflight must succeed: $($capabilityPreflight.Output)"
  $capabilityEvidence = Get-Content -Raw -LiteralPath $capabilityEvidencePath | ConvertFrom-Json
  $runContract.capability_evidence_path = $capabilityEvidencePath; $runContract.capability_evidence_id = $capabilityEvidence.capability_evidence_id
  $runContract.commands.green = @([pscustomobject]@{ argv=@('pwsh','-NoProfile','-Command','Write-Output "LOCAL_TESTS_OK tests=1"'); cwd=$runRoot; timeout_seconds=30; expected_tests=1 })
  $runContract.commands.regression = @([pscustomobject]@{ argv=@('git','diff','--check'); cwd=$runRoot; timeout_seconds=30; expected_tests=0 })
  $runContractPath = Join-Path $env:TEMP "ficant-run-contract-$PID.json"; $runContract | ConvertTo-Json -Depth 30 | Set-Content $runContractPath
  $double = Join-Path $env:TEMP "ficant-worker-double-$PID.ps1"

  $sensitive = Join-Path $env:TEMP "ficant-sensitive-$PID.txt"
  $outside = Get-Content -Raw -LiteralPath $runContractPath | ConvertFrom-Json; $outside.result_path = $sensitive
  $outsidePath = Join-Path $env:TEMP "ficant-outside-contract-$PID.json"; $outside | ConvertTo-Json -Depth 30 | Set-Content $outsidePath
  $outsideCheck = Invoke-Entry @('-Action','ValidateContract','-Contract',$outsidePath) @{FICANT_EVIDENCE_ROOT=$evidenceRoot}
  Assert-True ($outsideCheck.Exit -ne 0 -and -not(Test-Path -LiteralPath $sensitive)) 'unrelated absolute result paths must be rejected without writes'
  $traversal = Get-Content -Raw -LiteralPath $runContractPath | ConvertFrom-Json; $traversal.evidence_path = Join-Path $evidenceRoot "$($traversal.task_id)\..\escape"; $traversal.result_path = Join-Path $traversal.evidence_path 'result.json'
  $traversalPath = Join-Path $env:TEMP "ficant-traversal-contract-$PID.json"; $traversal | ConvertTo-Json -Depth 30 | Set-Content $traversalPath
  $traversalCheck = Invoke-Entry @('-Action','ValidateContract','-Contract',$traversalPath) @{FICANT_EVIDENCE_ROOT=$evidenceRoot}
  Assert-True ($traversalCheck.Exit -ne 0 -and -not(Test-Path -LiteralPath (Join-Path $evidenceRoot 'escape'))) 'evidence traversal must be rejected without writes'

  $primaryContract = Get-Content -Raw -LiteralPath $runContractPath | ConvertFrom-Json; $primaryContract.worktree=$primaryRoot; $primaryContract.commands.green[0].cwd=$primaryRoot; $primaryContract.commands.regression[0].cwd=$primaryRoot
  $primaryPath=Join-Path $env:TEMP "ficant-primary-contract-$PID.json"; $primaryContract|ConvertTo-Json -Depth 30|Set-Content $primaryPath
  $primaryCheck=Invoke-Entry @('-Action','ValidateContract','-Contract',$primaryPath) @{FICANT_EVIDENCE_ROOT=$evidenceRoot}
  Assert-True ($primaryCheck.Exit -ne 0) 'primary checkout must be rejected'
  $cloneRoot=Join-Path $env:TEMP "ficant-standalone-$PID"; & git clone --quiet $primaryRoot $cloneRoot
  $cloneContract=Get-Content -Raw -LiteralPath $runContractPath|ConvertFrom-Json; $cloneContract.worktree=$cloneRoot; $cloneContract.commands.green[0].cwd=$cloneRoot; $cloneContract.commands.regression[0].cwd=$cloneRoot
  $clonePath=Join-Path $env:TEMP "ficant-clone-contract-$PID.json"; $cloneContract|ConvertTo-Json -Depth 30|Set-Content $clonePath
  $cloneCheck=Invoke-Entry @('-Action','ValidateContract','-Contract',$clonePath) @{FICANT_EVIDENCE_ROOT=$evidenceRoot}
  Assert-True ($cloneCheck.Exit -ne 0) 'standalone clone at the right SHA must be rejected'
  Set-Content -LiteralPath $double -Value 'param([string]$Worktree); Set-Content -LiteralPath (Join-Path $Worktree "allowed.txt") -Value "changed"; [Console]::Out.Write("double raw output")'
  $run = Invoke-Entry @('-Action','Run','-Contract',$runContractPath,'-TestDoublePath',$double) @{FICANT_EVIDENCE_ROOT=$evidenceRoot}
  Assert-True ($run.Exit -eq 0) "Run must succeed: $($run.Output)"
  $runJson = Get-Content -Raw -LiteralPath $runContract.result_path | ConvertFrom-Json
  Assert-True ($run.Exit -eq 0 -and $runJson.status -eq 'ready' -and $runJson.actual_model -eq 'gpt-5.6-sol') "Run must succeed through a local test double"
  Assert-True ($runJson.commands[0].argv.Count -gt 0 -and $runJson.commands[0].cwd -eq $runRoot -and $runJson.commands[0].duration_ms -ge 0 -and $runJson.tests.total -eq 1) "Run must capture argv/cwd/exit/duration/test counts"
  Assert-True ((& git -C $runRoot rev-parse HEAD).Trim() -eq $base -and $runJson.changed_files -contains 'allowed.txt' -and (Test-Path (Join-Path $taskEvidence 'raw-output.txt'))) "Run must retain HEAD and write runner-owned evidence"

  & git -C $runRoot restore --worktree allowed.txt
  $runContract.permission_profile = 'test-executor'
  $runContract.mentor = 'Quality'
  $testExecutorCapabilityPath = Join-Path (Join-Path $evidenceRoot 'capabilities') ("test-executor-$([guid]::NewGuid().ToString('n')).json")
  $testExecutorCapabilityPreflight = Invoke-Entry @('-Action','CapabilityPreflight','-RequestedCapabilities','git,pwsh','-Profile','strong','-Permission','test-executor','-ResultPath',$testExecutorCapabilityPath) @{FICANT_EVIDENCE_ROOT=$evidenceRoot}
  Assert-True ($testExecutorCapabilityPreflight.Exit -eq 0) "Test Executor capability preflight must succeed: $($testExecutorCapabilityPreflight.Output)"
  $testExecutorCapability = Get-Content -Raw -LiteralPath $testExecutorCapabilityPath | ConvertFrom-Json
  $runContract.capability_evidence_path = $testExecutorCapabilityPath
  $runContract.capability_evidence_id = $testExecutorCapability.capability_evidence_id
  $runContract.result_path = Join-Path $taskEvidence 'test-executor-result.json'
  $runContract.commands.green = @([pscustomobject]@{ argv=@('git','rev-parse','HEAD'); cwd=$runRoot; timeout_seconds=30; expected_tests=0 })
  $runContractPath = Join-Path $env:TEMP "ficant-test-executor-contract-$PID.json"; $runContract | ConvertTo-Json -Depth 30 | Set-Content $runContractPath
  Set-Content -LiteralPath $double -Value 'param([string]$Worktree); $brief = [Console]::In.ReadToEnd(); if ($brief -notmatch ''```json'' -or $brief -notmatch ''git-rev-parse-head'' -or $brief -notmatch ''forbidden_paths'' -or $brief -notmatch ''argv'' -or $brief -notmatch ''cwd'' -or $brief -notmatch ''exit_code'' -or $brief -notmatch ''duration_ms'' -or $brief -notmatch ''expected_tests'' -or $brief -notmatch ''observed_tests'') { exit 24 }; [Console]::Out.Write(''brief-after-command-evidence'')'
  $testExecutorRun = Invoke-Entry @('-Action','Run','-Contract',$runContractPath,'-TestDoublePath',$double) @{FICANT_EVIDENCE_ROOT=$evidenceRoot}
  Assert-True ($testExecutorRun.Exit -eq 0) "Test Executor Run must execute commands before its brief step: $($testExecutorRun.Output)"
  $testExecutorJson = Get-Content -Raw -LiteralPath $runContract.result_path | ConvertFrom-Json
  Assert-True ($testExecutorJson.requested_sandbox -eq 'read-only' -and $testExecutorJson.effective_sandbox -eq 'read-only' -and $testExecutorJson.command_executor -eq 'runner-managed') "Test Executor Run must use only the read-only model route"
  Assert-True ($testExecutorJson.commands[0].catalog_identity -eq 'git-rev-parse-head' -and $testExecutorJson.timings.runner_validation_ms -gt 0 -and $testExecutorJson.timings.total_ms -gt 0) 'read-only evidence must identify the catalog entry and report performed validation timing'

  Remove-Item -LiteralPath $runContract.result_path -Force
  Set-Content -LiteralPath $double -Value 'param([string]$Worktree); Set-Content -LiteralPath (Join-Path $Worktree "allowed.txt") -Value "forbidden-test-executor-write"; [Console]::Out.Write("must-not-be-accepted")'
  $writeAttempt = Invoke-Entry @('-Action','Run','-Contract',$runContractPath,'-TestDoublePath',$double) @{FICANT_EVIDENCE_ROOT=$evidenceRoot}
  Assert-True ($writeAttempt.Exit -ne 0 -and -not (Test-Path -LiteralPath $runContract.result_path)) "Test Executor write attempts must fail closed without an accepted result"
  Assert-True ((& git -C $runRoot status --porcelain).Count -eq 0) "Test Executor write attempts must leave the worktree unchanged"

  $runContract.permission_profile = 'quality'
  $runContract.mentor = 'Quality'
  $qualityCapabilityPath = Join-Path (Join-Path $evidenceRoot 'capabilities') ("quality-$([guid]::NewGuid().ToString('n')).json")
  $qualityCapabilityPreflight = Invoke-Entry @('-Action','CapabilityPreflight','-RequestedCapabilities','git,pwsh','-Profile','strong','-Permission','quality','-ResultPath',$qualityCapabilityPath) @{FICANT_EVIDENCE_ROOT=$evidenceRoot}
  Assert-True ($qualityCapabilityPreflight.Exit -eq 0) "Quality capability preflight must succeed: $($qualityCapabilityPreflight.Output)"
  $qualityCapability = Get-Content -Raw -LiteralPath $qualityCapabilityPath | ConvertFrom-Json
  $auditCapabilityPath = Join-Path (Join-Path $evidenceRoot 'capabilities') ("audit-$([guid]::NewGuid().ToString('n')).json")
  $auditCapabilityPreflight = Invoke-Entry @('-Action','CapabilityPreflight','-RequestedCapabilities','git,pwsh','-Profile','strong','-Permission','audit','-ResultPath',$auditCapabilityPath) @{FICANT_EVIDENCE_ROOT=$evidenceRoot}
  Assert-True ($auditCapabilityPreflight.Exit -eq 0) "Audit capability preflight must succeed: $($auditCapabilityPreflight.Output)"
  $runContract.capability_evidence_path = $qualityCapabilityPath
  $runContract.capability_evidence_id = $qualityCapability.capability_evidence_id
  $runContract.result_path = Join-Path $taskEvidence 'quality-result.json'
  $runContractPath = Join-Path $env:TEMP "ficant-quality-contract-$PID.json"; $runContract | ConvertTo-Json -Depth 30 | Set-Content $runContractPath
  Set-Content -LiteralPath $double -Value 'param([string]$Worktree); Set-Content -LiteralPath (Join-Path $Worktree "allowed.txt") -Value "forbidden-quality-write"; [Console]::Out.Write("must-not-be-accepted")'
  $qualityWriteAttempt = Invoke-Entry @('-Action','Run','-Contract',$runContractPath,'-TestDoublePath',$double) @{FICANT_EVIDENCE_ROOT=$evidenceRoot}
  Assert-True ($qualityWriteAttempt.Exit -ne 0 -and -not (Test-Path -LiteralPath $runContract.result_path)) "Quality write attempts must fail closed without an accepted result"
  Assert-True ((& git -C $runRoot status --porcelain).Count -eq 0) "Quality write attempts must leave the worktree unchanged"

  $runContract.result_path=Join-Path $taskEvidence 'quality-command-result.json'
  $runContract.commands.green=@([pscustomobject]@{argv=@('pwsh','-NoProfile','-Command','Set-Content -LiteralPath "command-write.txt" -Value "bad"; Write-Output "LOCAL_TESTS_OK tests=1"');cwd=$runRoot;timeout_seconds=30;expected_tests=1})
  $runContract|ConvertTo-Json -Depth 30|Set-Content $runContractPath
  $modelMarker=Join-Path $env:TEMP "ficant-model-called-$PID.txt"; $escapedModelMarker=$modelMarker.Replace("'","''")
  Set-Content -LiteralPath $double -Value "param([string]`$Worktree); Set-Content -LiteralPath '$escapedModelMarker' -Value called"
  $qualityCommandMutation=Invoke-Entry @('-Action','Run','-Contract',$runContractPath,'-TestDoublePath',$double) @{FICANT_EVIDENCE_ROOT=$evidenceRoot}
  Assert-True ($qualityCommandMutation.Exit -ne 0 -and -not(Test-Path (Join-Path $runRoot 'command-write.txt')) -and -not(Test-Path $modelMarker) -and -not(Test-Path $runContract.result_path)) 'Quality command mutation must be cleaned and rejected before any model brief'

  $runContract.permission_profile='development'
  $runContract.mentor='Orchestrator'
  $runContract.capability_evidence_path=$capabilityEvidencePath
  $runContract.capability_evidence_id=$capabilityEvidence.capability_evidence_id
  $runContract.result_path=Join-Path $taskEvidence 'mixed-result.json'
  $runContract.commands.green[0]=[pscustomobject]@{argv=@('pwsh','-NoProfile','-Command','Write-Output "LOCAL_TESTS_OK tests=1"');cwd=$runRoot;timeout_seconds=30;expected_tests=1}
  $runContract|ConvertTo-Json -Depth 30|Set-Content $runContractPath
  Set-Content -LiteralPath $double -Value 'param([string]$Worktree); Set-Content -LiteralPath (Join-Path $Worktree "allowed.txt") "candidate"; Set-Content -LiteralPath (Join-Path $Worktree "forbidden.txt") "bad"'
  $mixed=Invoke-Entry @('-Action','Run','-Contract',$runContractPath,'-TestDoublePath',$double) @{FICANT_EVIDENCE_ROOT=$evidenceRoot}
  Assert-True ($mixed.Exit -ne 0 -and (Get-Content -Raw (Join-Path $runRoot 'allowed.txt')).Trim() -eq 'candidate' -and -not(Test-Path (Join-Path $runRoot 'forbidden.txt')) -and -not(Test-Path $runContract.result_path)) 'mixed mutations must preserve allowed changes, remove the explicitly forbidden path, and reject the result'
  & git -C $runRoot restore --worktree allowed.txt

  $runContract.allowed_paths=@('deploy/**');$runContract.forbidden_paths=@('deploy/secret.txt');$runContract.result_path=Join-Path $taskEvidence 'deny-precedence-result.json';$runContract|ConvertTo-Json -Depth 30|Set-Content $runContractPath
  Set-Content -LiteralPath $double -Value 'param([string]$Worktree); New-Item -ItemType Directory -Force (Join-Path $Worktree "deploy"),(Join-Path $Worktree "deployment")|Out-Null; Set-Content (Join-Path $Worktree "deploy/kept.txt") kept; Set-Content (Join-Path $Worktree "deploy/secret.txt") secret; Set-Content (Join-Path $Worktree "deployment/escape.txt") escape'
  $denyPrecedence=Invoke-Entry @('-Action','Run','-Contract',$runContractPath,'-TestDoublePath',$double) @{FICANT_EVIDENCE_ROOT=$evidenceRoot}
  Assert-True ($denyPrecedence.Exit -ne 0 -and (Test-Path (Join-Path $runRoot 'deploy/kept.txt')) -and -not(Test-Path (Join-Path $runRoot 'deploy/secret.txt')) -and -not(Test-Path (Join-Path $runRoot 'deployment/escape.txt')) -and -not(Test-Path $runContract.result_path)) 'deny precedence must preserve permitted subtree changes, remove forbidden and sibling-prefix escapes, then reject'
  Remove-Item -LiteralPath (Join-Path $runRoot 'deploy') -Recurse -Force

    $excludePathOutput = @(& git -C $runRoot rev-parse --path-format=absolute --git-path info/exclude)
    if ($LASTEXITCODE -ne 0) {
        throw "git rev-parse --git-path info/exclude failed with exit code $LASTEXITCODE"
    }
    if ($excludePathOutput.Count -ne 1 -or [string]::IsNullOrWhiteSpace($excludePathOutput[0])) {
        throw "git rev-parse --git-path info/exclude returned empty or multiple output"
    }
    $excludePath = $excludePathOutput[0]
    if ($excludePath -notmatch '^[A-Za-z]:[\\/]') {
        throw "git rev-parse --git-path info/exclude did not return a Windows absolute path: $excludePath"
    }
    Add-Content -LiteralPath $excludePath -Value "`nignored.tmp"
  $runContract.allowed_paths=@('allowed.txt');$runContract.forbidden_paths=@('forbidden.txt');$runContract.permission_profile='quality';$runContract.mentor='Quality';$runContract.capability_evidence_path=$qualityCapabilityPath;$runContract.capability_evidence_id=$qualityCapability.capability_evidence_id;$runContract.result_path=Join-Path $taskEvidence 'ignored-readonly-result.json';$runContract|ConvertTo-Json -Depth 30|Set-Content $runContractPath
  Set-Content -LiteralPath $double -Value 'param([string]$Worktree); Set-Content (Join-Path $Worktree "ignored.tmp") ignored'
  $ignoredReadOnly=Invoke-Entry @('-Action','Run','-Contract',$runContractPath,'-TestDoublePath',$double) @{FICANT_EVIDENCE_ROOT=$evidenceRoot}
  Assert-True ($ignoredReadOnly.Exit -ne 0 -and -not(Test-Path (Join-Path $runRoot 'ignored.tmp')) -and -not(Test-Path $runContract.result_path)) 'read-only ignored mutations must be detected, removed, and rejected'
  $runContract.permission_profile='development';$runContract.mentor='Orchestrator';$runContract.capability_evidence_path=$capabilityEvidencePath;$runContract.capability_evidence_id=$capabilityEvidence.capability_evidence_id;$runContract.result_path=Join-Path $taskEvidence 'ignored-write-result.json';$runContract|ConvertTo-Json -Depth 30|Set-Content $runContractPath
  $ignoredWrite=Invoke-Entry @('-Action','Run','-Contract',$runContractPath,'-TestDoublePath',$double) @{FICANT_EVIDENCE_ROOT=$evidenceRoot}
  Assert-True ($ignoredWrite.Exit -ne 0 -and -not(Test-Path (Join-Path $runRoot 'ignored.tmp')) -and -not(Test-Path $runContract.result_path)) 'out-of-allowlist ignored mutations must be detected, removed, and rejected'

  $runContract.result_path=Join-Path $taskEvidence 'cleanup-failure-result.json'; $runContract|ConvertTo-Json -Depth 30|Set-Content $runContractPath
  Set-Content -LiteralPath $double -Value 'param([string]$Worktree); Set-Content -LiteralPath (Join-Path $Worktree "blocked.txt") "bad"'
  $cleanupFailure=Invoke-Entry @('-Action','Run','-Contract',$runContractPath,'-TestDoublePath',$double) @{FICANT_EVIDENCE_ROOT=$evidenceRoot;FICANT_TEST_CLEANUP_FAIL_PATH='blocked.txt'}
  Assert-True ($cleanupFailure.Exit -ne 0 -and $cleanupFailure.Output.Contains('cleanup failure') -and -not(Test-Path $runContract.result_path)) 'cleanup failure must terminate and must not claim an accepted result'
  Remove-Item -LiteralPath (Join-Path $runRoot 'blocked.txt') -Force

  $driftRoot=Join-Path $env:TEMP "ficant-head-drift-$PID-$([guid]::NewGuid().ToString('n'))"
  try {
    & git -C $primaryRoot worktree add --quiet --detach $driftRoot $base
    $driftContract=Get-Content -Raw -LiteralPath $runContractPath|ConvertFrom-Json
    $driftContract.worktree=$driftRoot;$driftContract.allowed_paths=@('committed.txt');$driftContract.result_path=Join-Path $taskEvidence 'head-drift-result.json'
    $driftContract.commands.green[0].cwd=$driftRoot;$driftContract.commands.regression[0].cwd=$driftRoot
    $driftPath=Join-Path $env:TEMP "ficant-head-drift-contract-$PID.json";$driftContract|ConvertTo-Json -Depth 30|Set-Content $driftPath
    Set-Content -LiteralPath $double -Value 'param([string]$Worktree); Set-Content -LiteralPath (Join-Path $Worktree "committed.txt") committed; & git -C $Worktree add committed.txt; & git -C $Worktree -c user.email=test@example.invalid -c user.name=Test commit --quiet -m drift'
    $headDrift=Invoke-Entry @('-Action','Run','-Contract',$driftPath,'-TestDoublePath',$double) @{FICANT_EVIDENCE_ROOT=$evidenceRoot}
    $driftHead=(& git -C $driftRoot rev-parse HEAD).Trim()
    $headDriftMessageMatches = ($headDrift.Output -match '(?s)cleanup.*failure') -and ($headDrift.Output -match '(?s)HEAD/base.*drift.*requires.*disposal.*isolated.*worktree')
    Assert-True ($headDrift.Exit -ne 0 -and $driftHead -ne $base -and -not(Test-Path $driftContract.result_path) -and $headDriftMessageMatches) "HEAD drift must fail without reset, accepted result, or successful cleanup claim; exit=$($headDrift.Exit); base=$base; head=$driftHead; result_exists=$(Test-Path $driftContract.result_path); output=$($headDrift.Output)"
  } finally {
    if(Test-Path -LiteralPath $driftRoot){& git -C $primaryRoot worktree remove --force $driftRoot}
    Remove-Item -LiteralPath $driftPath -Force -ErrorAction SilentlyContinue
  }

  $junctionRunRoot=Join-Path $env:TEMP "ficant-worker-junction-$PID-$([guid]::NewGuid().ToString('n'))"
  $junctionOutside=Join-Path $env:TEMP "ficant-junction-outside-$PID-$([guid]::NewGuid().ToString('n'))"
  $junctionProbe=Join-Path $env:TEMP "ficant-junction-probe-$PID-$([guid]::NewGuid().ToString('n'))"
  try {
    New-Item -ItemType Directory -Path $junctionOutside|Out-Null
    $canaryPath=Join-Path $junctionOutside 'canary.txt';Set-Content -LiteralPath $canaryPath -NoNewline -Value 'outside-canary'
    $junctionAvailable=$true
    try {New-Item -ItemType Junction -Path $junctionProbe -Target $junctionOutside -ErrorAction Stop|Out-Null} catch {$junctionAvailable=$false;$script:Skipped++;Write-Output 'WINDOWS_EXECUTION_TEST_SKIPPED capability=worker-created-junction'}
    if($junctionAvailable){
      Remove-Item -LiteralPath $junctionProbe -Force
      & git -C $primaryRoot worktree add --quiet --detach $junctionRunRoot $base
      $junctionContract=Get-Content -Raw -LiteralPath $runContractPath|ConvertFrom-Json
      $junctionContract.worktree=$junctionRunRoot;$junctionContract.allowed_paths=@('link/**');$junctionContract.result_path=Join-Path $taskEvidence 'junction-result.json'
      $junctionContract.commands.green[0].cwd=$junctionRunRoot;$junctionContract.commands.regression[0].cwd=$junctionRunRoot
      $junctionPath=Join-Path $env:TEMP "ficant-junction-contract-$PID.json";$junctionContract|ConvertTo-Json -Depth 30|Set-Content $junctionPath
      $escapedOutside=$junctionOutside.Replace("'","''")
      Set-Content -LiteralPath $double -Value "param([string]`$Worktree); New-Item -ItemType Junction -Path (Join-Path `$Worktree 'link') -Target '$escapedOutside' -ErrorAction Stop|Out-Null"
      $junctionRun=Invoke-Entry @('-Action','Run','-Contract',$junctionPath,'-TestDoublePath',$double) @{FICANT_EVIDENCE_ROOT=$evidenceRoot}
      Assert-True ($junctionRun.Exit -ne 0 -and -not(Test-Path -LiteralPath (Join-Path $junctionRunRoot 'link')) -and (Get-Content -Raw -LiteralPath $canaryPath) -eq 'outside-canary' -and -not(Test-Path $junctionContract.result_path)) 'post-worker junction must be rejected and removed without traversing or changing its outside canary'
    }
  } finally {
    if(Test-Path -LiteralPath $junctionProbe){Remove-Item -LiteralPath $junctionProbe -Force -ErrorAction SilentlyContinue}
    if(Test-Path -LiteralPath $junctionRunRoot){& git -C $primaryRoot worktree remove --force $junctionRunRoot}
    if(Test-Path -LiteralPath $junctionOutside){Remove-Item -LiteralPath $junctionOutside -Recurse -Force}
    Remove-Item -LiteralPath $junctionPath -Force -ErrorAction SilentlyContinue
  }
} finally {
  $cleanupPaths = @($runRoot,$primaryRoot,$cloneRoot,$evidenceRoot,$runContractPath,$double,$modelMarker,$outsidePath,$traversalPath,$primaryPath,$clonePath)
  $cleanupPaths = @($cleanupPaths | Where-Object { -not [string]::IsNullOrWhiteSpace([string]$_) })
  if ($cleanupPaths.Count -gt 0) { Remove-Item -LiteralPath $cleanupPaths -Recurse -Force -ErrorAction SilentlyContinue }
}

# --- Model identity parsing tests (Get-ClaudeModelIdentity) ---
. $runner

$missingToolName = "ficant-nonexistent-tool-$([guid]::NewGuid().ToString('n'))"
Assert-True ($null -eq (Resolve-Tool $missingToolName)) 'a nonexistent tool must resolve to null without throwing'

foreach ($llvmTool in @('clang','clang++','clang-cl')) {
  $expectedPath = "C:\Program Files\LLVM\bin\$llvmTool.exe"
  if (Test-Path -LiteralPath $expectedPath -PathType Leaf) {
    $resolvedPath = Resolve-Tool $llvmTool
    Assert-True ($null -ne $resolvedPath -and $resolvedPath.Equals($expectedPath,[StringComparison]::OrdinalIgnoreCase)) "installed default LLVM tool must resolve deterministically: $llvmTool"
  }
}
$defaultLlvmRc = 'C:\Program Files\LLVM\bin\llvm-rc.exe'
if (Test-Path -LiteralPath $defaultLlvmRc -PathType Leaf) {
  Assert-True ((Resolve-Tool 'llvm-rc').Equals($defaultLlvmRc,[StringComparison]::OrdinalIgnoreCase)) 'installed LLVM resource compiler must resolve deterministically'
}
$vsLlvmRoot = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\Llvm\x64\bin'
foreach($mapping in @{
  'vs-clang'='clang.exe';'vs-clang++'='clang++.exe';'vs-clang-cl'='clang-cl.exe';'vs-llvm-rc'='llvm-rc.exe'
}.GetEnumerator()){
  $expectedPath=Join-Path $vsLlvmRoot $mapping.Value
  if(Test-Path -LiteralPath $expectedPath -PathType Leaf){
    Assert-True ((Resolve-Tool $mapping.Key).Equals($expectedPath,[StringComparison]::OrdinalIgnoreCase)) "VS LLVM capability must resolve explicitly without changing standalone precedence: $($mapping.Key)"
  }
}
Assert-True ((Resolve-Tool 'clang').Equals('C:\Program Files\LLVM\bin\clang.exe',[StringComparison]::OrdinalIgnoreCase)) 'explicit VS LLVM capabilities must not replace the standalone Clang 18 identity'
$defaultCTest = 'C:\Program Files\CMake\bin\ctest.exe'
if (Test-Path -LiteralPath $defaultCTest -PathType Leaf) {
  Assert-True ((Resolve-Tool 'ctest').Equals($defaultCTest,[StringComparison]::OrdinalIgnoreCase)) 'installed CTest must resolve deterministically with the CMake toolset'
}

$waveDefinition=(Get-Command Invoke-FixedIncomeWave1).Definition
Assert-True (-not($waveDefinition.Contains('Invoke-Preflight') -or $waveDefinition.Contains("'codex'") -or $waveDefinition.Contains("'claude'"))) 'deterministic Wave 1 execution must not invoke an external model'
$invalidWave=Invoke-Entry @('-Action','FixedIncomeWave1','-TaskId','test-wave1','-BaseSha',('0'*40),'-Worktree','C:\missing','-CapabilityEvidencePath','C:\missing-capability.json','-Profile','strong','-Permission','test-executor')
Assert-True ($invalidWave.Exit -ne 0 -and $invalidWave.Output.Contains('requires fast/test-executor')) 'Wave 1 action must fail closed outside its exact read-only Test Executor capability context'

Assert-True (-not(Test-AllowedPath 'deployment/evil.txt' @('deploy/**'))) 'deploy subtree rule must not match a sibling prefix'
Assert-True (Test-AllowedPath 'deploy' @('deploy/**')) 'subtree rule must match its directory root'
Assert-True (-not(Test-AllowedPath 'deploy/secret.txt' @('deploy/**') @('deploy/secret.txt'))) 'forbidden rule must override a broad allow rule'
$overlapContract=[pscustomobject]@{allowed_paths=@('deploy/**');forbidden_paths=@('deploy/secret.txt')}
$overlapAccepted=$true;try{Assert-PathRules $overlapContract}catch{$overlapAccepted=$false}
Assert-True $overlapAccepted 'broad allow plus narrow forbidden must be a valid deny-precedence contract'

$junctionRoot=Join-Path $env:TEMP "ficant-junction-$PID-$([guid]::NewGuid().ToString('n'))"
try {
  $target=Join-Path $junctionRoot 'target';$trusted=Join-Path $junctionRoot 'trusted';New-Item -ItemType Directory -Path $target,$trusted|Out-Null
  $junction=Join-Path $trusted 'link'
  try {New-Item -ItemType Junction -Path $junction -Target $target -ErrorAction Stop|Out-Null} catch {$script:Skipped++;Write-Output 'WINDOWS_EXECUTION_TEST_SKIPPED capability=temporary-junction'}
  if(Test-Path -LiteralPath $junction){$reparseRejected=$false;try{Assert-NoReparsePathComponents (Join-Path $junction 'result.json') $trusted}catch{$reparseRejected=$true};Assert-True $reparseRejected 'physical containment guard must reject a junction component'}
} finally {if(Test-Path -LiteralPath $junctionRoot){Remove-Item -LiteralPath $junctionRoot -Recurse -Force}}

# RED: multiple modelUsage keys sorted deterministically
$multiKey = [pscustomobject]@{modelUsage = [pscustomobject]@{'deepseek-v4-pro[1M]' = [pscustomobject]@{tokens = 100}; 'deepseek-v4-flash' = [pscustomobject]@{tokens = 50}}}
$multiResult = Get-ClaudeModelIdentity $multiKey
Assert-True ($multiResult -eq 'deepseek-v4-flash,deepseek-v4-pro[1M]') "modelUsage keys must sort ordinally; got '$multiResult'"

# RED: single exact key
$singleKey = [pscustomobject]@{modelUsage = [pscustomobject]@{'deepseek-v4-pro[1M]' = [pscustomobject]@{tokens = 100}}}
$singleResult = Get-ClaudeModelIdentity $singleKey
Assert-True ($singleResult -eq 'deepseek-v4-pro[1M]') "single modelUsage key must be returned as-is; got '$singleResult'"

# RED: missing modelUsage rejected
$missing = [pscustomobject]@{result = 'some model text'; other = 1}
$missingThrew = $false
try { Get-ClaudeModelIdentity $missing } catch { $missingThrew = $true }
Assert-True $missingThrew 'missing modelUsage must throw'

# RED: empty modelUsage rejected
$emptyModelUsage = [pscustomobject]@{modelUsage = [pscustomobject]@{}}
$emptyThrew = $false
try { Get-ClaudeModelIdentity $emptyModelUsage } catch { $emptyThrew = $true }
Assert-True $emptyThrew 'empty modelUsage must throw'

# RED: model-authored result property ignored
$withResult = [pscustomobject]@{result = 'claude-sonnet-5-20251001'; modelUsage = [pscustomobject]@{'deepseek-v4-pro[1M]' = [pscustomobject]@{tokens = 100}}}
$resultValue = Get-ClaudeModelIdentity $withResult
Assert-True ($resultValue -eq 'deepseek-v4-pro[1M]') "model-authored result must be ignored; got '$resultValue'"

# --- Regression: helper must use explicit ordinal comparer, not culture-sensitive Sort-Object ---
$funcDef = (Get-Command Get-ClaudeModelIdentity).Definition
Assert-True ($funcDef -match '\[StringComparer\]::Ordinal') "Get-ClaudeModelIdentity must use [StringComparer]::Ordinal for deterministic model identity sort"

# Semantic result mappings must fail independently of schema shape.
Assert-InvalidResult { param($r) $r.executor = 'claude' } 'strong profile must reject Claude executor'
Assert-InvalidResult { param($r) $r.requested_model = 'gpt-5.3-codex-spark' } 'strong profile must reject fast requested model'
Assert-InvalidResult { param($r) $r.permission_profile = 'test-executor' } 'test-executor permission must reject workspace-write sandbox'
Assert-InvalidResult { param($r) $r.command_executor = 'runner-managed' } 'workspace-write permission must reject runner-managed command execution'
Assert-InvalidResult { param($r) $r.environment = 'linux' } 'standalone results must reject non-Windows environment'

# Candidate manifests bind untracked path and bytes, including Unicode and spaces.
$manifestRoot=Join-Path $env:TEMP "ficant-manifest-$PID-$([guid]::NewGuid().ToString('n'))"
try {
  New-Item -ItemType Directory -Path $manifestRoot|Out-Null
  & git -C $manifestRoot init -q
  & git -C $manifestRoot config user.email test@example.invalid
  & git -C $manifestRoot config user.name test
  Set-Content -LiteralPath (Join-Path $manifestRoot 'tracked.txt') -NoNewline -Value base
  & git -C $manifestRoot add tracked.txt; & git -C $manifestRoot commit -qm base
  $untracked=Join-Path $manifestRoot '未跟踪 file.txt'
  Set-Content -LiteralPath $untracked -NoNewline -Value first
  $firstManifest=Get-CandidateManifest $manifestRoot
  Set-Content -LiteralPath $untracked -NoNewline -Value second
  Add-Content -LiteralPath (Join-Path $manifestRoot '.git/info/exclude') -Value "`npermitted.ignored"
  Set-Content -LiteralPath (Join-Path $manifestRoot 'permitted.ignored') -NoNewline -Value bound
  $secondManifest=Get-CandidateManifest $manifestRoot
  Assert-True ($firstManifest.Sha256 -ne $secondManifest.Sha256) 'different untracked content must change candidate digest'
  Assert-True ($secondManifest.Manifest.untracked.Count -eq 2 -and ([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($secondManifest.Manifest.untracked[0].path_utf8_base64)) -eq 'permitted.ignored' -or [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($secondManifest.Manifest.untracked[1].path_utf8_base64)) -eq 'permitted.ignored')) 'candidate manifest must bind ordinary and ignored untracked files'
} finally { if(Test-Path -LiteralPath $manifestRoot){Remove-Item -LiteralPath $manifestRoot -Recurse -Force} }

Write-Output "WINDOWS_EXECUTION_TESTS_OK tests=$script:Passed skipped=$script:Skipped"
