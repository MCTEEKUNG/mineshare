param(
    [Parameter(Mandatory=$true)][string]$NewDirectory,
    [Parameter(Mandatory=$true)]
    [ValidateSet('msi-msi','msi-exe','exe-msi','exe-exe','script-msi','script-exe','programs-msi','programs-exe','fresh-msi','fresh-exe','rollback','rollback-nsis','downgrade')]
    [string]$Scenario
)
$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
# Destructive installation tests are ONLY for disposable GitHub-hosted runners.
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') {
    throw 'This test must not run on a user PC or self-hosted runner'
}
function Assert([bool]$Condition,[string]$Message) { if (-not $Condition) { throw $Message } }
function Registrations {
    @(Get-ChildItem HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall,
        HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall,
        HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall -ErrorAction SilentlyContinue |
        Get-ItemProperty | Where-Object { $_.DisplayName -eq 'MineShare' })
}
function Install([string]$Path,[bool]$ShouldFail=$false) {
    $log=Join-Path $env:RUNNER_TEMP ('msi-'+[Guid]::NewGuid().ToString('N')+'.log')
    if ($Path.EndsWith('.msi')) {
        $p=Start-Process msiexec.exe -WindowStyle Hidden -ArgumentList @('/i',('"'+$Path+'"'),'/qn','/norestart','AUTOLAUNCHAPP=0',('/l*v "'+$log+'"')) -PassThru
    } else {
        $p=Start-Process -FilePath $Path -WindowStyle Hidden -ArgumentList '/S' -PassThru
    }
    if (-not $p.WaitForExit(600000)) { throw 'Installer timed out' }
    if ($ShouldFail) { Assert ($p.ExitCode -eq 1603) "Expected rollback failure, got $($p.ExitCode)" }
    else { Assert ($p.ExitCode -in @(0,3010)) "Installer failed with $($p.ExitCode); inspect $log" }
}
Assert (@(Registrations).Count -eq 0) 'Runner already has MineShare installed'
$oldDirectory=Join-Path $env:RUNNER_TEMP 'old-installers'
New-Item -ItemType Directory -Force -Path $oldDirectory | Out-Null
# Pin the original, verified release build used on the user's machines. The old
# tag workflow rebuilt public v0.0.12 assets; do not silently trust replacements.
& gh run download 38932717686 --repo MCTEEKUNG/mineshare -n mineshare-windows-x64 -D $oldDirectory
Assert ($LASTEXITCODE -eq 0) 'Could not fetch baseline release'
$oldMsi=(Get-ChildItem $oldDirectory -Filter *.msi | Select-Object -First 1).FullName
$oldExe=(Get-ChildItem $oldDirectory -Filter *.exe | Select-Object -First 1).FullName
Assert ((Get-FileHash -LiteralPath $oldMsi).Hash -eq 'BF990B03FC322866C9DFDE5A9861CA2E7C127F10ADF104D46D8A478B2966AB1C') 'Baseline MSI digest changed'
Assert ((Get-FileHash -LiteralPath $oldExe).Hash -eq 'A62CE43252903F4723D841DC18F4C6C7B7D637606A83111D73746AEE899C95DD') 'Baseline EXE digest changed'
$oldKind=if($Scenario -eq 'rollback') { 'msi' } elseif($Scenario -in @('downgrade','rollback-nsis')) { 'exe' } else { $Scenario.Split('-')[0] }
$newKind=if($Scenario -in @('rollback','rollback-nsis','downgrade')) { 'msi' } else { $Scenario.Split('-')[1] }
if ($oldKind -eq 'msi') { Install $oldMsi; $location=Join-Path $env:ProgramFiles 'MineShare' }
elseif ($oldKind -eq 'exe') { Install $oldExe; $location=Join-Path $env:LOCALAPPDATA 'MineShare' }
elseif ($oldKind -eq 'fresh') { $location=Join-Path $env:ProgramFiles 'MineShare' }
else {
    # Script-only deployments have no Windows Installer product registration.
    # Extract an authentic old binary without installing another product.
    $extract=Join-Path $env:RUNNER_TEMP 'old-extracted'
    $p=Start-Process msiexec.exe -WindowStyle Hidden -ArgumentList @('/a',('"'+$oldMsi+'"'),'/qn',('TARGETDIR="'+$extract+'"')) -Wait -PassThru
    Assert ($p.ExitCode -eq 0) 'Baseline extraction failed'
    $location=if($oldKind -eq 'programs') { Join-Path $env:LOCALAPPDATA 'Programs\MineShare' } else { Join-Path $env:LOCALAPPDATA 'MineShare' }
    New-Item -ItemType Directory -Force -Path $location | Out-Null
    Copy-Item -LiteralPath (Get-ChildItem $extract -Recurse -Filter mineshare-app.exe | Select-Object -First 1).FullName -Destination $location
}
$installed=Join-Path $location 'mineshare-app.exe'
if($oldKind -ne 'fresh') { Assert (Test-Path -LiteralPath $installed) 'Baseline binary absent' }
Get-Process mineshare-app -ErrorAction SilentlyContinue | Where-Object { $_.Path -ieq $installed } | Stop-Process -Force
$oldHash=if($oldKind -ne 'fresh') { (Get-FileHash -LiteralPath $installed).Hash } else { '' }
$data=Join-Path $env:APPDATA 'MineShare'
New-Item -ItemType Directory -Force -Path $data | Out-Null
@{ device_id='fixture-do-not-change'; trusted_peers=@('pairing-fixture') } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $data 'identity.json')
'{"keyboard_mode":"smart","fixture":"retain"}' | Set-Content -LiteralPath (Join-Path $data 'settings.json')
$before=@{}
Get-ChildItem -LiteralPath $data -File | ForEach-Object { $before[$_.Name]=(Get-FileHash -LiteralPath $_.FullName).Hash }
# Preserve a deliberately disabled task too: updating must not re-enable it.
$taskAction=New-ScheduledTaskAction -Execute $installed -WorkingDirectory $location
$taskTrigger=New-ScheduledTaskTrigger -AtLogOn
if($oldKind -ne 'fresh') {
    Register-ScheduledTask -TaskName MineShareLaunch -Action $taskAction -Trigger $taskTrigger -Force | Out-Null
    if($Scenario -in @('exe-msi','script-msi')) { Disable-ScheduledTask -TaskName MineShareLaunch | Out-Null }
    $taskBefore=Export-ScheduledTask -TaskName MineShareLaunch
}
$runKey='HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
New-Item -Path $runKey -Force | Out-Null
$runValue='"'+$installed+'" --fixture-startup-args'
if($oldKind -ne 'fresh') { New-ItemProperty -LiteralPath $runKey -Name MineShare -Value $runValue -PropertyType String -Force | Out-Null }
$newMsi=(Get-ChildItem $NewDirectory -Recurse -Filter *.msi | Select-Object -First 1).FullName
$newExe=(Get-ChildItem $NewDirectory -Recurse -Filter *-setup.exe | Select-Object -First 1).FullName
& "$PSScriptRoot/test-msi-package.ps1" -MsiFile $newMsi
if($Scenario -in @('rollback','rollback-nsis','downgrade')) {
    # Fault injection changes a test copy only, never the distributed package.
    $fault=Join-Path $env:RUNNER_TEMP 'rollback.msi'
    Copy-Item -LiteralPath $newMsi -Destination $fault
    $engine=New-Object -ComObject WindowsInstaller.Installer
    $db=$engine.OpenDatabase($fault,1)
    $statements=if($Scenario -in @('rollback','rollback-nsis')) { @(
        "INSERT INTO ``CustomAction`` (``Action``,``Type``,``Target``) VALUES ('FixtureFail',19,'Intentional rollback test')",
        "INSERT INTO ``InstallExecuteSequence`` (``Action``,``Condition``,``Sequence``) VALUES ('FixtureFail','NOT REMOVE',6599)"
    ) } else { @("UPDATE ``Property`` SET ``Value`` = '0.0.11' WHERE ``Property`` = 'ProductVersion'") }
    foreach($sql in $statements) {
        $view=$db.OpenView($sql); $null=$view.Execute(); $null=$view.Close()
        $null=[Runtime.InteropServices.Marshal]::FinalReleaseComObject($view)
    }
    $null=$db.Commit()
    # Committing does not close the database's write lock. Release it before
    # msiexec tries to open the modified test copy (otherwise error 1619/2203).
    $null=[Runtime.InteropServices.Marshal]::FinalReleaseComObject($db)
    $null=[Runtime.InteropServices.Marshal]::FinalReleaseComObject($engine)
    Install $fault $true
    Assert ((Get-FileHash -LiteralPath $installed).Hash -eq $oldHash) 'Rollback did not restore old executable'
    Assert ((Get-Item $installed).VersionInfo.ProductVersion -eq '0.0.12') 'Rollback version mismatch'
} else {
    if($newKind -eq 'msi') { Install $newMsi } else { Install $newExe }
    $newVersion=(Get-Content (Join-Path $PSScriptRoot '../../ui/src-tauri/tauri.conf.json') -Raw | ConvertFrom-Json).version
    Assert ((Get-Item -LiteralPath $installed).VersionInfo.ProductVersion -eq $newVersion) 'Wrong installed version or second installation path'
    if($oldKind -eq 'exe') { Assert (-not (Test-Path (Join-Path $location 'uninstall.exe'))) 'Old NSIS uninstaller was not retired' }
    # Run the SAME payload again: maintenance must not add another product.
    if($newKind -eq 'msi') { Install $newMsi } else { Install $newExe }
}
Assert (@(Registrations).Count -eq 1) 'Duplicate or missing installed product registration'
$transactions=Join-Path $env:LOCALAPPDATA 'MineShareInstaller'
Assert (@(Get-ChildItem -LiteralPath $transactions -Filter *.json -ErrorAction SilentlyContinue).Count -eq 0) 'Installer transaction/rollback did not finish'
foreach($name in $before.Keys) {
    Assert ((Get-FileHash -LiteralPath (Join-Path $data $name)).Hash -eq $before[$name]) "User data changed: $name"
}
if($oldKind -ne 'fresh') {
    Assert ((Export-ScheduledTask -TaskName MineShareLaunch) -ceq $taskBefore) 'Startup task XML/enabled state changed'
    Assert ((Get-ItemProperty -LiteralPath $runKey).MineShare -ceq $runValue) 'Startup Run value changed'
} else {
    Assert (-not (Get-ScheduledTask -TaskName MineShareLaunch -ErrorAction SilentlyContinue)) 'Fresh install enabled startup without consent'
    Assert (-not (Get-ItemProperty -LiteralPath $runKey -Name MineShare -ErrorAction SilentlyContinue)) 'Fresh install created a Run startup value'
}
Write-Output "PASS: $Scenario; original location, pairing, settings, startup and single product registration preserved"
