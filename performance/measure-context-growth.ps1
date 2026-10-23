param(
    [Parameter(Mandatory = $true)][string]$Checker,
    [Parameter(Mandatory = $true)][string]$RunName,
    [switch]$Negative
)

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$checkerPath = (Resolve-Path -LiteralPath $Checker).Path
$driverPath = Join-Path (Split-Path $checkerPath -Parent) 'irq-check-driver.exe'
$output = Join-Path $PSScriptRoot "$RunName.jsonl"
$log = Join-Path $root "target/$RunName.log"
$start = [Diagnostics.ProcessStartInfo]::new()
$start.FileName = $checkerPath
$start.WorkingDirectory = $root
$start.UseShellExecute = $false
$start.CreateNoWindow = $true
$start.RedirectStandardOutput = $true
$start.RedirectStandardError = $true
$start.EnvironmentVariables['RUSTUP_TOOLCHAIN'] = 'nightly'
$start.EnvironmentVariables['IRQ_CHECK_STATS'] = '1'
$start.EnvironmentVariables['IRQ_CHECK_REQUIRED_CRATE'] = 'context_growth'
$start.EnvironmentVariables['IRQ_CHECK_ANALYSIS_ID'] = $RunName
$start.EnvironmentVariables['CARGO_TARGET_DIR'] = Join-Path $root 'target/context-growth-memory-cargo'
$start.EnvironmentVariables.Remove('IRQ_CHECK_DUMP_DIR')
$start.EnvironmentVariables.Remove('IRQ_CHECK_TRACE_HAVOC')
$start.Arguments = '--cargo build --manifest-path scenarios/context-growth/Cargo.toml -p context-growth --release --target x86_64-unknown-none -Zbuild-std=core,alloc,compiler_builtins -Zbuild-std-features=compiler-builtins-mem'
if ($Negative) { $start.Arguments += ' --features negative' }
$writer = [IO.StreamWriter]::new($output, $false)
$writer.AutoFlush = $true
$process = [Diagnostics.Process]::new()
$process.StartInfo = $start
$watch = [Diagnostics.Stopwatch]::StartNew()
$peakPrivate = 0L
$peakCommit = 0L
$seen = [Collections.Generic.HashSet[int]]::new()
$errorFile = [IO.FileStream]::new($log, [IO.FileMode]::Create, [IO.FileAccess]::Write, [IO.FileShare]::ReadWrite, 1, [IO.FileOptions]::Asynchronous)
try {
    $writer.WriteLine((@{ event='start'; checker=$checkerPath; driver_sha256=(Get-FileHash -LiteralPath $driverPath).Hash; negative=[bool]$Negative; time_utc=[DateTime]::UtcNow.ToString('o') } | ConvertTo-Json -Compress))
    if (!$process.Start()) { throw 'Cannot start checker' }
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $stderr = $process.StandardError.BaseStream.CopyToAsync($errorFile)
    do {
        foreach ($driver in @(Get-Process irq-check-driver -ErrorAction SilentlyContinue)) {
            try {
                if ($driver.Path -ne $driverPath) { continue }
                if ($seen.Add($driver.Id)) {
                    $command = (Get-CimInstance Win32_Process -Filter "ProcessId=$($driver.Id)").CommandLine
                    $crate = $null
                    if ($command -match '--irq-check-arguments\s+"?(.+?\.json)"?(?:\s|$)') {
                        $compilerArgs = [IO.File]::ReadAllText($Matches[1]) | ConvertFrom-Json
                        $position = [Array]::IndexOf($compilerArgs, '--crate-name')
                        if ($position -ge 0) { $crate = $compilerArgs[$position + 1] }
                    }
                    $writer.WriteLine((@{event='process'; pid=$driver.Id; crate=$crate; command=$command} | ConvertTo-Json -Compress))
                }
                $private = $driver.PrivateMemorySize64
                $peak = $driver.PeakPagedMemorySize64
                $peakPrivate = [Math]::Max($peakPrivate, $private)
                $peakCommit = [Math]::Max($peakCommit, $peak)
                $reader = [IO.StreamReader]::new([IO.FileStream]::new($log, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite))
                try { $text = $reader.ReadToEnd() } finally { $reader.Dispose() }
                $phase = if ($text.Contains('irq-check: context batches=')) {'after-analysis'} elseif ($text.Contains('irq-check: initial IRQ root:')) {'analysis'} else {'frontend'}
                $writer.WriteLine((@{ event='sample'; phase=$phase; seconds=$watch.Elapsed.TotalSeconds; pid=$driver.Id; private_bytes=$private; peak_commit_bytes=$peak; working_set_bytes=$driver.WorkingSet64; cpu_seconds=$driver.CPU } | ConvertTo-Json -Compress))
            } catch [InvalidOperationException] {}
        }
        Start-Sleep -Milliseconds 200
    } while (!$process.HasExited)
    $process.WaitForExit()
    [void]$stderr.GetAwaiter().GetResult()
    $errorFile.Dispose()
    [IO.File]::AppendAllText($log, $stdout.Result)
    $result = @{ event='result'; seconds=$watch.Elapsed.TotalSeconds; exit_code=$process.ExitCode; peak_private_bytes=$peakPrivate; peak_commit_bytes=$peakCommit; sampled_processes=$seen.Count; log=$log }
    $writer.WriteLine(($result | ConvertTo-Json -Compress))
    $result | ConvertTo-Json -Compress
} finally {
    $writer.Dispose()
    $errorFile.Dispose()
    $process.Dispose()
}
