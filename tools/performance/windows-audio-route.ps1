param([Parameter(Mandatory)][ValidateSet('Original','Reverse')][string]$Route)
$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
. (Join-Path $PSScriptRoot 'windows-processes.ps1')
$path=Join-Path $env:APPDATA 'MineShare/settings.json'
$backup=Join-Path $PSScriptRoot 'route-original-settings.json'
$exe=Join-Path $env:LOCALAPPDATA 'MineShare/mineshare-app.exe'
$task=Get-ScheduledTask MineShareLaunch
if($task.Actions[0].Execute.Trim('"') -ine $exe -or $task.Actions[0].Arguments -ne '--mineshare-autostart'){throw 'Unexpected startup action'}
$startup=Export-ScheduledTask MineShareLaunch
if(-not (Test-Path -LiteralPath $backup)){Copy-Item -LiteralPath $path -Destination $backup}
$original=[IO.File]::ReadAllText($backup) | ConvertFrom-Json
Stop-ScheduledTask MineShareLaunch
Stop-MineShareProcesses $exe
$until=[DateTime]::UtcNow.AddSeconds(10)
while(@(Get-Process mineshare-app -ErrorAction SilentlyContinue).Count -gt 0 -and [DateTime]::UtcNow -lt $until){Start-Sleep -Milliseconds 100}
if(@(Get-Process mineshare-app -ErrorAction SilentlyContinue).Count -gt 0){Start-ScheduledTask MineShareLaunch;throw 'Application did not stop; settings left unchanged'}
try {
    $current=[IO.File]::ReadAllText($path) | ConvertFrom-Json
    $current.audio_send_sysout=$original.audio_send_sysout
    $current.audio_play_sysout=$original.audio_play_sysout
    $unchanged=($current | ConvertTo-Json -Depth 20 -Compress) -ceq ($original | ConvertTo-Json -Depth 20 -Compress)
    if($Route -eq 'Reverse') {
        if($env:COMPUTERNAME -eq 'LAPTOP-4E1649F3') {$current.audio_send_sysout=$true;$current.audio_play_sysout=$false}
        elseif($env:COMPUTERNAME -eq 'MCTEEKUNG-PC') {$current.audio_send_sysout=$false;$current.audio_play_sysout=$true}
        else {throw 'Unexpected computer'}
        [IO.File]::WriteAllText($path,($current | ConvertTo-Json -Depth 20),(New-Object Text.UTF8Encoding($false)))
    } elseif($unchanged) {
        [IO.File]::WriteAllBytes($path,[IO.File]::ReadAllBytes($backup))
    } else {
        # Preserve any unrelated preference the user changed while testing.
        [IO.File]::WriteAllText($path,($current | ConvertTo-Json -Depth 20),(New-Object Text.UTF8Encoding($false)))
    }
} finally {Start-ScheduledTask MineShareLaunch}
$until=[DateTime]::UtcNow.AddSeconds(25)
while(@(Get-Process mineshare-app -ErrorAction SilentlyContinue).Count -ne 3 -and [DateTime]::UtcNow -lt $until){Start-Sleep -Milliseconds 250}
if(@(Get-Process mineshare-app -ErrorAction SilentlyContinue).Count -ne 3){throw 'Startup did not restore supervisor, GUI and keyboard worker'}
if((Export-ScheduledTask MineShareLaunch) -cne $startup){throw 'Startup settings changed'}
[pscustomobject]@{Computer=$env:COMPUTERNAME;Route=$Route;Hash=(Get-FileHash $exe).Hash;OriginalNonAudioPreferences=$unchanged;StartupUnchanged=$true;SettingsHash=(Get-FileHash $path).Hash} | ConvertTo-Json -Compress
