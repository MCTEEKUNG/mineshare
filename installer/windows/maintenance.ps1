# Executed by the MSI's embedded native helper, impersonating the installing user.
# Never delete app configuration, pairing keys, logs, startup entries or directories.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
try {
    $action = $env:MINESHARE_INSTALL_ACTION
    $transaction = [Guid]::Parse($env:MINESHARE_INSTALL_TRANSACTION).ToString('N')
    $directory = [IO.Path]::GetFullPath($env:MINESHARE_INSTALL_DIRECTORY).TrimEnd('\','/')
    $exe = Join-Path $directory 'mineshare-app.exe'
    if (-not [IO.Path]::IsPathRooted($directory) -or $directory -eq [IO.Path]::GetPathRoot($directory).TrimEnd('\')) { throw 'Unsafe installation directory' }
    $stateRoot = Join-Path $env:LOCALAPPDATA 'MineShareInstaller'
    $statePath = Join-Path $stateRoot ($transaction + '.json')
    if ($action -eq 'prepare') {
        $running = @(Get-Process -Name mineshare-app,mineshare-daemon -ErrorAction SilentlyContinue | Where-Object {
            $_.Path -and [IO.Path]::GetDirectoryName($_.Path).TrimEnd('\') -ieq $directory
        })
        # Refuse to create a second installation when an existing startup task
        # points somewhere else. The user must select its existing directory.
        $task = Get-ScheduledTask -TaskName MineShareLaunch -ErrorAction SilentlyContinue
        if ($task) {
            foreach ($entry in $task.Actions) {
                if ($entry.Execute -and [IO.Path]::GetFileName($entry.Execute) -ieq 'mineshare-app.exe' -and
                    [IO.Path]::GetFullPath($entry.Execute) -ine $exe) {
                    throw 'MineShare startup uses another directory. Select the existing installation location.'
                }
            }
        }
        New-Item -ItemType Directory -Path $stateRoot -Force | Out-Null
        @{ Directory=$directory; WasRunning=($running.Count -gt 0) } | ConvertTo-Json | Set-Content -LiteralPath $statePath -Encoding UTF8
        foreach ($process in $running) {
            $null = $process.CloseMainWindow()
            if (-not $process.WaitForExit(1500)) { $process.Kill(); $process.WaitForExit() }
        }
    } elseif ($action -in @('complete','rollback')) {
        if (-not (Test-Path -LiteralPath $statePath)) { exit 0 }
        $state = Get-Content -Raw -LiteralPath $statePath | ConvertFrom-Json
        if ($state.Directory -ine $directory) { throw 'Installer transaction directory mismatch' }
        if ($action -eq 'complete') {
            if (-not (Test-Path -LiteralPath $exe)) { throw 'Installed executable missing' }
            # MSI owns the files now. Remove only a verified old NSIS registration,
            # not its uninstaller (running it would remove our newly installed files).
            foreach ($key in @('HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\MineShare',
                'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\MineShare',
                'HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\MineShare')) {
                $old = Get-ItemProperty -LiteralPath $key -ErrorAction SilentlyContinue
                if (-not $old) { continue }
                $expected = '"' + (Join-Path $directory 'uninstall.exe') + '"'
                if ($old.DisplayName -ne 'MineShare' -or $old.Publisher -ne 'MineShare Contributors' -or
                    $old.UninstallString -ine $expected) { throw 'Unrecognized legacy registration; refusing to remove it' }
                # Exact, fixed registry key only; no registry or filesystem recursion.
                Remove-Item -LiteralPath $key -Force
            }
        }
        if ($state.WasRunning -and (Test-Path -LiteralPath $exe)) {
            Start-Process -FilePath $exe -WorkingDirectory $directory -WindowStyle Hidden | Out-Null
        }
        Remove-Item -LiteralPath $statePath -Force
    } else { throw 'Unknown installer action' }
    exit 0
} catch {
    Write-Error $_
    exit 1
}
