param([Parameter(Mandatory)][ValidateSet('before','after')][string]$Variant)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$manifest = Get-Content (Join-Path $PSScriptRoot 'deployment.json') -Raw | ConvertFrom-Json
$exe = Join-Path $env:LOCALAPPDATA 'MineShare/mineshare-app.exe'
$final = Join-Path $PSScriptRoot 'final-app.exe'
$original = Join-Path $PSScriptRoot 'before/mineshare-app.exe'
if (-not (Test-Path -LiteralPath $final)) {
    if ((Get-FileHash -LiteralPath $exe).Hash -ne $manifest.PayloadHash) { throw 'Cannot preserve final payload' }
    Copy-Item -LiteralPath $exe -Destination $final
}
if ((Get-FileHash -LiteralPath $final).Hash -ne $manifest.PayloadHash -or (Get-FileHash -LiteralPath $original).Hash -ne $manifest.BeforeHash) { throw 'Comparison payload hash mismatch' }
if ((Get-FileHash -LiteralPath $exe).Hash -notin @($manifest.BeforeHash,$manifest.PayloadHash)) { throw 'Unexpected active payload' }
$startup = Export-ScheduledTask MineShareLaunch
$task = Get-ScheduledTask MineShareLaunch
if ($task.Actions[0].Execute.Trim('"') -ine $exe -or $task.Actions[0].Arguments -ne '--mineshare-autostart') { throw 'Unexpected startup action' }
$config = @(Get-ChildItem "$env:APPDATA/MineShare" -File | Get-FileHash | Select-Object Path,Hash)
Stop-ScheduledTask MineShareLaunch
Get-CimInstance Win32_Process -Filter "name='mineshare-app.exe'" | Where-Object ExecutablePath -eq $exe | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
Start-Sleep -Milliseconds 700
if (@(Get-CimInstance Win32_Process -Filter "name='mineshare-app.exe'" | Where-Object ExecutablePath -eq $exe).Count -gt 0) { throw 'Application did not stop' }
$selected = if ($Variant -eq 'before') { $original } else { $final }
try { Copy-Item -LiteralPath $selected -Destination $exe } catch {
    Copy-Item -LiteralPath $final -Destination $exe
    Start-ScheduledTask MineShareLaunch
    throw
}
$started = [DateTime]::UtcNow
Start-ScheduledTask MineShareLaunch
$deadline = $started.AddSeconds(30)
do {
    Start-Sleep -Milliseconds 500
    $apps = @(Get-CimInstance Win32_Process -Filter "name='mineshare-app.exe'" | Where-Object ExecutablePath -eq $exe)
} while ($apps.Count -ne 3 -and [DateTime]::UtcNow -lt $deadline)
if ($apps.Count -ne 3) { throw 'Expected supervisor, GUI and keyboard worker' }
if ((Export-ScheduledTask MineShareLaunch) -cne $startup) { throw 'Startup configuration changed' }
foreach ($item in $config) { if ((Get-FileHash -LiteralPath $item.Path).Hash -ne $item.Hash) { throw 'Application configuration changed' } }
[pscustomobject]@{Computer=$env:COMPUTERNAME;Variant=$Variant;Hash=(Get-FileHash -LiteralPath $exe).Hash;StartedUtc=$started.ToString('o');ProcessCount=$apps.Count;StartupUnchanged=$true;ConfigUnchanged=$true} | ConvertTo-Json -Compress
