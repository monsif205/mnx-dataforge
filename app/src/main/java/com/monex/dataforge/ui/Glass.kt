package com.monex.dataforge.ui

// Real liquid glass. This is the ONLY file that touches the io.github.kyant0:backdrop library,
// so any API difference between library versions is fixed here and nowhere else.

import android.os.Build
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.kyant.backdrop.Backdrop
import com.kyant.backdrop.backdrops.layerBackdrop
import com.kyant.backdrop.backdrops.rememberLayerBackdrop
import com.kyant.backdrop.drawBackdrop
import com.kyant.backdrop.effects.blur
import com.kyant.backdrop.effects.lens
import com.kyant.backdrop.effects.vibrancy

/** The backdrop that glass elements refract. Null outside a [BackdropHost] (glass then falls back to a flat fill). */
val LocalBackdrop = compositionLocalOf<Backdrop?> { null }

/**
 * Wraps the scrolling content. Everything drawn inside is captured so that glass elements placed
 * ABOVE it (dock, top buttons, toasts) blur and refract what is behind them.
 * [overlay] is drawn on top of the captured layer and may use [LiquidGlass].
 */
@Composable
fun BackdropHost(
    modifier: Modifier = Modifier,
    background: Color,
    content: @Composable BoxScope.() -> Unit,
    overlay: @Composable BoxScope.() -> Unit
) {
    val backdrop = rememberLayerBackdrop()
    CompositionLocalProvider(LocalBackdrop provides backdrop) {
        Box(modifier) {
            Box(Modifier.fillMaxSize().background(background).layerBackdrop(backdrop), content = content)
            overlay()
        }
    }
}

/**
 * A glass surface: blur + saturation boost + (Android 13+) edge refraction lens, with a thin lit rim.
 * Falls back to a translucent flat fill when no backdrop is available.
 */
@Composable
fun LiquidGlass(
    modifier: Modifier = Modifier,
    radius: Dp = 28.dp,
    tint: Color = Color.Unspecified,
    content: @Composable BoxScope.() -> Unit
) {
    val p = LocalPalette.current
    val backdrop = LocalBackdrop.current
    val shape = RoundedCornerShape(radius)
    val surface = if (p.dark) Color(0xFF1C1C1E).copy(alpha = 0.40f) else Color.White.copy(alpha = 0.45f)
    val m = if (backdrop == null) {
        modifier.then(Modifier.glassFallback(shape, surface))
    } else {
        modifier.drawBackdrop(
            backdrop = backdrop,
            shape = { shape },
            effects = {
                vibrancy()
                blur(with(this) { 10.dp.toPx() })
                if (Build.VERSION.SDK_INT >= 33) {
                    val h = with(this) { (radius.toPx() * 0.5f).coerceAtMost(16.dp.toPx()) }
                    lens(h, h * 1.4f)
                }
            },
            onDrawSurface = {
                drawRect(surface)
                if (tint != Color.Unspecified) drawRect(tint.copy(alpha = 0.35f))
            }
        )
    }
    Box(m, content = content)
}

private fun Modifier.glassFallback(shape: androidx.compose.ui.graphics.Shape, fill: Color): Modifier =
    this.then(Modifier.background(fill.copy(alpha = 0.85f), shape))
