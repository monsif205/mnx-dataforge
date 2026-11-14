package com.monex.dataforge.ui

import androidx.compose.animation.core.AnimationSpec
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.snap
import androidx.compose.animation.core.spring
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicText
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDirection
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.platform.LocalLayoutDirection

// ------------------------------------------------------------------ palette

class Palette(
    val dark: Boolean,
    val bgTop: Color, val bgBottom: Color,
    val blob1: Color, val blob2: Color, val blob3: Color,
    val text: Color, val textDim: Color,
    val glassFill: Color, val glassEdge: Color,
    val accent: Color, val accent2: Color,
    val danger: Color, val warn: Color, val ok: Color,
    val field: Color
)

val LightPalette = Palette(
    dark = false,
    bgTop = Color(0xFFE9F0FF), bgBottom = Color(0xFFF6F1FF),
    blob1 = Color(0x665B8CFF), blob2 = Color(0x55B07CFF), blob3 = Color(0x4458D6C8),
    text = Color(0xFF0E1626), textDim = Color(0xFF55627A),
    glassFill = Color.White, glassEdge = Color.White,
    accent = Color(0xFF2F6BFF), accent2 = Color(0xFF6D8DFF),
    danger = Color(0xFFD7263D), warn = Color(0xFFB7791F), ok = Color(0xFF1E8E5A),
    field = Color(0x14000000)
)

val DarkPalette = Palette(
    dark = true,
    bgTop = Color(0xFF060A14), bgBottom = Color(0xFF111A33),
    blob1 = Color(0x553D6BFF), blob2 = Color(0x449B5CFF), blob3 = Color(0x3320C9B5),
    text = Color(0xFFEAF0FF), textDim = Color(0xFF9AA8C4),
    glassFill = Color(0xFF1C2742), glassEdge = Color(0xFFBFD0FF),
    accent = Color(0xFF6D9BFF), accent2 = Color(0xFF8FB0FF),
    danger = Color(0xFFFF6B7D), warn = Color(0xFFF2B34E), ok = Color(0xFF4FD39A),
    field = Color(0x22FFFFFF)
)

val LocalPalette = compositionLocalOf { LightPalette }
val LocalLang = compositionLocalOf { "en" }
val LocalMotion = compositionLocalOf { true }

@Composable
fun tr(en: String, ar: String): String = if (LocalLang.current == "ar") ar else en

@Composable
fun AppTheme(dark: Boolean, lang: String, motion: Boolean, content: @Composable () -> Unit) {
    CompositionLocalProvider(
        LocalPalette provides (if (dark) DarkPalette else LightPalette),
        LocalLang provides lang,
        LocalMotion provides motion,
        LocalLayoutDirection provides (if (lang == "ar") LayoutDirection.Rtl else LayoutDirection.Ltr),
        content = content
    )
}

@Composable
fun <T> motion(): AnimationSpec<T> =
    if (LocalMotion.current) spring(dampingRatio = 0.78f, stiffness = Spring.StiffnessMediumLow) else snap()

// ------------------------------------------------------------------ background and glass material

@Composable
fun AppBackground(modifier: Modifier = Modifier, content: @Composable BoxScope.() -> Unit) {
    val p = LocalPalette.current
    Box(
        modifier
            .fillMaxSize()
            .background(Brush.verticalGradient(listOf(p.bgTop, p.bgBottom)))
            .drawBehind {
                // Static soft light sources behind the glass (cheap: three radial gradients, no blur pass).
                val w = size.width; val h = size.height
                drawCircle(Brush.radialGradient(listOf(p.blob1, Color.Transparent), center = Offset(w * 0.15f, h * 0.12f), radius = w * 0.75f), radius = w * 0.75f, center = Offset(w * 0.15f, h * 0.12f))
                drawCircle(Brush.radialGradient(listOf(p.blob2, Color.Transparent), center = Offset(w * 0.9f, h * 0.45f), radius = w * 0.7f), radius = w * 0.7f, center = Offset(w * 0.9f, h * 0.45f))
                drawCircle(Brush.radialGradient(listOf(p.blob3, Color.Transparent), center = Offset(w * 0.25f, h * 0.92f), radius = w * 0.8f), radius = w * 0.8f, center = Offset(w * 0.25f, h * 0.92f))
            },
        content = content
    )
}

