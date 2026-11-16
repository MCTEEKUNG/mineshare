param([Parameter(Mandatory)][string]$ExpectedComputer)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$root = $PSScriptRoot
$queryHelper = Join-Path $root 'windows-processes.ps1'
if(Test-Path -LiteralPath $queryHelper){. $queryHelper}
function Get-AppProcesses([string]$Executable) {
    if(Get-Command Get-MineShareProcesses -ErrorAction SilentlyContinue){Get-MineShareProcesses $Executable}
    else {Get-CimInstance Win32_Process -Filter "name='mineshare-app.exe'" | Where-Object ExecutablePath -eq $Executable}
}
try {
    if ($env:COMPUTERNAME -ne $ExpectedComputer) { throw 'Unexpected target computer' }
    $manifest = Get-Content (Join-Path $root 'deployment.json') -Raw | ConvertFrom-Json
    $msi = Join-Path $root 'MineShare_0.0.16_x64_en-US.msi'
    $exe = Join-Path $env:LOCALAPPDATA 'MineShare/mineshare-app.exe'
    if ((Get-FileHash -LiteralPath $msi).Hash -ne $manifest.MsiHash) { throw 'Package hash mismatch' }
    if ((Get-FileHash -LiteralPath $exe).Hash -ne $manifest.BeforeHash) { throw 'Installed application changed since baseline' }
    $task = Get-ScheduledTask MineShareLaunch
    if (@($task.Actions).Count -ne 1 -or $task.Actions[0].Execute.Trim('"') -ine $exe) { throw 'Unexpected startup action' }
    $startup = Export-ScheduledTask MineShareLaunch
    $old = @(Get-AppProcesses $exe | Select-Object -ExpandProperty ProcessId)
    if ($old.Count -eq 0) { throw 'Expected the baseline application to be running' }
    # Private configuration stays in the local staging directory, outside the repository.
    $backup = Join-Path $root 'before'
    if (Test-Path -LiteralPath $backup) { throw 'Existing backup: refusing to overwrite' }
    New-Item -ItemType Directory -Path (Join-Path $backup 'config') | Out-Null
    Copy-Item -LiteralPath $exe -Destination $backup
    $startup | Set-Content -LiteralPath (Join-Path $backup 'startup.xml') -Encoding UTF8
    $configPath = Join-Path $env:APPDATA 'MineShare'
    $config = @(Get-ChildItem -LiteralPath $configPath -File | Get-FileHash | Select-Object Path,Hash)
    Get-ChildItem -LiteralPath $configPath -File | Copy-Item -Destination (Join-Path $backup 'config')
    $config | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $backup 'config-hashes.json') -Encoding UTF8
    $installer = New-Object -ComObject WindowsInstaller.Installer
    $db = $installer.OpenDatabase($msi,0)
    $view = $db.OpenView('SELECT `Value` FROM `Property` WHERE `Property` = ''ProductCode''')
    $view.Execute(); $productCode = $view.Fetch().StringData(1); $view.Close()
    $arguments = @('/i',('"'+$msi+'"'),'/qn','/norestart','AUTOLAUNCHAPP=0',('/l*v "'+(Join-Path $root 'install.log')+'"'))
    if ($installer.ProductState($productCode) -eq 5) { $arguments += @('REINSTALL=ALL','REINSTALLMODE=amus') }
    $process = Start-Process msiexec.exe -WindowStyle Hidden -ArgumentList $arguments -Wait -PassThru
    if ($process.ExitCode -notin @(0,3010)) { throw ('MSI exit '+$process.ExitCode) }
    if ((Get-FileHash -LiteralPath $exe).Hash -ne $manifest.PayloadHash) { throw 'Installed binary differs from verified package payload' }
    if ((Export-ScheduledTask MineShareLaunch) -cne $startup) { throw 'Startup configuration changed' }
    foreach ($item in $config) {
        if ((Get-FileHash -LiteralPath $item.Path).Hash -ne $item.Hash) { throw ('Configuration changed: '+(Split-Path $item.Path -Leaf)) }
    }
    $apps = @(Get-AppProcesses $exe)
    if ($task.Actions[0].Arguments -eq '--mineshare-autostart' -and @($apps | Where-Object CommandLine -like '*--mineshare-autostart*').Count -eq 0) {
        # The MSI may relaunch the GUI directly. Exercise the unchanged login
        # task so the final acceptance run also retains its supervisor.
        Stop-ScheduledTask MineShareLaunch
        if(Get-Command Stop-MineShareProcesses -ErrorAction SilentlyContinue){Stop-MineShareProcesses $exe}
        else {$apps | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }}
        Start-Sleep -Milliseconds 700
        Start-ScheduledTask MineShareLaunch
    } elseif ($apps.Count -eq 0) { Start-ScheduledTask MineShareLaunch }
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    do {
        Start-Sleep -Milliseconds 500
        $apps = @(Get-AppProcesses $exe)
        $gui = @($apps | Where-Object { $_.SessionId -gt 0 -and $_.CommandLine -notmatch '--mineshare-(keyboard-worker|autostart)' })
        $workers = @($apps | Where-Object CommandLine -like '*--mineshare-keyboard-worker*')
        if(@($apps | Where-Object CommandLine).Count -eq 0 -and $apps.Count -eq 3) {
            $supervisor=@($apps | Where-Object {$_.ParentProcessId -notin $apps.ProcessId})
            $gui=@($apps | Where-Object {$_.ParentProcessId -in $supervisor.ProcessId -and $_.SessionId -gt 0})
            $workers=@($apps | Where-Object {$_.ParentProcessId -in $gui.ProcessId -and $_.SessionId -gt 0})
        }
    } while (($gui.Count -ne 1 -or $workers.Count -ne 1) -and [DateTime]::UtcNow -lt $deadline)
    if ($gui.Count -ne 1 -or $workers.Count -ne 1) { throw 'Expected one desktop GUI and one keyboard worker' }
    if (@($apps | Where-Object ProcessId -in $old).Count -gt 0) { throw 'Old application process survived upgrade' }
    [pscustomobject]@{
        Result='PASS'; Computer=$env:COMPUTERNAME; SourceId=$manifest.SourceId
        MsiExit=$process.ExitCode; Hash=(Get-FileHash -LiteralPath $exe).Hash
        StartupUnchanged=$true; ConfigUnchanged=$true; Backup=$backup
        Apps=@($apps | Select-Object ProcessId,SessionId,ExecutablePath,CommandLine)
    } | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $root 'result.json') -Encoding UTF8
} catch {
    ($_ | Out-String) | Set-Content -LiteralPath (Join-Path $root 'error.txt') -Encoding UTF8
    throw
}
