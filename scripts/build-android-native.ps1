param(
    [string]$Sdk = "$env:LOCALAPPDATA/Android/Sdk",
    [string]$NdkVersion = '29.0.14206865',
    [ValidateSet('arm64-v8a', 'x86_64')][string[]]$Abi = @('arm64-v8a')
)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$ndkRoot = Join-Path $Sdk "ndk/$NdkVersion"
$toolchain = Join-Path $ndkRoot 'toolchains/llvm/prebuilt/windows-x86_64/bin'
if (!(Test-Path -LiteralPath $toolchain)) { throw "Android NDK missing at $ndkRoot" }
$previousEnvironment = @{}
foreach ($name in @(
    'ANDROID_NDK_HOME', 'ANDROID_NDK_ROOT', 'CMAKE_TOOLCHAIN_FILE',
    'ANDROID_PLATFORM', 'ANDROID_ABI', 'OPUS_STATIC', 'OPUS_NO_PKG',
    'CMAKE_GENERATOR', 'PATH',
    'CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER', 'CC_aarch64_linux_android',
    'CXX_aarch64_linux_android', 'AR_aarch64_linux_android',
    'CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER', 'CC_x86_64_linux_android',
    'CXX_x86_64_linux_android', 'AR_x86_64_linux_android'
)) {
    $previousEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
$env:ANDROID_NDK_HOME = $ndkRoot
$env:ANDROID_NDK_ROOT = $ndkRoot
$env:ANDROID_PLATFORM = 'android-34'
$env:OPUS_STATIC = '1'
$env:OPUS_NO_PKG = '1'
$env:CMAKE_GENERATOR = 'Ninja'
$ninjaDirectory = Join-Path $Sdk 'cmake/3.22.1/bin'
$env:PATH = "$toolchain;$ninjaDirectory;$env:PATH"
Push-Location $projectRoot
try {
    foreach ($architecture in $Abi) {
        # CMake's Android toolchain consumes cache variables, not the process
        # ANDROID_ABI/ANDROID_PLATFORM environment variables. The wrapper keeps
        # the Opus build on the same ABI and minimum API level as Rust.
        $cmakeDirectory = Join-Path $projectRoot 'target/android-toolchains'
        New-Item -ItemType Directory -Force -Path $cmakeDirectory | Out-Null
        $cmakeWrapper = Join-Path $cmakeDirectory "$architecture.cmake"
        $ndkToolchain = (Join-Path $ndkRoot 'build/cmake/android.toolchain.cmake').Replace('\', '/')
        @"
set(ANDROID_ABI "$architecture" CACHE STRING "Android ABI" FORCE)
set(ANDROID_PLATFORM "android-34" CACHE STRING "Android API" FORCE)
include("$ndkToolchain")
"@ | Set-Content -LiteralPath $cmakeWrapper -Encoding utf8
        $env:CMAKE_TOOLCHAIN_FILE = $cmakeWrapper
        if ($architecture -eq 'arm64-v8a') {
            $triple = 'aarch64-linux-android'
            $env:ANDROID_ABI = 'arm64-v8a'
            $env:CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER = Join-Path $toolchain 'aarch64-linux-android34-clang.cmd'
            $env:CC_aarch64_linux_android = $env:CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER
            $env:CXX_aarch64_linux_android = Join-Path $toolchain 'aarch64-linux-android34-clang++.cmd'
            $env:AR_aarch64_linux_android = Join-Path $toolchain 'llvm-ar.exe'
        } else {
            $triple = 'x86_64-linux-android'
            $env:ANDROID_ABI = 'x86_64'
            $env:CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER = Join-Path $toolchain 'x86_64-linux-android34-clang.cmd'
            $env:CC_x86_64_linux_android = $env:CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER
            $env:CXX_x86_64_linux_android = Join-Path $toolchain 'x86_64-linux-android34-clang++.cmd'
            $env:AR_x86_64_linux_android = Join-Path $toolchain 'llvm-ar.exe'
        }
        cargo build --release --locked -p mineshare-android --target $triple
        if ($LASTEXITCODE -ne 0) { throw "Native Android build failed for $architecture" }
        $destination = Join-Path $projectRoot "android/app/src/main/jniLibs/$architecture"
        New-Item -ItemType Directory -Force -Path $destination | Out-Null
        Copy-Item -LiteralPath "target/$triple/release/libmineshare_android.so" -Destination $destination -Force
    }
} finally {
    Pop-Location
    foreach ($name in $previousEnvironment.Keys) {
        [Environment]::SetEnvironmentVariable($name, $previousEnvironment[$name], 'Process')
    }
}