/**
 * Glass surface. `strength` 0..1: low = airy chrome (dock, headers), high = near-opaque panel for data-heavy
 * areas so text stays readable. The material is a translucent gradient fill, a lit top edge (specular
 * highlight), a gradient hairline border and a soft shadow. It is not true backdrop refraction.
 */
@Composable
fun Glass(
    modifier: Modifier = Modifier,
    strength: Float = 0.5f,
    radius: Dp = 22.dp,
    content: @Composable BoxScope.() -> Unit
) {
    val p = LocalPalette.current
    val shape = RoundedCornerShape(radius)
    val a = (0.22f + 0.62f * strength).coerceIn(0f, 0.95f)
    val hi = if (p.dark) 0.10f else 0.40f
    Box(
        modifier
            .shadow(if (p.dark) 8.dp else 12.dp, shape, ambientColor = Color(0x22000000), spotColor = Color(0x33000000))
            .clip(shape)
            .background(Brush.verticalGradient(listOf(p.glassFill.copy(alpha = (a + 0.10f).coerceAtMost(0.97f)), p.glassFill.copy(alpha = a))))
            .drawBehind {
                drawRect(Brush.verticalGradient(listOf(Color.White.copy(alpha = hi), Color.Transparent), endY = size.height * 0.4f))
            }
            .border(
                BorderStroke(1.dp, Brush.linearGradient(listOf(p.glassEdge.copy(alpha = if (p.dark) 0.35f else 0.95f), p.glassEdge.copy(alpha = 0.08f), p.glassEdge.copy(alpha = if (p.dark) 0.2f else 0.55f)))),
                shape
            ),
        content = content
    )
}

@Composable
fun Modifier.pressable(enabled: Boolean = true, onClick: () -> Unit): Modifier {
    val src = remember { MutableInteractionSource() }
    val pressed by src.collectIsPressedAsState()
    val s by animateFloatAsState(if (pressed && enabled) 0.95f else 1f, animationSpec = motion(), label = "press")
    return this
        .graphicsLayer { scaleX = s; scaleY = s }
        .clickable(interactionSource = src, indication = null, enabled = enabled, onClick = onClick)
}

// ------------------------------------------------------------------ text and controls

@Composable
fun T(
    text: String, size: TextUnit = 15.sp, weight: FontWeight = FontWeight.Normal, color: Color? = null,
    modifier: Modifier = Modifier, maxLines: Int = Int.MAX_VALUE, mono: Boolean = false
) {
    val p = LocalPalette.current
    BasicText(
        text,
        modifier = modifier,
        maxLines = maxLines,
        overflow = if (maxLines == Int.MAX_VALUE) TextOverflow.Clip else TextOverflow.Ellipsis,
        style = TextStyle(
            color = color ?: p.text, fontSize = size, fontWeight = weight, textDirection = TextDirection.Content,
            fontFamily = if (mono) FontFamily.Monospace else FontFamily.Default, lineHeight = size * 1.35f
        )
    )
}

@Composable
fun GButton(text: String, onClick: () -> Unit, modifier: Modifier = Modifier, icon: Ic? = null, accent: Boolean = false, danger: Boolean = false, enabled: Boolean = true) {
    val p = LocalPalette.current
    val shape = RoundedCornerShape(16.dp)
    val fg = if (accent) Color.White else if (danger) p.danger else p.text
    val base = modifier
        .pressable(enabled, onClick)
        .clip(shape)
        .then(
            if (accent) Modifier.background(Brush.verticalGradient(listOf(p.accent2, p.accent)))
            else Modifier.background(p.glassFill.copy(alpha = if (p.dark) 0.55f else 0.65f))
        )
        .border(1.dp, p.glassEdge.copy(alpha = if (p.dark) 0.25f else 0.9f), shape)
        .graphicsLayer { alpha = if (enabled) 1f else 0.45f }
        .padding(horizontal = 14.dp, vertical = 10.dp)
    Row(base, verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally)) {
        if (icon != null) Icon(icon, fg, 18.dp)
        if (text.isNotEmpty()) T(text, 14.sp, FontWeight.SemiBold, fg)
    }
}

