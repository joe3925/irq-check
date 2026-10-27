param(
    [string]$Checker = (Get-Command irq-check -ErrorAction Stop).Source,
    [string]$Scenario = 'memory-growth',
    [string]$ManifestPath = '',
    [string]$WorkingDirectory = '',
    [string]$CargoTargetDirectory = '',
    [string]$Target = 'x86_64-unknown-none',
    [string]$BuildStd = 'core,alloc,compiler_builtins',
    [string]$Features = '',
    [double]$StopPrivateGiB = 0,
    [int]$TimeoutSeconds = 0,
    [string]$DumpDirectory = '',
    [switch]$Plain
)

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$checkerPath = (Resolve-Path -LiteralPath $Checker).Path
$driverPath = Join-Path (Split-Path $checkerPath -Parent) 'irq-check-driver.exe'
$run = Join-Path $root ('target/scenarios/memory-' + [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss-fffffff'))
New-Item -ItemType Directory -Path $run -Force | Out-Null
$manifest = if ($ManifestPath) { (Resolve-Path -LiteralPath $ManifestPath).Path } else { (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "$Scenario/Cargo.toml")).Path }
Copy-Item -LiteralPath (Join-Path (Split-Path $manifest -Parent) 'src/main.rs') -Destination (Join-Path $run 'main.rs')
Copy-Item -LiteralPath $manifest -Destination (Join-Path $run 'Cargo.toml')
$start = [Diagnostics.ProcessStartInfo]::new()
$start.FileName = $checkerPath
$start.WorkingDirectory = if ($WorkingDirectory) { (Resolve-Path -LiteralPath $WorkingDirectory).Path } else { $root }
$start.UseShellExecute = $false
$start.CreateNoWindow = $true
$start.RedirectStandardOutput = $true
$start.RedirectStandardError = $true
$start.EnvironmentVariables['RUSTUP_TOOLCHAIN'] = 'nightly'
$start.EnvironmentVariables['IRQ_CHECK_STATS'] = '1'
$start.EnvironmentVariables['IRQ_CHECK_REQUIRED_CRATE'] = $Scenario.Replace('-', '_')
$start.EnvironmentVariables['IRQ_CHECK_ANALYSIS_ID'] = Split-Path $run -Leaf
$start.EnvironmentVariables['CARGO_TARGET_DIR'] = if ($CargoTargetDirectory) { [IO.Path]::GetFullPath($CargoTargetDirectory) } else { Join-Path $root 'target/memory-scenario-cargo' }
$start.EnvironmentVariables.Remove('IRQ_CHECK_DUMP_DIR')
if ($DumpDirectory) { $start.EnvironmentVariables['IRQ_CHECK_DUMP_DIR'] = [IO.Path]::GetFullPath($DumpDirectory) }
$start.Arguments = '--cargo build --manifest-path "' + $manifest + '" -p ' + $Scenario + ' --release --target "' + $Target + '" -Zbuild-std=' + $BuildStd + ' -Zbuild-std-features=compiler-builtins-mem'
if ($Target.EndsWith('.json')) { $start.Arguments += ' -Zunstable-options -Zjson-target-spec' }
if ($PSBoundParameters.ContainsKey('Features')) { $start.Arguments += ' --no-default-features' }
if ($Features) { $start.Arguments += ' --features "' + $Features + '"' }
if ($Plain) {
    $start.FileName = (Get-Command cargo -ErrorAction Stop).Source
    $start.Arguments = '+nightly ' + $start.Arguments.Substring(8)
    $start.EnvironmentVariables.Remove('RUSTC_WRAPPER')
    $start.EnvironmentVariables.Remove('RUSTC_WORKSPACE_WRAPPER')
}
$logPath = Join-Path $run 'stderr.log'
$log = [IO.FileStream]::new($logPath, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::ReadWrite, 4096, [IO.FileOptions]::Asynchronous)
$samples = [IO.StreamWriter]::new((Join-Path $run 'memory.jsonl'), $false)
$samples.AutoFlush = $true
$process = [Diagnostics.Process]::new()
$process.StartInfo = $start
$watch = [Diagnostics.Stopwatch]::StartNew()
$owned = [Collections.Generic.HashSet[int]]::new()
$ownedStarts = @{}
$peakPrivate = 0L
$peakCommit = 0L
$targetPeakPrivate = 0L
$targetPeakCommit = 0L
$crates = @{}
$reason = 'completed'
$started = $false
try {
    $samples.WriteLine((@{event='start'; checker=$checkerPath; driver_sha256=(Get-FileHash -LiteralPath $driverPath).Hash; source_sha256=(Get-FileHash -LiteralPath (Join-Path (Split-Path $manifest -Parent) 'src/main.rs')).Hash; scenario=$Scenario; features=$Features; plain=[bool]$Plain; command=$start.Arguments; stop_private_gib=$StopPrivateGiB; timeout_seconds=$TimeoutSeconds; time_utc=[DateTime]::UtcNow.ToString('o')} | ConvertTo-Json -Compress))
    if (!$process.Start()) { throw 'Cannot start compiler' }
    $started = $true
    [void]$owned.Add($process.Id)
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $stderr = $process.StandardError.BaseStream.CopyToAsync($log)
    Write-Output "Evidence: $run"
    do {
        $snapshot = @(Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name, CommandLine, CreationDate)
        $liveProcesses = @{}
        foreach ($entry in $snapshot) { $liveProcesses[[int]$entry.ProcessId] = $entry }
        foreach ($ownedId in @($owned)) {
            $entry = $liveProcesses[$ownedId]
            if (!$entry -or ($ownedStarts.ContainsKey($ownedId) -and $ownedStarts[$ownedId] -ne $entry.CreationDate)) {
                [void]$owned.Remove($ownedId)
                $ownedStarts.Remove($ownedId)
                $crates.Remove($ownedId)
            } else {
                $ownedStarts[$ownedId] = $entry.CreationDate
            }
        }
        do {
            $added = $false
            foreach ($entry in $snapshot) {
                if ($owned.Contains([int]$entry.ParentProcessId) -and $owned.Add([int]$entry.ProcessId)) {
                    $ownedStarts[[int]$entry.ProcessId] = $entry.CreationDate
                    $added = $true
                    $samples.WriteLine((@{event='process'; pid=$entry.ProcessId; parent=$entry.ParentProcessId; name=$entry.Name; command=$entry.CommandLine} | ConvertTo-Json -Compress))
                }
            }
        } while ($added)
        foreach ($entry in $snapshot) {
            if (!$owned.Contains([int]$entry.ProcessId) -or $entry.Name -notin @('irq-check-driver.exe', 'rustc.exe')) { continue }
            $child = Get-Process -Id $entry.ProcessId -ErrorAction SilentlyContinue
            if (!$child) { continue }
            try {
                $processStart = $child.StartTime
                if ($null -eq $processStart) { continue }
                if (!$crates.ContainsKey([int]$entry.ProcessId)) {
                    $command = $entry.CommandLine
                    if ($entry.Name -eq 'irq-check-driver.exe') {
                        $command = ($snapshot | Where-Object ProcessId -eq $entry.ParentProcessId | Select-Object -First 1).CommandLine
                    }
                    if ($command -match '--crate-name\s+(\w+)') { $crates[[int]$entry.ProcessId] = $Matches[1] }
                }
                $crate = $crates[[int]$entry.ProcessId]
                $private = $child.PrivateMemorySize64
                $peakPrivate = [Math]::Max($peakPrivate, $private)
                $peakCommit = [Math]::Max($peakCommit, $child.PeakPagedMemorySize64)
                if ($crate -eq $Scenario.Replace('-', '_')) {
                    $targetPeakPrivate = [Math]::Max($targetPeakPrivate, $private)
                    $targetPeakCommit = [Math]::Max($targetPeakCommit, $child.PeakPagedMemorySize64)
                }
                $samples.WriteLine((@{event='sample'; seconds=$watch.Elapsed.TotalSeconds; process_seconds=([DateTime]::UtcNow - $processStart.ToUniversalTime()).TotalSeconds; pid=$child.Id; crate=$crate; private_bytes=$private; peak_commit_bytes=$child.PeakPagedMemorySize64; working_set_bytes=$child.WorkingSet64; cpu_seconds=$child.CPU} | ConvertTo-Json -Compress))
                if ($StopPrivateGiB -gt 0 -and $private -ge $StopPrivateGiB * 1GB) { $reason = 'memory-limit' }
            } finally { $child.Dispose() }
        }
        if ($stderr.IsFaulted) { [void]$stderr.GetAwaiter().GetResult() }
        if ($TimeoutSeconds -gt 0 -and $watch.Elapsed.TotalSeconds -ge $TimeoutSeconds) { $reason = 'time-limit' }
        if ($reason -ne 'completed' -and !$process.HasExited) {
            & taskkill.exe /PID $process.Id /T /F | Out-Null
        }
        Start-Sleep -Milliseconds 250
    } while (!$process.HasExited)
    $process.WaitForExit()
    [void]$stderr.GetAwaiter().GetResult()
    [IO.File]::WriteAllText((Join-Path $run 'stdout.log'), $stdout.Result)
    $result = @{event='result'; reason=$reason; seconds=$watch.Elapsed.TotalSeconds; exit_code=$process.ExitCode; peak_private_bytes=$peakPrivate; peak_commit_bytes=$peakCommit; target_peak_private_bytes=$targetPeakPrivate; target_peak_commit_bytes=$targetPeakCommit; log=$logPath}
    $samples.WriteLine(($result | ConvertTo-Json -Compress))
    $result | ConvertTo-Json -Compress
} finally {
    if ($started -and !$process.HasExited) { & taskkill.exe /PID $process.Id /T /F | Out-Null }
    $samples.Dispose()
    $log.Dispose()
    $process.Dispose()
}
