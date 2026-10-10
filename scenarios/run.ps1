param([string]$Toolchain = 'nightly', [string[]]$Scenario = @('formatting', 'mixed-dyn', 'correlated-dyn', 'erased-records', 'pointer-alias'), [switch]$VerifyCache)

function Invoke-CheckerProcess($StartInfo, [string]$Log) {
    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $StartInfo
    if (!$process.Start()) { throw 'Cannot start the installed checker.' }
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $stderr = $process.StandardError.ReadToEndAsync()
    $process.WaitForExit()
    $result = $process.ExitCode
    $output = $stdout.Result + $stderr.Result
    [System.IO.File]::WriteAllText($Log, $output)
    $process.Dispose()
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
    $checker = (Get-Command irq-check -ErrorAction Stop).Source
    & $checker --self-check
    if ($LASTEXITCODE -ne 0) { throw 'The installed checker cannot load.' }
    $identity = & $checker --version
    if ($LASTEXITCODE -ne 0 -or $identity -notmatch '\b([0-9a-f]{40})\b') { throw 'Cannot read the installed compiler identity.' }
    $compilerCommit = $Matches[1]
    Write-Output "checker=$checker identity=$identity"
    $runId = [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss-fffffff')
    $logDirectory = Join-Path (Join-Path $PSScriptRoot '../target/scenarios') $runId
    New-Item -ItemType Directory -Force -Path $logDirectory | Out-Null
    $env:CARGO_TARGET_DIR = Join-Path $logDirectory 'cargo'
    foreach ($name in $Scenario) {
        if ($name -notin @('formatting', 'mixed-dyn', 'correlated-dyn', 'erased-records', 'pointer-alias')) { throw "Unknown scenario: $name" }
        $env:IRQ_CHECK_REQUIRED_CRATE = $name.Replace('-', '_')
        foreach ($negative in @($false, $true)) {
            $variant = if ($negative) { 'negative' } else { 'safe' }
            $log = Join-Path $logDirectory "$name-$variant.log"
            $env:IRQ_CHECK_DUMP_DIR = Join-Path $logDirectory "$name-$variant-evidence"
            $arguments = @('--cargo', 'build', '--manifest-path', (Join-Path $PSScriptRoot "$name/Cargo.toml"), '-p', $name, '--release', '--target', 'x86_64-unknown-none', '-Zbuild-std=core,alloc,compiler_builtins', '-Zbuild-std-features=compiler-builtins-mem')
            if ($negative) { $arguments += @('--features', 'negative') }
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
            }
            $accepted = if ($negative) {
                $result -ne 0 -and $output.Contains($expected) -and $output.Contains('irq::forbidden') -and $output -notmatch 'internal compiler error|panicked at|error\[E\d+\]|cannot be fully checked|did not reach a complete result|metadata is missing'
            } else { $result -eq 0 }
            $evidenceFile = Join-Path $env:IRQ_CHECK_DUMP_DIR "$($name.Replace('-', '_')).json"
            if (Test-Path -LiteralPath $evidenceFile) {
                $evidence = Get-Content -Raw -LiteralPath $evidenceFile | ConvertFrom-Json
                $roots = @($evidence.roots | Where-Object { $_.instance -eq 'irq_entry' })
                $outgoing = @{}
                foreach ($edge in $evidence.edges) {
                    $key = "$($edge.caller_context):$($edge.caller)"
                    if (!$outgoing.ContainsKey($key)) { $outgoing[$key] = [System.Collections.Generic.List[object]]::new() }
                    $outgoing[$key].Add($edge)
                }
                $pending = [System.Collections.Generic.Queue[string]]::new()
                foreach ($root in $roots) { $pending.Enqueue("$($root.context):$($root.instance)") }
                $visited = [System.Collections.Generic.HashSet[string]]::new()
                $indirect = $false
                $expectedReachable = !$negative
                while ($pending.Count) {
                    $key = $pending.Dequeue()
                    if (!$visited.Add($key)) { continue }
                    foreach ($edge in $outgoing[$key]) {
                        if ($null -eq $edge.callee) { continue }
                        if ($edge.kind -in @('virtual call', 'function-pointer call')) { $indirect = $true }
                        if ($negative -and $edge.callee.Contains($expected)) { $expectedReachable = $true }
                        $pending.Enqueue("$($edge.callee_context):$($edge.callee)")
                    }
                }
                $accepted = $accepted -and $roots.Count -gt 0 -and $indirect -and $expectedReachable -and !$evidence.incomplete -and $evidence.compiler -eq $compilerCommit -and $evidence.target -eq 'x86_64-unknown-none' -and 'core' -in $evidence.checked_crates -and $name.Replace('-', '_') -in $evidence.checked_crates
            } else { $accepted = $false }
            if ($VerifyCache -and !$negative -and $accepted) {
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
