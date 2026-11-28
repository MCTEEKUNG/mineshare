param(
    [switch]$SkipNative,
    [ValidateSet('arm64-v8a', 'x86_64')][string[]]$Abi = @('arm64-v8a', 'x86_64')
)
$ErrorActionPreference = 'Stop'
$repositoryRoot = Split-Path -Parent $PSScriptRoot

if (-not $env:ANDROID_HOME) {
    $env:ANDROID_HOME = Join-Path $env:LOCALAPPDATA 'Android/Sdk'
}
$javaMajor = 0
if ($env:JAVA_HOME) {
    $releaseFile = Join-Path $env:JAVA_HOME 'release'
    if (Test-Path -LiteralPath $releaseFile) {
        $releaseText = Get-Content -LiteralPath $releaseFile -Raw
        if ($releaseText -match 'JAVA_VERSION="(\d+)') { $javaMajor = [int]$Matches[1] }
    }
}
if ($javaMajor -lt 17 -or $javaMajor -gt 23) {
    $studioJava = 'C:/Program Files/Android/Android Studio/jbr'
    if (Test-Path -LiteralPath $studioJava) {
        $env:JAVA_HOME = $studioJava
    } else {
        throw 'Set JAVA_HOME to JDK 17 or 21. Gradle 8.13 cannot run on JDK 24 or newer.'
    }
}
if (-not $SkipNative) {
    & (Join-Path $PSScriptRoot 'build-android-native.ps1') -Abi $Abi
    if ($LASTEXITCODE -ne 0) { throw 'Android native library build failed.' }
}

Push-Location (Join-Path $repositoryRoot 'android')
try {
    & ./gradlew.bat :app:testDebugUnitTest :app:assembleDebug :app:lintDebug --console plain
    if ($LASTEXITCODE -ne 0) { throw 'Android APK build or lint failed.' }
} finally {
    Pop-Location
}
$apk = Join-Path $repositoryRoot 'android/app/build/outputs/apk/debug/app-debug.apk'
Write-Host "APK ready at $apk"