@Composable
fun IconBtn(icon: Ic, onClick: () -> Unit, enabled: Boolean = true, tint: Color? = null, modifier: Modifier = Modifier) {
    val p = LocalPalette.current
    Box(
        modifier
            .size(40.dp)
            .pressable(enabled, onClick)
            .clip(RoundedCornerShape(14.dp))
            .background(p.glassFill.copy(alpha = if (p.dark) 0.35f else 0.5f))
            .graphicsLayer { alpha = if (enabled) 1f else 0.35f },
        contentAlignment = Alignment.Center
    ) { Icon(icon, tint ?: p.text, 20.dp) }
}

@Composable
fun Chip(text: String, selected: Boolean, onClick: () -> Unit) {
    val p = LocalPalette.current
    val shape = RoundedCornerShape(50)
    Box(
        Modifier
            .pressable(true, onClick)
            .clip(shape)
            .background(if (selected) p.accent.copy(alpha = 0.9f) else p.glassFill.copy(alpha = if (p.dark) 0.4f else 0.55f))
            .border(1.dp, p.glassEdge.copy(alpha = if (p.dark) 0.2f else 0.8f), shape)
            .padding(horizontal = 12.dp, vertical = 7.dp)
    ) { T(text, 13.sp, FontWeight.Medium, if (selected) Color.White else p.text, maxLines = 1) }
}

@Composable
fun Badge(text: String, color: Color) {
    Box(Modifier.clip(RoundedCornerShape(8.dp)).background(color.copy(alpha = 0.18f)).padding(horizontal = 8.dp, vertical = 3.dp)) {
        T(text, 11.sp, FontWeight.SemiBold, color, maxLines = 1)
    }
}

@Composable
fun Field(
    value: String, onChange: (String) -> Unit, modifier: Modifier = Modifier, hint: String = "",
    singleLine: Boolean = false, minHeight: Dp = 44.dp, mono: Boolean = false, enabled: Boolean = true,
    keyboard: KeyboardOptions = KeyboardOptions.Default
) {
    val p = LocalPalette.current
    val shape = RoundedCornerShape(14.dp)
    Box(
        modifier
            .clip(shape)
            .background(p.field)
            .border(1.dp, p.glassEdge.copy(alpha = if (p.dark) 0.15f else 0.7f), shape)
            .padding(horizontal = 12.dp, vertical = 10.dp)
            .heightIn(min = minHeight - 20.dp)
    ) {
        if (value.isEmpty() && hint.isNotEmpty()) T(hint, 14.sp, color = p.textDim)
        BasicTextField(
            value = value, onValueChange = onChange, enabled = enabled, singleLine = singleLine,
            keyboardOptions = keyboard,
            textStyle = TextStyle(
                color = p.text, fontSize = 15.sp, textDirection = TextDirection.Content,
                fontFamily = if (mono) FontFamily.Monospace else FontFamily.Default, lineHeight = 21.sp
            ),
            cursorBrush = SolidColor(p.accent),
            modifier = Modifier.fillMaxWidth()
        )
    }
}

