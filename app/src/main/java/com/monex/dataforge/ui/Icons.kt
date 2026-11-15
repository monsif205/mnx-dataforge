package com.monex.dataforge.ui

// Icon pack: Cupertino-style, duotone (soft tinted fill + rounded stroke). 24x24 SVG paths, stroke 1.8.
// Generated from journal/icons-source.txt design; render check was done in Chromium.

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp

enum class Ic { HOME, DATA, BACKUP, GEAR, PLUS, CHECK, SEARCH, SPLIT, CLOCK, TRASH, COPY, UP, DOWN, BACK, UNDO, REDO, IMPORT, EXPORT, LOCK, SAVE, CLOSE, MERGE, SCISSORS, WRITE, CODE, DB, BOLT, LAYERS, PLAY }

/** (path, filled). Filled parts get the duotone tint under the stroke. */
private val ICON_DATA: Map<Ic, List<Pair<String, Boolean>>> = mapOf(
    Ic.HOME to listOf("M4 11 12 4.2 20 11v8a1.5 1.5 0 0 1-1.5 1.5h-13A1.5 1.5 0 0 1 4 19z" to true, "M10 20.5v-5a2 2 0 0 1 4 0v5" to false),
    Ic.DATA to listOf("M3.5 7A2.5 2.5 0 0 1 6 4.5h3.2a2 2 0 0 1 1.5.7L12 6.5h6A2.5 2.5 0 0 1 20.5 9v8a2.5 2.5 0 0 1-2.5 2.5H6A2.5 2.5 0 0 1 3.5 17z" to true),
    Ic.BACKUP to listOf("M7.2 18.5a4.2 4.2 0 0 1-.5-8.4A5.6 5.6 0 0 1 17.2 9a4.8 4.8 0 0 1-.2 9.5z" to true, "M12 15.5v-6M12 9.5l-2.3 2.3M12 9.5l2.3 2.3" to false),
    Ic.GEAR to listOf("M4 7h8.5M17.5 7H20M4 17h2.5M11.5 17H20" to false, "M15 4.8a2.2 2.2 0 1 1 0 4.4 2.2 2.2 0 0 1 0-4.4z" to true, "M9 14.8a2.2 2.2 0 1 1 0 4.4 2.2 2.2 0 0 1 0-4.4z" to true),
    Ic.PLUS to listOf("M12 5v14M5 12h14" to false),
    Ic.CHECK to listOf("M5 12.8l4.4 4.4L19 7.6" to false),
    Ic.SEARCH to listOf("M10.5 4.5a6 6 0 1 1 0 12 6 6 0 0 1 0-12z" to true, "M15 15l4.8 4.8" to false),
    Ic.SPLIT to listOf("M4 6.5A2.5 2.5 0 0 1 6.5 4H11v16H6.5A2.5 2.5 0 0 1 4 17.5z" to true, "M13 4h4.5A2.5 2.5 0 0 1 20 6.5v11a2.5 2.5 0 0 1-2.5 2.5H13z" to false),
    Ic.CLOCK to listOf("M12 4a8 8 0 1 1 0 16 8 8 0 0 1 0-16z" to true, "M12 8v4.2l2.8 1.8" to false),
    Ic.TRASH to listOf("M7 7.5l.8 10.7A2 2 0 0 0 9.8 20h4.4a2 2 0 0 0 2-1.8L17 7.5z" to true, "M4.5 7.5h15M9.5 4.5h5M10.2 11v5.2M13.8 11v5.2" to false),
    Ic.COPY to listOf("M10.5 9h7A2.5 2.5 0 0 1 20 11.5v6a2.5 2.5 0 0 1-2.5 2.5h-7A2.5 2.5 0 0 1 8 17.5v-6A2.5 2.5 0 0 1 10.5 9z" to true, "M16 6.5A2.5 2.5 0 0 0 13.5 4h-7A2.5 2.5 0 0 0 4 6.5v7A2.5 2.5 0 0 0 6.5 16H8" to false),
    Ic.UP to listOf("M6 14.5l6-6 6 6" to false),
    Ic.DOWN to listOf("M6 9.5l6 6 6-6" to false),
    Ic.BACK to listOf("M15 5.5L8.5 12l6.5 6.5" to false),
    Ic.UNDO to listOf("M9 6.5L4.8 10.7 9 14.9" to false, "M5 10.7h8.2a5 5 0 0 1 0 10H11" to false),
    Ic.REDO to listOf("M15 6.5l4.2 4.2L15 14.9" to false, "M19 10.7h-8.2a5 5 0 0 0 0 10H13" to false),
    Ic.IMPORT to listOf("M5 15v2.5A2.5 2.5 0 0 0 7.5 20h9a2.5 2.5 0 0 0 2.5-2.5V15" to false, "M12 4v10.5M12 14.5l-3.5-3.5M12 14.5l3.5-3.5" to false),
    Ic.EXPORT to listOf("M5 15v2.5A2.5 2.5 0 0 0 7.5 20h9a2.5 2.5 0 0 0 2.5-2.5V15" to false, "M12 14.5V4M12 4L8.5 7.5M12 4l3.5 3.5" to false),
    Ic.LOCK to listOf("M6.5 11h11a1.5 1.5 0 0 1 1.5 1.5v6a1.5 1.5 0 0 1-1.5 1.5h-11A1.5 1.5 0 0 1 5 18.5v-6A1.5 1.5 0 0 1 6.5 11z" to true, "M8 11V8a4 4 0 0 1 8 0v3" to false),
    Ic.SAVE to listOf("M5 6.5A2.5 2.5 0 0 1 7.5 4H15l4 4v9.5a2.5 2.5 0 0 1-2.5 2.5h-9A2.5 2.5 0 0 1 5 17.5z" to true, "M8.5 4v4h5.5V4M8.5 20v-5h7v5" to false),
    Ic.CLOSE to listOf("M6.5 6.5l11 11M17.5 6.5l-11 11" to false),
    Ic.MERGE to listOf("M6 4v3a5 5 0 0 0 5 5h2a5 5 0 0 1 5 5v3M18 4v3a5 5 0 0 1-5 5" to false),
    Ic.SCISSORS to listOf("M6 4.5a2.5 2.5 0 1 1 0 5 2.5 2.5 0 0 1 0-5z" to true, "M6 14.5a2.5 2.5 0 1 1 0 5 2.5 2.5 0 0 1 0-5z" to true, "M8 9.2l11 6.3M8 14.8L19 8.5" to false),
    Ic.WRITE to listOf("M4.5 19.5l.9-4L16.2 4.7a2.1 2.1 0 0 1 3 0l.1.1a2.1 2.1 0 0 1 0 3L8.5 18.6z" to true, "M14.5 6.5l3 3" to false),
    Ic.CODE to listOf("M8 8l-4 4 4 4M16 8l4 4-4 4M13.6 5.5l-3.2 13" to false),
    Ic.DB to listOf("M4 6.5C4 5.1 7.6 4 12 4s8 1.1 8 2.5S16.4 9 12 9 4 7.9 4 6.5z" to true, "M4 6.5v11C4 18.9 7.6 20 12 20s8-1.1 8-2.5v-11M4 12c0 1.4 3.6 2.5 8 2.5s8-1.1 8-2.5" to false),
    Ic.BOLT to listOf("M13.2 3.5L5.5 13.5h5.8l-.9 7 7.9-10.2h-5.8z" to true),
    Ic.LAYERS to listOf("M12 4l8.5 4.2L12 12.4 3.5 8.2z" to true, "M3.5 12.2L12 16.4l8.5-4.2M3.5 16.2L12 20.4l8.5-4.2" to false),
    Ic.PLAY to listOf("M8.2 5.6v12.8a.8.8 0 0 0 1.2.7l10-6.4a.8.8 0 0 0 0-1.4l-10-6.4a.8.8 0 0 0-1.2.7z" to true),
)

@Composable
fun Icon(ic: Ic, tint: Color, size: Dp = 22.dp) {
    val parts = remember(ic) { ICON_DATA.getValue(ic).map { (d, filled) -> PathParser().parsePathString(d).toPath() to filled } }
    val mirror = LocalLayoutDirection.current == LayoutDirection.Rtl && (ic == Ic.BACK || ic == Ic.UNDO || ic == Ic.REDO)
    Canvas(Modifier.size(size)) {
        val s = this.size.width / 24f
        val stroke = Stroke(width = 1.8f, cap = StrokeCap.Round, join = StrokeJoin.Round)
        val fill = tint.copy(alpha = 0.28f)
        fun draw(scope: androidx.compose.ui.graphics.drawscope.DrawScope) = with(scope) {
            scale(s, s, pivot = Offset.Zero) {
                for ((path, filled) in parts) {
                    if (filled) drawPath(path, fill)
                    drawPath(path, tint, style = stroke)
                }
            }
        }
        if (mirror) scale(-1f, 1f, pivot = center) { draw(this) } else draw(this)
    }
}
