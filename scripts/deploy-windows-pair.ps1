param(
    [string]$RemoteHost = '192.168.1.104',
    [string]$RemoteUser = 'Teeza',
    [string]$IdentityFile = "$env:USERPROFILE\.ssh\mineshare_ed25519",
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
$RepoRoot = Split-Path -Parent $PSScriptRoot
$UiRoot = Join-Path $RepoRoot 'ui'
$BuiltExe = Join-Path $UiRoot 'src-tauri\target\release\mineshare-app.exe'
$BuiltIcon = Join-Path $UiRoot 'src-tauri\icons\icon.ico'
$Installer = Join-Path $RepoRoot 'installer\windows\install.ps1'
$Remote = "$RemoteUser@$RemoteHost"
$RemoteStage = 'C:/Users/{0}/AppData/Local/Temp/MineShareDeploy' -f $RemoteUser

if (-not $SkipBuild) {
    Push-Location $UiRoot
    try {
        npm test -- --run
        if ($LASTEXITCODE -ne 0) { throw 'UI tests failed' }
        npm run tauri build -- --no-bundle
        if ($LASTEXITCODE -ne 0) { throw 'Tauri release build failed' }
    }
    finally {
        Pop-Location
    }
}

foreach ($path in @($BuiltExe, $BuiltIcon, $Installer, $IdentityFile)) {
    if (-not (Test-Path -LiteralPath $path)) {
        throw "Required deployment file missing: $path"
    }
}

$ExpectedHash = (Get-FileHash -LiteralPath $BuiltExe -Algorithm SHA256).Hash
Write-Host "==> release SHA-256: $ExpectedHash"

Write-Host '==> staging release on peer'
$RemoteStageCommand = "New-Item -ItemType Directory -Force -Path '$RemoteStage' | Out-Null"
$RemoteStageEncoded = [Convert]::ToBase64String(
    [Text.Encoding]::Unicode.GetBytes($RemoteStageCommand)
)
ssh -i $IdentityFile -o BatchMode=yes $Remote `
    "powershell.exe -NoProfile -EncodedCommand $RemoteStageEncoded"
if ($LASTEXITCODE -ne 0) { throw 'Could not create remote staging directory' }
scp -i $IdentityFile $BuiltExe "${Remote}:${RemoteStage}/mineshare-app.exe"
if ($LASTEXITCODE -ne 0) { throw 'Could not upload remote executable' }
scp -i $IdentityFile $BuiltIcon "${Remote}:${RemoteStage}/icon.ico"
if ($LASTEXITCODE -ne 0) { throw 'Could not upload remote icon' }
scp -i $IdentityFile $Installer "${Remote}:${RemoteStage}/install.ps1"
if ($LASTEXITCODE -ne 0) { throw 'Could not upload remote installer' }

Write-Host '==> installing release on local Laptop'
& $Installer -BuiltExe $BuiltExe -BuiltIcon $BuiltIcon

Write-Host '==> installing release on peer PC'
ssh -i $IdentityFile -o BatchMode=yes $Remote "powershell.exe -NoProfile -ExecutionPolicy Bypass -File $RemoteStage/install.ps1 -BuiltExe $RemoteStage/mineshare-app.exe -BuiltIcon $RemoteStage/icon.ico"
if ($LASTEXITCODE -ne 0) { throw 'Remote installation failed' }

Start-Sleep -Seconds 3
$LocalInstalled = Join-Path $env:LOCALAPPDATA 'MineShare\mineshare-app.exe'
$LocalHash = (Get-FileHash -LiteralPath $LocalInstalled -Algorithm SHA256).Hash
$RemoteHashCommand = "`$ProgressPreference='SilentlyContinue'; (Get-FileHash -Algorithm SHA256 -LiteralPath 'C:/Users/$RemoteUser/AppData/Local/MineShare/mineshare-app.exe').Hash"
$RemoteHashEncoded = [Convert]::ToBase64String(
    [Text.Encoding]::Unicode.GetBytes($RemoteHashCommand)
)
$RemoteHash = (
    ssh -i $IdentityFile -o BatchMode=yes $Remote `
        "powershell.exe -NoProfile -EncodedCommand $RemoteHashEncoded"
).Trim()

if ($LocalHash -ne $ExpectedHash) {
    throw "Local installed hash mismatch: $LocalHash"
}
if ($RemoteHash -ne $ExpectedHash) {
    throw "Remote installed hash mismatch: $RemoteHash"
}

Write-Host '==> pair deployment verified'
Write-Host "  local:  $LocalHash"
Write-Host "  remote: $RemoteHash"
