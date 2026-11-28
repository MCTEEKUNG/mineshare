package com.mineshare.mobile

import android.content.Context
import android.content.Intent
import android.content.res.Configuration
import android.net.Uri
import android.provider.Settings
import android.view.Surface

object PhoneRotation {
    fun apply(context: Context, portrait: Boolean? = null, automatic: Boolean = false) {
        if (!Settings.System.canWrite(context)) {
            BridgeState.error("อนุญาตให้ MineShare เปลี่ยนการตั้งค่าระบบ แล้วกดคีย์ลัดหมุนจออีกครั้ง")
            context.startActivity(Intent(Settings.ACTION_MANAGE_WRITE_SETTINGS, Uri.parse("package:${context.packageName}"))
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            return
        }
        runCatching {
            val resolver = context.contentResolver
            if (automatic) {
                check(Settings.System.putInt(resolver, Settings.System.ACCELEROMETER_ROTATION, 1))
            } else {
                val targetPortrait = portrait ?: (context.resources.configuration.orientation != Configuration.ORIENTATION_PORTRAIT)
                check(Settings.System.putInt(resolver, Settings.System.USER_ROTATION,
                    if (targetPortrait) Surface.ROTATION_0 else Surface.ROTATION_90))
                check(Settings.System.putInt(resolver, Settings.System.ACCELEROMETER_ROTATION, 0))
            }
            if (BridgeState.state.value.error?.startsWith("อนุญาตให้ MineShare") == true)
                BridgeState.state.value = BridgeState.state.value.copy(error = null)
        }.onFailure { BridgeState.error("หมุนจอไม่ได้: ${it.message}") }
    }
}
