param([string]$Toolchain = 'nightly', [string[]]$Scenario = @('formatting', 'mixed-dyn', 'correlated-dyn', 'erased-records', 'pointer-alias'), [switch]$VerifyCache, [string]$Features = '', [switch]$Fast, [string]$Checker = '', [string]$CargoTargetDirectory = '')

function Invoke-CheckerProcess($StartInfo, [string]$Log) {
    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $StartInfo
    if (!$process.Start()) { throw 'Cannot start the installed checker.' }
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $stream = [IO.FileStream]::new($Log, [IO.FileMode]::Create, [IO.FileAccess]::Write, [IO.FileShare]::ReadWrite, 4096, [IO.FileOptions]::Asynchronous)
    try {
        $stderr = $process.StandardError.BaseStream.CopyToAsync($stream)
        $process.WaitForExit()
        [void]$stderr.GetAwaiter().GetResult()
        $result = $process.ExitCode
        $stream.Flush()
    } finally {
        $stream.Dispose()
        $process.Dispose()
    }
    [IO.File]::AppendAllText($Log, $stdout.Result)
    $output = [IO.File]::ReadAllText($Log)
    [pscustomobject]@{ ExitCode = $result; Output = $output }
}

$ErrorActionPreference = 'Stop'
$previousToolchain = $env:RUSTUP_TOOLCHAIN
$previousRequired = $env:IRQ_CHECK_REQUIRED_CRATE
$previousDump = $env:IRQ_CHECK_DUMP_DIR
$previousTarget = $env:CARGO_TARGET_DIR
$failed = @()
try {
    $env:RUSTUP_TOOLCHAIN = $Toolchain
    $checker = if ($Checker) { (Resolve-Path -LiteralPath $Checker).Path } else { (Get-Command irq-check -ErrorAction Stop).Source }
    & $checker --self-check
    if ($LASTEXITCODE -ne 0) { throw 'The installed checker cannot load.' }
    $identity = & $checker --version
    if ($LASTEXITCODE -ne 0 -or $identity -notmatch '\b([0-9a-f]{40})\b') { throw 'Cannot read the installed compiler identity.' }
    $compilerCommit = $Matches[1]
    Write-Output "checker=$checker identity=$identity"
    $runId = [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss-fffffff')
    $logDirectory = Join-Path (Join-Path $PSScriptRoot '../target/scenarios') $runId
    New-Item -ItemType Directory -Force -Path $logDirectory | Out-Null
    $env:CARGO_TARGET_DIR = if ($CargoTargetDirectory) { [IO.Path]::GetFullPath($CargoTargetDirectory) } elseif ($Fast) { Join-Path $PSScriptRoot '../target/scenario-fast-cargo' } else { Join-Path $logDirectory 'cargo' }
    foreach ($name in $Scenario) {
        if ($name -notin @('formatting', 'mixed-dyn', 'correlated-dyn', 'erased-records', 'pointer-alias', 'context-growth', 'heap-context', 'leaf-reads', 'batch-frontier', 'read-chain', 'heap-read-chain', 'context-retirement', 'return-widening', 'recursive-bindings')) { throw "Unknown scenario: $name" }
        $env:IRQ_CHECK_REQUIRED_CRATE = $name.Replace('-', '_')
        foreach ($negative in @($false, $true)) {
            $variant = if ($negative) { 'negative' } else { 'safe' }
            $log = Join-Path $logDirectory "$name-$variant.log"
            $env:IRQ_CHECK_DUMP_DIR = Join-Path $logDirectory "$name-$variant-evidence"
            $arguments = @('--cargo', 'build', '--manifest-path', (Join-Path $PSScriptRoot "$name/Cargo.toml"), '-p', $name, '--release', '--target', 'x86_64-unknown-none', '-Zbuild-std=core,alloc,compiler_builtins', '-Zbuild-std-features=compiler-builtins-mem')
            if ($Fast) { $arguments[1] = 'rustc' }
            $featuresForRun = @($Features -split ',' | Where-Object { $_ })
            if ($negative) { $featuresForRun += 'negative' }
            if ($featuresForRun.Count) { $arguments += @('--features', ($featuresForRun -join ',')) }
            if ($Fast) { $arguments += @('--', '-C', "metadata=irq_scenario_$runId") }
            $start = New-Object System.Diagnostics.ProcessStartInfo
            $start.FileName = $checker
            $start.Arguments = ($arguments | ForEach-Object {
                '"' + [regex]::Replace([regex]::Replace($_, '(\\*)"', '$1$1\"'), '(\\+)$', '$1$1') + '"'
            }) -join ' '
            $start.UseShellExecute = $false
            $start.CreateNoWindow = $true
            $start.RedirectStandardOutput = $true
            $start.RedirectStandardError = $true
            $invocation = Invoke-CheckerProcess $start $log
            $result = $invocation.ExitCode
            $output = $invocation.Output
            $expected = switch ($name) {
                'formatting' { 'forbidden_format' }
                'mixed-dyn' { 'Bad' }
                'correlated-dyn' { 'Bad' }
                'erased-records' { 'forbidden_record' }
                'pointer-alias' { 'forbidden_alias' }
                'context-growth' { 'forbidden_growth' }
                'heap-context' { 'forbidden_heap' }
                'leaf-reads' { 'forbidden_leaf' }
                'read-chain' { 'forbidden_leaf' }
                'heap-read-chain' { 'forbidden_heap' }
                'context-retirement' { 'shared' }
                'return-widening' { 'forbidden_return' }
                'recursive-bindings' { 'Forbidden' }
                'batch-frontier' { 'shared' }
            }
            $expectInvalid = $name -eq 'recursive-bindings' -and 'escaping-cut' -in $featuresForRun
            $expectUnknown = $name -eq 'recursive-bindings' -and 'coverage-unknown' -in $featuresForRun
            $expectForbidden = $negative -and !$expectUnknown -and !($name -eq 'recursive-bindings' -and
                ('cut' -in $featuresForRun -or ('stable-cycle' -in $featuresForRun -and 'unbounded' -notin $featuresForRun)))
            $accepted = if ($expectInvalid) {
                $result -ne 0 -and $output.Contains('irq::unreachable(unsafe) requires a block that cannot return') -and $output -notmatch 'internal compiler error|panicked at|error\[E\d+\]'
            } elseif ($expectUnknown) {
                $result -ne 0 -and $output.Contains('interrupt call path cannot be fully checked') -and $output.Contains('the storage address is unknown') -and $output -notmatch 'irq::forbidden|internal compiler error|panicked at|error\[E\d+\]|did not reach a complete result'
            } elseif ($expectForbidden) {
                $result -ne 0 -and $output.Contains($expected) -and $output.Contains('irq::forbidden') -and $output.Contains('forbidden operation is reachable from interrupt context') -and $output -notmatch 'internal compiler error|panicked at|error\[E\d+\]|cannot be fully checked|did not reach a complete result|metadata is missing'
            } else { $result -eq 0 }
            $evidenceFile = Join-Path $env:IRQ_CHECK_DUMP_DIR "$($name.Replace('-', '_')).json"
            if (!$expectInvalid -and (Test-Path -LiteralPath $evidenceFile)) {
                $evidence = Get-Content -Raw -LiteralPath $evidenceFile | ConvertFrom-Json
                $roots = @($evidence.roots | Where-Object { $_.instance -eq 'irq_entry' })
                $outgoing = @{}
                foreach ($edge in $evidence.edges) {
                    $key = "$($edge.caller_context):$($edge.caller)"
                    if (!$outgoing.ContainsKey($key)) { $outgoing[$key] = [System.Collections.Generic.List[object]]::new() }
                    $outgoing[$key].Add($edge)
                }
                $pending = [System.Collections.Generic.Queue[object]]::new()
                foreach ($root in $roots) { $pending.Enqueue(@{ Key = "$($root.context):$($root.instance)"; Panic = $false; Info = $false; Proven = $true }) }
                $visited = [System.Collections.Generic.HashSet[string]]::new()
                $indirect = $false
                $expectedReachable = !$expectForbidden
                $expectedProven = $false
                $requirePanicPath = $name -eq 'recursive-bindings' -and 'panic-format' -in $featuresForRun -and $expectForbidden
                while ($pending.Count) {
                    $visit = $pending.Dequeue()
                    $key = $visit.Key
                    if (!$visited.Add("$key|$($visit.Panic)|$($visit.Info)|$($visit.Proven)")) { continue }
                    foreach ($edge in $outgoing[$key]) {
                        if ($null -eq $edge.callee) { continue }
                        if ($edge.evidence -ne 'bound') { continue }
                        $proven = $visit.Proven -and $edge.path_proven
                        if ($edge.kind -in @('virtual call', 'function-pointer call')) { $indirect = $true }
                        $panic = $visit.Panic -or $edge.callee -eq 'panic_handler'
                        $info = $visit.Info -or $edge.callee -match '^<core::panic::PanicInfo.* as core::fmt::Display>::fmt$'
                        if ($expectForbidden -and $edge.callee.Contains($expected) -and (!$requirePanicPath -or ($panic -and $info))) {
                            $expectedReachable = $true
                            $expectedProven = $expectedProven -or $proven
                        }
                        $pending.Enqueue(@{ Key = "$($edge.callee_context):$($edge.callee)"; Panic = $panic; Info = $info; Proven = $proven })
                    }
                }
                $safeSummary = !$expectForbidden -and @($roots | Where-Object { $_.instance -notin $evidence.safe_summaries }).Count -eq 0
                $accepted = $accepted -and $roots.Count -gt 0 -and ($indirect -or $safeSummary -or $expectUnknown) -and $expectedReachable -and !$evidence.incomplete -and $evidence.compiler -eq $compilerCommit -and $evidence.target -eq 'x86_64-unknown-none' -and 'core' -in $evidence.checked_crates -and $name.Replace('-', '_') -in $evidence.checked_crates
                if ($expectForbidden -and !$expectedProven) {
                    $accepted = $accepted -and $output.Contains('reachability is uncertain: analysis could not determine whether execution reaches this forbidden call')
                }
            } elseif (!$expectInvalid) { $accepted = $false }
            if ($name -eq 'recursive-bindings' -and @($featuresForRun | Where-Object { $_ -in @('coverage-local', 'coverage-write', 'coverage-unknown') }).Count -gt 0) {
                $coverageFile = Join-Path $env:IRQ_CHECK_DUMP_DIR 'recursive_bindings-call-coverage.txt'
                $covered = if ('coverage-write' -in $featuresForRun -or $expectUnknown) { 'false' } else { 'true' }
                $accepted = $accepted -and (Test-Path -LiteralPath $coverageFile) -and (Get-Content -Raw -LiteralPath $coverageFile) -match "(?m)^irq_entry at .*: covered=$covered "
            }
            if ($VerifyCache -and !$negative -and !$expectInvalid -and !$expectUnknown -and $accepted) {
                $evidenceTime = (Get-Item -LiteralPath $evidenceFile).LastWriteTimeUtc
                $evidenceHash = (Get-FileHash -LiteralPath $evidenceFile -Algorithm SHA256).Hash
                $cached = Invoke-CheckerProcess $start (Join-Path $logDirectory "$name-cache.log")
                $cacheAccepted = $cached.ExitCode -eq 0 -and (Get-Item -LiteralPath $evidenceFile).LastWriteTimeUtc -eq $evidenceTime -and (Get-FileHash -LiteralPath $evidenceFile -Algorithm SHA256).Hash -eq $evidenceHash
                Write-Output "$name cache exit=$($cached.ExitCode) accepted=$cacheAccepted"
                $accepted = $accepted -and $cacheAccepted
            }
            Write-Output "$name $variant exit=$result accepted=$accepted log=$log"
            if (!$accepted) { $failed += "$name/$variant" }
        }
    }
} finally {
    $env:RUSTUP_TOOLCHAIN = $previousToolchain
    $env:IRQ_CHECK_REQUIRED_CRATE = $previousRequired
    $env:IRQ_CHECK_DUMP_DIR = $previousDump
    $env:CARGO_TARGET_DIR = $previousTarget
}
if ($failed.Count) { throw "Scenario failures: $($failed -join ', ')" }
