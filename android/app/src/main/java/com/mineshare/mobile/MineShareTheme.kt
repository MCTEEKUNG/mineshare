package com.mineshare.mobile

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.unit.dp

/** Near-black cool surfaces and sky-blue controls keep every Material role on brand. */
private val MineShareColors = darkColorScheme(
    primary = Color(0xff4da3ff), onPrimary = Color(0xff00213f),
    primaryContainer = Color(0xff0d3b66), onPrimaryContainer = Color(0xffd3e7ff),
    secondary = Color(0xffa9c4e4), onSecondary = Color(0xff12304d),
    secondaryContainer = Color(0xff1f3247), onSecondaryContainer = Color(0xffd4e4f6),
    tertiary = Color(0xff7fd3e0), onTertiary = Color(0xff00363e),
    tertiaryContainer = Color(0xff004f5a), onTertiaryContainer = Color(0xffa5eefb),
    background = Color(0xff0a0c0f), onBackground = Color(0xffe8edf3),
    surface = Color(0xff13161b), onSurface = Color(0xffe8edf3),
    surfaceVariant = Color(0xff262c35), onSurfaceVariant = Color(0xffa9b3c0),
    surfaceTint = Color(0xff4da3ff),
    surfaceDim = Color(0xff0a0c0f), surfaceBright = Color(0xff262c35),
    surfaceContainerLowest = Color(0xff060709), surfaceContainerLow = Color(0xff0f1115),
    surfaceContainer = Color(0xff13161b), surfaceContainerHigh = Color(0xff18202b),
    surfaceContainerHighest = Color(0xff262c35),
    outline = Color(0xff7d8896), outlineVariant = Color(0xff333a44),
    error = Color(0xffffb4ab), onError = Color(0xff690005),
    errorContainer = Color(0xff93000a), onErrorContainer = Color(0xffffdad6),
    inverseSurface = Color(0xffe2e7ee), inverseOnSurface = Color(0xff1b2026),
    inversePrimary = Color(0xff0061a6), scrim = Color.Black,
)

@Composable fun MineShareTheme(content: @Composable () -> Unit) {
    MaterialTheme(colorScheme = MineShareColors, content = content)
}

/** Soft light sources behind the glass. Radial gradients are already soft, so no per-frame blur is needed.
 *  Peak alphas keep primary text on glass at or above WCAG AA (4.87:1 worst case). */
@Composable fun GlassBackground(modifier: Modifier = Modifier, content: @Composable BoxScope.() -> Unit) {
    Box(modifier.fillMaxSize().background(MaterialTheme.colorScheme.background).drawBehind {
        fun glow(color: Color, alpha: Float, x: Float, y: Float, radius: Float) {
            val center = Offset(size.width * x, size.height * y)
            val r = size.minDimension * radius
            drawCircle(Brush.radialGradient(listOf(color.copy(alpha = alpha), Color.Transparent), center, r), r, center)
        }
        glow(Color(0xff4da3ff), 0.16f, 0.95f, 0.02f, 1.0f)
        glow(Color(0xff5b6cff), 0.18f, 0.0f, 0.62f, 0.9f)
        glow(Color(0xff7fd3e0), 0.14f, 1.0f, 0.95f, 0.8f)
    }) {
        // Surface used to supply the default text color; keep text light without it.
        CompositionLocalProvider(LocalContentColor provides MaterialTheme.colorScheme.onBackground) { content() }
    }
}

val GlassShape = RoundedCornerShape(24.dp)

/** Frosted pane: a faint white fill plus a light edge that fades from the top, like light catching glass. */
fun Modifier.glass(shape: Shape = GlassShape, fill: Float = 0.07f): Modifier = clip(shape)
    .background(Color.White.copy(alpha = fill), shape)
    .border(1.dp, Brush.verticalGradient(listOf(Color.White.copy(alpha = 0.20f), Color.White.copy(alpha = 0.04f))), shape)
