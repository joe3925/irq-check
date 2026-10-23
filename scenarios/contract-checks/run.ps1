param([Parameter(Mandatory)][string]$Checker, [string[]]$OnlyFeature)

$ErrorActionPreference = 'Stop'
$checkerPath = (Resolve-Path -LiteralPath $Checker).Path
$runId = [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss-fffffff')
$repo = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../..')).Path
$run = Join-Path $repo "target/contract-checks/$runId"
New-Item -ItemType Directory -Path $run | Out-Null
$settings = @{}
foreach ($name in @('RUSTUP_TOOLCHAIN', 'CARGO_TARGET_DIR', 'IRQ_CHECK_REQUIRED_CRATE', 'IRQ_CHECK_DUMP_DIR')) {
    $settings[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
$cases = @(
    @{ Feature = ''; Expected = 'accepted' }
    @{ Feature = 'local-asm'; Expected = 'accepted' }
    @{ Feature = 'uncertain'; Expected = 'accepted' }
    @{ Feature = 'trusted-condition'; Expected = 'accepted' }
    @{ Feature = 'direct-condition'; Expected = 'accepted' }
    @{ Feature = 'direct-condition-forbidden'; Expected = 'forbidden' }
    @{ Feature = 'unmarked'; Expected = 'unknown' }
    @{ Feature = 'trusted'; Expected = 'unknown' }
    @{ Feature = 'repeated'; Expected = 'unknown' }
    @{ Feature = 'effects'; Expected = 'unknown' }
    @{ Feature = 'returned'; Expected = 'unknown' }
    @{ Feature = 'escape'; Expected = 'unknown' }
    @{ Feature = 'unmarked,uncertain'; Expected = 'forbidden-uncertain' }
    @{ Feature = 'forbidden'; Expected = 'forbidden' }
    @{ Feature = 'early-forbidden'; Expected = 'forbidden' }
    @{ Feature = 'invalid-block'; Expected = 'invalid' }
    @{ Feature = 'invalid-function'; Expected = 'invalid' }
    @{ Feature = 'invalid-argument'; Expected = 'invalid' }
)
if ($OnlyFeature) { $cases = @($cases | Where-Object { $_.Feature -in $OnlyFeature }) }
$records = [Collections.Generic.List[object]]::new()
try {
    $env:RUSTUP_TOOLCHAIN = 'nightly'
    $env:CARGO_TARGET_DIR = Join-Path $repo 'target/contract-checks-cargo'
    $env:IRQ_CHECK_REQUIRED_CRATE = 'contract_checks'
    foreach ($case in $cases) {
        $name = if ($case.Feature) { $case.Feature.Replace(',', '-') } else { 'accepted' }
        $env:IRQ_CHECK_DUMP_DIR = Join-Path $run "$name-evidence"
        $start = [Diagnostics.ProcessStartInfo]::new()
        $start.FileName = $checkerPath
        $start.WorkingDirectory = $repo
        $start.UseShellExecute = $false
        $start.CreateNoWindow = $true
        $start.RedirectStandardOutput = $true
        $start.RedirectStandardError = $true
        $start.Arguments = '--cargo rustc --manifest-path "' + (Join-Path $PSScriptRoot 'Cargo.toml') + '" -p contract-checks --release --target x86_64-unknown-none -Zbuild-std=core,alloc,compiler_builtins -Zbuild-std-features=compiler-builtins-mem'
        if ($case.Feature) { $start.Arguments += ' --features "' + $case.Feature + '"' }
        $start.Arguments += ' -- -C metadata=contract_' + $runId
        $process = [Diagnostics.Process]::new()
        $process.StartInfo = $start
        if (!$process.Start()) { throw 'Cannot start checker' }
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        $process.WaitForExit()
        $exitCode = $process.ExitCode
        $output = $stdout.Result + $stderr.Result
        [IO.File]::WriteAllText((Join-Path $run "$name.log"), $output)
        $process.Dispose()
        $valid = $output -notmatch 'internal compiler error|panicked at|error\[E\d+\]|metadata is missing|did not reach a complete result'
        $accepted = $valid -and $(switch ($case.Expected) {
            'accepted' { $exitCode -eq 0 }
            'unknown' { $exitCode -ne 0 -and $output.Contains('interrupt call path cannot be fully checked') -and !$output.Contains('forbidden operation is reachable') }
            'forbidden' { $exitCode -ne 0 -and $output.Contains('forbidden_contract is marked irq::forbidden') -and $output.Contains('forbidden operation is reachable') }
            'forbidden-uncertain' { $exitCode -ne 0 -and $output.Contains('forbidden_contract is marked irq::forbidden') -and ([regex]::Matches($output, '(?m)^error: forbidden operation is reachable from interrupt context').Count -eq 1) -and $output.Contains('reachability is uncertain: analysis could not determine whether execution reaches this forbidden call') -and $output.Contains('cause of uncertainty:') -and ([regex]::Matches($output, '(?m)^error: interrupt call path cannot be fully checked').Count -eq 2) -and !$output.Contains('the call target and a feasible interrupt path are established') }
            'invalid' { $exitCode -ne 0 -and $output.Contains('irq::allow_incomplete(unsafe) requires an explicit unsafe block') }
        })
        $evidenceFile = Join-Path $env:IRQ_CHECK_DUMP_DIR 'contract_checks.json'
        if ($case.Expected -ne 'invalid') {
            if (!(Test-Path -LiteralPath $evidenceFile)) { $accepted = $false } else {
                $evidence = Get-Content -Raw -LiteralPath $evidenceFile | ConvertFrom-Json
                $accepted = $accepted -and !$evidence.incomplete
                if ($case.Feature -ne 'local-asm') {
                    $accepted = $accepted -and 'contract_leaf' -in $evidence.checked_crates -and 'contract_dependency' -in $evidence.checked_crates
                }
                if ($case.Expected -eq 'accepted') {
                    $gaps = @(Get-Content -Raw -LiteralPath (Join-Path $env:IRQ_CHECK_DUMP_DIR 'contract_checks-accepted-gaps.json') | ConvertFrom-Json)
                    $accepted = $accepted -and $gaps.Count -gt 0 -and 'irq_entry' -notin $evidence.safe_summaries
                    $before = Get-FileHash -LiteralPath $evidenceFile
                    $time = (Get-Item -LiteralPath $evidenceFile).LastWriteTimeUtc
                    $cached = [Diagnostics.Process]::new()
                    $cached.StartInfo = $start
                    if (!$cached.Start()) { throw 'Cannot start cache check' }
                    $cachedOut = $cached.StandardOutput.ReadToEndAsync()
                    $cachedErr = $cached.StandardError.ReadToEndAsync()
                    $cached.WaitForExit()
                    [IO.File]::WriteAllText((Join-Path $run "$name-cache.log"), $cachedOut.Result + $cachedErr.Result)
                    $accepted = $accepted -and $cached.ExitCode -eq 0 -and (Get-FileHash -LiteralPath $evidenceFile).Hash -eq $before.Hash -and (Get-Item -LiteralPath $evidenceFile).LastWriteTimeUtc -eq $time
                    $cached.Dispose()
                }
            }
        }
        $records.Add(@{ feature = $case.Feature; expected = $case.Expected; accepted = $accepted; exit_code = $exitCode })
        Write-Output "$name expected=$($case.Expected) accepted=$accepted log=$run\$name.log"
    }
} finally {
    foreach ($name in $settings.Keys) { [Environment]::SetEnvironmentVariable($name, $settings[$name], 'Process') }
    $records | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $run 'results.json')
}
if (@($records | Where-Object { !$_.accepted }).Count) { throw "Contract cases failed: $run" }
