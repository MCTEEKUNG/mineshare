param([Parameter(Mandatory)][string]$Output, [int]$Seconds = 60)
$ErrorActionPreference = 'Stop'
$all = @(Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,Name)
$ids = @($all | Where-Object Name -eq 'mineshare-app.exe' | ForEach-Object { [int]$_.ProcessId })
if ($ids.Count -eq 0) { throw 'MineShare is not running' }
do {
    $children = @($all | Where-Object { $_.ParentProcessId -in $ids -and $_.ProcessId -notin $ids } | ForEach-Object { [int]$_.ProcessId })
    $ids += $children
} while ($children.Count -gt 0)
$samples = @()
$clock = [Diagnostics.Stopwatch]::StartNew()
do {
    $processes = @(Get-Process -Id $ids -ErrorAction Stop)
    $samples += [pscustomobject]@{
        ElapsedSeconds = $clock.Elapsed.TotalSeconds
        CpuSeconds = ($processes | Measure-Object CPU -Sum).Sum
        PrivateBytes = ($processes | Measure-Object PrivateMemorySize64 -Sum).Sum
        WorkingSetBytes = ($processes | Measure-Object WorkingSet64 -Sum).Sum
        Handles = ($processes | Measure-Object HandleCount -Sum).Sum
    }
    if ($clock.Elapsed.TotalSeconds -ge $Seconds) { break }
    Start-Sleep -Seconds 1
} while ($true)
$first = $samples[0]; $last = $samples[-1]
$logical = (Get-CimInstance Win32_ComputerSystem).NumberOfLogicalProcessors
$end = @(Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'mineshare-app.exe' -or $_.ParentProcessId -in $ids } | Select-Object -ExpandProperty ProcessId)
if (@($end | Where-Object { $_ -notin $ids }).Count -gt 0) { throw 'Process tree changed; repeat the sample' }
[pscustomobject]@{
    Computer = $env:COMPUTERNAME
    State = 'Connected; existing system-audio route enabled; no synthetic stimulus'
    DurationSeconds = $last.ElapsedSeconds - $first.ElapsedSeconds
    LogicalProcessors = $logical
    CpuPercentMachine = 100 * ($last.CpuSeconds - $first.CpuSeconds) / ($last.ElapsedSeconds - $first.ElapsedSeconds) / $logical
    PrivateBytesFirst = $first.PrivateBytes; PrivateBytesLast = $last.PrivateBytes
    HandlesFirst = $first.Handles; HandlesLast = $last.Handles
    Processes = @($all | Where-Object ProcessId -in $ids)
    Samples = $samples
} | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $Output -Encoding UTF8