@Composable
fun Toggle(on: Boolean, onChange: (Boolean) -> Unit, label: String) {
    val p = LocalPalette.current
    Row(
        Modifier.fillMaxWidth().pressable(true) { onChange(!on) }.padding(vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.SpaceBetween
    ) {
        T(label, 15.sp, modifier = Modifier.weight(1f, fill = true))
        val x by animateFloatAsState(if (on) 1f else 0f, animationSpec = motion(), label = "toggle")
        Box(Modifier.size(width = 48.dp, height = 28.dp).clip(RoundedCornerShape(50)).background(if (on) p.accent else p.field).padding(3.dp)) {
            Box(Modifier.padding(start = (20.dp * x)).size(22.dp).clip(RoundedCornerShape(50)).background(Color.White))
        }
    }
}

@Composable
fun SectionTitle(text: String) {
    T(text, 13.sp, FontWeight.SemiBold, LocalPalette.current.textDim, Modifier.padding(top = 14.dp, bottom = 6.dp, start = 4.dp))
}

// ------------------------------------------------------------------ icon system (one stroke style, 24 grid)

enum class Ic { HOME, DATA, BACKUP, GEAR, PLUS, CHECK, SEARCH, SPLIT, CLOCK, TRASH, COPY, UP, DOWN, BACK, UNDO, REDO, IMPORT, EXPORT, LOCK, SAVE, CLOSE, MERGE, SCISSORS }

private typealias Line = List<Pair<Float, Float>>

private class Shape2(val lines: List<Line>, val circles: List<Triple<Float, Float, Float>> = emptyList())

private fun l(vararg p: Float): Line = p.toList().chunked(2).map { it[0] to it[1] }

private val ICONS: Map<Ic, Shape2> = mapOf(
    Ic.HOME to Shape2(listOf(l(3f, 11f, 12f, 3f, 21f, 11f), l(5f, 10f, 5f, 21f, 19f, 21f, 19f, 10f), l(10f, 21f, 10f, 14f, 14f, 14f, 14f, 21f))),
    Ic.DATA to Shape2(listOf(l(4f, 5f, 20f, 5f, 20f, 9f, 4f, 9f, 4f, 5f), l(4f, 11f, 20f, 11f, 20f, 15f, 4f, 15f, 4f, 11f), l(4f, 17f, 20f, 17f, 20f, 21f, 4f, 21f, 4f, 17f))),
    Ic.BACKUP to Shape2(listOf(l(12f, 3f, 12f, 15f), l(7f, 10f, 12f, 15f, 17f, 10f), l(4f, 17f, 4f, 21f, 20f, 21f, 20f, 17f))),
    Ic.GEAR to Shape2(listOf(l(4f, 7f, 20f, 7f), l(4f, 17f, 20f, 17f)), listOf(Triple(9f, 7f, 2.4f), Triple(15f, 17f, 2.4f))),
    Ic.PLUS to Shape2(listOf(l(12f, 5f, 12f, 19f), l(5f, 12f, 19f, 12f))),
    Ic.CHECK to Shape2(listOf(l(5f, 12f, 10f, 17f, 19f, 7f))),
    Ic.SEARCH to Shape2(listOf(l(15.5f, 15.5f, 20f, 20f)), listOf(Triple(11f, 11f, 6f))),
    Ic.SPLIT to Shape2(listOf(l(12f, 21f, 12f, 12f), l(12f, 12f, 6f, 4f), l(12f, 12f, 18f, 4f))),
    Ic.CLOCK to Shape2(listOf(l(12f, 7f, 12f, 12f, 16f, 14f)), listOf(Triple(12f, 12f, 9f))),
    Ic.TRASH to Shape2(listOf(l(4f, 7f, 20f, 7f), l(9f, 7f, 9f, 4f, 15f, 4f, 15f, 7f), l(6f, 7f, 7f, 20f, 17f, 20f, 18f, 7f))),
    Ic.COPY to Shape2(listOf(l(8f, 8f, 8f, 3f, 20f, 3f, 20f, 16f, 15f, 16f), l(4f, 8f, 15f, 8f, 15f, 21f, 4f, 21f, 4f, 8f))),
    Ic.UP to Shape2(listOf(l(6f, 15f, 12f, 8f, 18f, 15f))),
    Ic.DOWN to Shape2(listOf(l(6f, 9f, 12f, 16f, 18f, 9f))),
    Ic.BACK to Shape2(listOf(l(15f, 5f, 8f, 12f, 15f, 19f))),
    Ic.UNDO to Shape2(listOf(l(9f, 7f, 4f, 12f, 9f, 17f), l(4f, 12f, 14f, 12f, 19f, 15f, 19f, 19f))),
    Ic.REDO to Shape2(listOf(l(15f, 7f, 20f, 12f, 15f, 17f), l(20f, 12f, 10f, 12f, 5f, 15f, 5f, 19f))),
    Ic.IMPORT to Shape2(listOf(l(12f, 3f, 12f, 14f), l(7f, 9f, 12f, 14f, 17f, 9f), l(4f, 15f, 4f, 21f, 20f, 21f, 20f, 15f))),
    Ic.EXPORT to Shape2(listOf(l(12f, 15f, 12f, 4f), l(7f, 9f, 12f, 4f, 17f, 9f), l(4f, 15f, 4f, 21f, 20f, 21f, 20f, 15f))),
    Ic.LOCK to Shape2(listOf(l(5f, 11f, 19f, 11f, 19f, 21f, 5f, 21f, 5f, 11f), l(8f, 11f, 8f, 8f, 10f, 5f, 14f, 5f, 16f, 8f, 16f, 11f))),
    Ic.SAVE to Shape2(listOf(l(5f, 3f, 17f, 3f, 21f, 7f, 21f, 21f, 5f, 21f, 5f, 3f), l(8f, 3f, 8f, 9f, 16f, 9f, 16f, 3f), l(8f, 21f, 8f, 14f, 16f, 14f, 16f, 21f))),
    Ic.CLOSE to Shape2(listOf(l(6f, 6f, 18f, 18f), l(18f, 6f, 6f, 18f))),
    Ic.MERGE to Shape2(listOf(l(6f, 4f, 12f, 12f), l(18f, 4f, 12f, 12f), l(12f, 12f, 12f, 21f))),
    Ic.SCISSORS to Shape2(listOf(l(8f, 8f, 20f, 18f), l(8f, 16f, 20f, 6f)), listOf(Triple(6f, 6.5f, 2.5f), Triple(6f, 17.5f, 2.5f)))
)

@Composable
fun Icon(ic: Ic, tint: Color, size: Dp = 22.dp) {
    val shape = ICONS.getValue(ic)
    val mirror = LocalLayoutDirection.current == LayoutDirection.Rtl && (ic == Ic.BACK || ic == Ic.UNDO || ic == Ic.REDO)
    Canvas(Modifier.size(size)) {
        val s = this.size.width / 24f
        val st = Stroke(width = 1.9f * s, cap = StrokeCap.Round, join = StrokeJoin.Round)
        fun render(scope: androidx.compose.ui.graphics.drawscope.DrawScope) = with(scope) {
            for (line in shape.lines) {
                val path = Path()
                line.forEachIndexed { i, (x, y) -> if (i == 0) path.moveTo(x * s, y * s) else path.lineTo(x * s, y * s) }
                drawPath(path, tint, style = st)
            }
            for ((cx, cy, r) in shape.circles) drawCircle(tint, r * s, Offset(cx * s, cy * s), style = st)
        }
        if (mirror) scale(-1f, 1f, pivot = center) { render(this) } else render(this)
    }
}

// ------------------------------------------------------------------ dock (bottom navigation)

@Composable
fun Dock(items: List<Triple<Ic, String, Boolean>>, onSelect: (Int) -> Unit, modifier: Modifier = Modifier) {
    val p = LocalPalette.current
    Glass(modifier.fillMaxWidth().padding(horizontal = 18.dp), strength = 0.25f, radius = 30.dp) {
        Row(Modifier.fillMaxWidth().padding(6.dp), horizontalArrangement = Arrangement.SpaceBetween) {
            items.forEachIndexed { i, (ic, label, selected) ->
                val a by animateFloatAsState(if (selected) 1f else 0f, animationSpec = motion(), label = "dock")
                Box(
                    Modifier
                        .weight(1f)
                        .pressable(true) { onSelect(i) }
                        .clip(RoundedCornerShape(24.dp))
                        .background(p.accent.copy(alpha = 0.16f * a))
                        .padding(vertical = 8.dp),
                    contentAlignment = Alignment.Center
                ) {
                    androidx.compose.foundation.layout.Column(horizontalAlignment = Alignment.CenterHorizontally) {
                        Icon(ic, if (selected) p.accent else p.textDim, 22.dp)
                        T(label, 11.sp, if (selected) FontWeight.SemiBold else FontWeight.Normal, if (selected) p.accent else p.textDim, maxLines = 1)
                    }
                }
            }
        }
    }
}
