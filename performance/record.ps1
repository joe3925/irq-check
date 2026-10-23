param(
    [Parameter(Mandatory = $true)][int]$CheckerProcessId,
    [Parameter(Mandatory = $true)][string]$LogPath,
    [Parameter(Mandatory = $true)][string]$OutputPath
)

$ErrorActionPreference = 'Stop'
$process = Get-Process -Id $CheckerProcessId
$creation = $process.StartTime.ToUniversalTime()
$binary = $process.Path
$identity = (Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash
$processors = [int](Get-CimInstance Win32_ComputerSystem).NumberOfLogicalProcessors
$previousTime = [DateTime]::UtcNow
$previousCpu = $process.CPU
$lastStatus = $null
$lastStatusTime = $null
$writer = [System.IO.StreamWriter]::new($OutputPath, $true)
$writer.AutoFlush = $true
try {
    $writer.WriteLine(([ordered]@{
        event = 'monitor_start'
        time_utc = $previousTime.ToString('o')
        process_id = $CheckerProcessId
        process_start_utc = $creation.ToString('o')
        binary = $binary
        sha256 = $identity
        logical_processors = $processors
        log = $LogPath
    } | ConvertTo-Json -Compress))
    while ($true) {
        Start-Sleep -Seconds 10
        $process = Get-Process -Id $CheckerProcessId -ErrorAction SilentlyContinue
        if (!$process -or $process.StartTime.ToUniversalTime() -ne $creation) {
            $writer.WriteLine(([ordered]@{
                event = 'process_no_longer_running'
                time_utc = [DateTime]::UtcNow.ToString('o')
                verdict = 'not_inferred'
            } | ConvertTo-Json -Compress))
            break
        }
        $time = [DateTime]::UtcNow
        $cpu = $process.CPU
        $seconds = ($time - $previousTime).TotalSeconds
        $observedStatus = Get-Content -LiteralPath $LogPath -Tail 300 |
            Where-Object { $_ -match 'elapsed=.*context_batches=' } |
            Select-Object -Last 1
        if ($observedStatus -and [string]$observedStatus -ne $lastStatus) {
            $lastStatus = [string]$observedStatus
            $lastStatusTime = $time
        }
        $writer.WriteLine(([ordered]@{
            event = 'sample'
            time_utc = $time.ToString('o')
            process_elapsed_seconds = ($time - $creation).TotalSeconds
            cpu_seconds = $cpu
            interval_seconds = $seconds
            cpu_percent_all_processors = 100 * ($cpu - $previousCpu) / $seconds / $processors
            private_bytes = $process.PrivateMemorySize64
            working_set_bytes = $process.WorkingSet64
            peak_process_commit_bytes = $process.PeakPagedMemorySize64
            free_system_commit_kib = (Get-CimInstance Win32_OperatingSystem).FreeVirtualMemory
            last_analysis_status = [string]$lastStatus
            analysis_status_age_seconds = if ($lastStatusTime) { ($time - $lastStatusTime).TotalSeconds } else { $null }
        } | ConvertTo-Json -Compress))
        $previousTime = $time
        $previousCpu = $cpu
    }
} finally {
    $writer.Dispose()
}
