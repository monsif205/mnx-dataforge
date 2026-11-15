package com.monex.dataforge

import android.app.Activity
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.runtime.remember
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import com.monex.dataforge.ui.popIn
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.EnterTransition
import androidx.compose.animation.ExitTransition
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.view.WindowCompat
import com.monex.dataforge.ui.AppBackground
import com.monex.dataforge.ui.AppState
import com.monex.dataforge.ui.AppTheme
import com.monex.dataforge.ui.BackupScreen
import com.monex.dataforge.ui.BusyOverlay
import com.monex.dataforge.ui.DataScreen
import com.monex.dataforge.ui.DatasetScreen
import com.monex.dataforge.ui.Dock
import com.monex.dataforge.ui.BackdropHost
import com.monex.dataforge.ui.LiquidGlass
import com.monex.dataforge.ui.WriteScreen
import com.monex.dataforge.ui.HomeScreen
import com.monex.dataforge.ui.Ic
import com.monex.dataforge.ui.RecordEditor
import com.monex.dataforge.ui.Screen
import com.monex.dataforge.ui.SettingsScreen
import com.monex.dataforge.ui.T
import com.monex.dataforge.ui.pressable
import com.monex.dataforge.ui.tr
import kotlinx.coroutines.delay

class MainActivity : ComponentActivity() {
    private lateinit var state: AppState

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // Normally done by DfApp; repeated here (idempotent) so logging never depends on the Application class loading.
        com.monex.dataforge.core.Logger.init(applicationContext)
        enableEdgeToEdge()
        state = AppState(applicationContext)
        state.init()
        setContent { App(state) }
    }

    override fun onDestroy() {
        super.onDestroy()
        if (isFinishing) state.repo.close()
    }
}

@Composable
fun App(st: AppState) {
    val sysDark = isSystemInDarkTheme()
    val dark = when (st.themeMode) { "dark" -> true; "light" -> false; else -> sysDark }
    val view = LocalView.current
    SideEffect {
        val w = (view.context as Activity).window
        val c = WindowCompat.getInsetsController(w, view)
        c.isAppearanceLightStatusBars = !dark
        c.isAppearanceLightNavigationBars = !dark
    }
    LaunchedEffect(Unit) {
        if (com.monex.dataforge.core.Logger.hadCrash()) {
            com.monex.dataforge.core.Logger.clearCrashFlag()
            st.toast(tr("The app closed unexpectedly last time. Open Settings → Logs to send the report.", "أُغلق التطبيق بشكل غير متوقع آخر مرة. افتح الإعدادات ← السجلات لإرسال التقرير."))
        }
    }
    AppTheme(dark, st.lang, !st.reduceMotion) {
        val bg = com.monex.dataforge.ui.LocalPalette.current.bgTop
        AppBackground {
            BackHandler(enabled = st.stack.isNotEmpty()) { st.pop() }
            BackdropHost(Modifier.fillMaxSize(), background = bg, content = {
                val target: Any = st.stack.lastOrNull() ?: st.tab
                var prevDepth by remember { mutableIntStateOf(0) }
                val deeper = st.stack.size >= prevDepth
                SideEffect { prevDepth = st.stack.size }
                AnimatedContent(
                    targetState = target,
                    transitionSpec = {
                        if (st.reduceMotion) EnterTransition.None togetherWith ExitTransition.None
                        else if (initialState is Int && targetState is Int) fadeIn(tween(220)) togetherWith fadeOut(tween(140))
                        else {
                            val d = if (deeper) 1 else -1
                            (slideInHorizontally(tween(320, easing = FastOutSlowInEasing)) { w -> d * w / 3 } + fadeIn(tween(260))) togetherWith
                                (slideOutHorizontally(tween(260, easing = FastOutSlowInEasing)) { w -> -d * w / 5 } + fadeOut(tween(160)))
                        }
                    },
                    modifier = Modifier.fillMaxSize(),
                    label = "screen"
                ) { t ->
                    when (t) {
                        is Screen.DatasetS -> DatasetScreen(st, t.id)
                        is Screen.EditorS -> RecordEditor(st, t.datasetId, t.recordId)
                        is Screen.WriteS -> WriteScreen(st, t.datasetId)
                        Screen.LogsS -> com.monex.dataforge.ui.LogsScreen(st)
                        0 -> HomeScreen(st)
                        1 -> DataScreen(st)
                        2 -> BackupScreen(st)
                        else -> SettingsScreen(st)
                    }
                }
            }, overlay = {
                if (st.stack.isEmpty()) {
                    Box(Modifier.align(Alignment.BottomCenter).navigationBarsPadding().padding(bottom = 10.dp)) {
                        Dock(
                            listOf(
                                Triple(Ic.HOME, tr("Home", "الرئيسية"), st.tab == 0),
                                Triple(Ic.DATA, tr("Data", "البيانات"), st.tab == 1),
                                Triple(Ic.BACKUP, tr("Backup", "النسخ"), st.tab == 2),
                                Triple(Ic.GEAR, tr("Settings", "الإعدادات"), st.tab == 3)
                            ),
                            { st.tab = it }
                        )
                    }
                }
                st.message?.let { m ->
                    LaunchedEffect(m) { delay(4000); if (st.message == m) st.message = null }
                    Box(Modifier.align(Alignment.BottomCenter).navigationBarsPadding().padding(start = 24.dp, end = 24.dp, bottom = if (st.stack.isEmpty()) 100.dp else 24.dp)) {
                        LiquidGlass(Modifier.fillMaxWidth().popIn().pressable { st.message = null }, radius = 28.dp, refraction = 14.dp) {
                            Box(Modifier.padding(horizontal = 20.dp, vertical = 14.dp)) { T(m, 14.sp) }
                        }
                    }
                }
            })
            st.busy?.let { BusyOverlay(it, st.busyDetail) { st.cancelFlag.set(true) } }
        }
    }
}
