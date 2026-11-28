package com.mineshare.mobile

/** Native network core shared with the desktop Noise/Opus implementation. */
object NativeBridge {
    val loadError: String? = try {
        System.loadLibrary("mineshare_android")
        null
    } catch (error: UnsatisfiedLinkError) {
        "MineShare network library is missing for this device. Install the complete APK. ${error.message}"
    }

    external fun start(configDir: String, displayName: String, width: Int, height: Int): String
    external fun stop()
    external fun command(json: String)
    external fun poll(): String?
    external fun sendPcm(samples: FloatArray)
    external fun pollAudio(): FloatArray?
}
