# MineShare for Android

The Android companion joins directly paired Windows and Ubuntu computers over the same local network. It receives mouse and keyboard input on the phone's screen, plays computer audio, and sends permitted phone media audio to selected computers. Each computer and phone pair has its own encrypted connection.

## Build

Install JDK 17 or 21, Android SDK platform 36, build tools 36, Android NDK `29.0.14206865`, CMake `3.22.1` (including Ninja), and Rust. The build script uses Android Studio's bundled JDK when the configured Java version cannot run Gradle 8.13. On Windows, run from the repository root.

```powershell
rustup target add aarch64-linux-android x86_64-linux-android
./scripts/build-android.ps1
```

The script builds the shared Rust network library for ARM64 phones and x86-64 emulators, then builds and lints the APK. The APK is at `android/app/build/outputs/apk/debug/app-debug.apk`. Gradle refuses to package an APK without the ARM64 network library. Use `-SkipNative` for a Kotlin-only rebuild after building native libraries.

The app requires Android 14 or later. It targets Android 17 (SDK 37) using compile SDK 36 because platform 37 was unavailable in the Android SDK repository in this environment. New local network permission requests use the platform permission name and run only on Android 17 or later. The ARM64 APK was verified on a physical Galaxy S24 Ultra (SM-S928B) running Android 17 / One UI 9.

## Set up

1. Install the APK and compatible `control-v5` MineShare builds on the computers.
2. On the phone, tap **Connect over Wi-Fi** and allow local network access when requested. Keep all devices on the same LAN.
3. Tap **Enable phone control**, then enable MineShare in Android Accessibility settings. If Android blocks the switch for a downloaded APK, use **App info → Allow restricted settings** first.
4. Select a nearby computer and confirm pairing on the receiving device. Enter the six-digit code shown by the other device.
5. Place the phone beside a computer in the desktop Layout. Move that computer's cursor across the corresponding edge to control the phone. Right-click goes Back, middle-click goes Home, and the extra Back mouse button opens Recents.
6. Select audio routes per device. **Hear this device on phone** receives computer audio. **Send phone audio here** asks Android to approve a media capture session, then sends phone media audio to that device.

Android's capture policy determines which phone apps can share audio. Calls and protected content may stay on the phone. MineShare excludes its own playback from capture to prevent audio loops. Capture stops when Android revokes projection, and starting it again requires a new approval. The ongoing notification provides a disconnect action. Disabling Accessibility releases computer control.

## Verify

```powershell
cd android
./gradlew.bat :app:assembleDebug :app:lintDebug
```

Physical S24 Ultra verification covered Wi-Fi PIN pairing, cursor movement and edge release, click, drag, wheel scrolling, mixed Thai/English input and Backspace, live portrait/landscape geometry, and simultaneous computer-to-phone audio playback and phone-to-computer audio capture. A permitted offline media source with amplitude 0.03 produced more than 6,000 decoded Opus frames on the computer, with measured peak 0.0313. Android's system Stop sharing action stopped capture, disabled the send route and retained the connection.

Native integration tests exercise four authenticated devices and continued operation after the first device stops. Physical three-device switching, PC shutdown while another real computer controls the phone, sustained congested Wi-Fi recovery, and denied/revoked LAN permission still require acceptance testing on the complete desk setup. The physical input test used a protocol-compatible host probe. Full desktop-hook-to-phone typing still needs that desk acceptance pass.
