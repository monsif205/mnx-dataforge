package com.monex.dataforge.ui

import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.State
import androidx.compose.runtime.produceState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

fun fmtBytes(b: Long): String = when {
    b < 1024 -> "$b B"
    b < 1024 * 1024 -> "%.1f KB".format(Locale.US, b / 1024.0)
    b < 1024L * 1024 * 1024 -> "%.1f MB".format(Locale.US, b / 1048576.0)
    else -> "%.2f GB".format(Locale.US, b / 1073741824.0)
}

fun fmtTime(ms: Long): String = SimpleDateFormat("yyyy-MM-dd HH:mm", Locale.US).format(Date(ms))

/** Loads [block] on the IO thread; re-runs when any key changes. On failure keeps [initial]. */
@Composable
fun <T> ioState(initial: T, vararg keys: Any?, block: () -> T): State<T> =
    produceState(initial, *keys) {
        value = withContext(Dispatchers.IO) { try { block() } catch (e: Throwable) { initial } }
    }

/**
 * Screen frame. The title and the screen content live in the captured layer; the back/action buttons float above it
 * as real glass, so they refract what is behind them.
 */
@Composable
fun Page(title: String, onBack: (() -> Unit)? = null, actions: @Composable RowScope.() -> Unit = {}, content: @Composable ColumnScope.() -> Unit) {
    val p = LocalPalette.current
    BackdropHost(Modifier.fillMaxSize(), background = p.bgTop, content = {
        Column(Modifier.fillMaxSize().statusBarsPadding()) {
            Box(Modifier.fillMaxWidth().height(58.dp).padding(start = if (onBack != null) 66.dp else 18.dp, end = 18.dp), contentAlignment = Alignment.CenterStart) {
                T(title, if (onBack == null) 30.sp else 20.sp, FontWeight.Bold, maxLines = 1)
            }
            content()
        }
    }, overlay = {
        Row(
            Modifier.fillMaxWidth().statusBarsPadding().padding(horizontal = 14.dp, vertical = 8.dp).height(42.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)
        ) {
            if (onBack != null) IconBtn(Ic.BACK, onBack)
            Box(Modifier.weight(1f))
            actions()
        }
    })
}

@Composable
fun ChipRow(content: @Composable RowScope.() -> Unit) {
    Row(
        Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(vertical = 4.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp), content = content
    )
}

@Composable
fun Modal(onDismiss: () -> Unit, content: @Composable ColumnScope.() -> Unit) {
    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Glass(Modifier.padding(20.dp).fillMaxWidth().popIn(), strength = 0.95f, radius = 26.dp) {
            Column(
                Modifier.padding(18.dp).heightIn(max = 560.dp).verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(10.dp), content = content
            )
        }
    }
}

@Composable
fun ConfirmDialog(title: String, text: String, confirm: String, danger: Boolean, onConfirm: () -> Unit, onDismiss: () -> Unit) {
    Modal(onDismiss) {
        T(title, 18.sp, FontWeight.Bold)
        T(text, 14.sp, color = LocalPalette.current.textDim)
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            GButton(tr("Cancel", "إلغاء"), onDismiss, Modifier.weight(1f))
            GButton(confirm, { onDismiss(); onConfirm() }, Modifier.weight(1f), accent = !danger, danger = danger)
        }
    }
}

@Composable
fun PromptDialog(title: String, initial: String, confirm: String, onConfirm: (String) -> Unit, onDismiss: () -> Unit) {
    var v = androidx.compose.runtime.remember { androidx.compose.runtime.mutableStateOf(initial) }
    Modal(onDismiss) {
        T(title, 18.sp, FontWeight.Bold)
        Field(v.value, { v.value = it }, Modifier.fillMaxWidth(), singleLine = true)
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            GButton(tr("Cancel", "إلغاء"), onDismiss, Modifier.weight(1f))
            GButton(confirm, { onDismiss(); onConfirm(v.value) }, Modifier.weight(1f), accent = true)
        }
    }
}

@Composable
fun BarRow(label: String, value: Int, max: Int, color: Color) {
    val p = LocalPalette.current
    Column(Modifier.fillMaxWidth().padding(vertical = 3.dp)) {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
            T(label, 13.sp, maxLines = 1, modifier = Modifier.weight(1f))
            T(value.toString(), 13.sp, FontWeight.SemiBold, p.textDim)
        }
        Box(Modifier.fillMaxWidth().height(8.dp).clip(RoundedCornerShape(4.dp)).background(p.field)) {
            val f = if (max <= 0) 0f else (value.toFloat() / max).coerceIn(0f, 1f)
            Box(Modifier.fillMaxWidth(f).height(8.dp).clip(RoundedCornerShape(4.dp)).background(color))
        }
    }
}

@Composable
fun StatTile(label: String, value: String, modifier: Modifier = Modifier) {
    Glass(modifier, strength = 0.55f, radius = 18.dp) {
        Column(Modifier.padding(14.dp)) {
            T(value, 24.sp, FontWeight.Bold)
            T(label, 12.sp, color = LocalPalette.current.textDim)
        }
    }
}

@Composable
fun BusyOverlay(label: String, detail: String, onCancel: () -> Unit) {
    val pulse = rememberInfiniteTransition(label = "busy")
    val a by pulse.animateFloat(0.45f, 1f, infiniteRepeatable(tween(900), RepeatMode.Reverse), label = "a")
    Box(Modifier.fillMaxSize().background(Color(0x66000000)), contentAlignment = Alignment.Center) {
        Glass(Modifier.padding(32.dp).fillMaxWidth(), strength = 0.95f, radius = 26.dp) {
            Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                T(label, 17.sp, FontWeight.Bold, modifier = Modifier.alpha(a))
                if (detail.isNotEmpty()) T(detail, 13.sp, color = LocalPalette.current.textDim)
                GButton(tr("Cancel", "إلغاء"), onCancel, Modifier.fillMaxWidth())
            }
        }
    }
}
