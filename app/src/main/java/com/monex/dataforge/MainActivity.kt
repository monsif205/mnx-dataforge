package com.monex.dataforge

import android.app.Activity
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
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
import com.monex.dataforge.ui.Glass
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
    AppTheme(dark, st.lang, !st.reduceMotion) {
        AppBackground {
            BackHandler(enabled = st.stack.isNotEmpty()) { st.pop() }
            val target: Any = st.stack.lastOrNull() ?: st.tab
            AnimatedContent(
                targetState = target,
                transitionSpec = {
                    if (st.reduceMotion) EnterTransition.None togetherWith ExitTransition.None
                    else fadeIn(tween(200)) togetherWith fadeOut(tween(120))
                },
                modifier = Modifier.fillMaxSize(),
                label = "screen"
            ) { t ->
                when (t) {
                    is Screen.DatasetS -> DatasetScreen(st, t.id)
                    is Screen.EditorS -> RecordEditor(st, t.datasetId, t.recordId)
                    0 -> HomeScreen(st)
                    1 -> DataScreen(st)
                    2 -> BackupScreen(st)
                    else -> SettingsScreen(st)
                }
            }
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
                    Glass(Modifier.fillMaxWidth().pressable { st.message = null }, strength = 0.9f, radius = 18.dp) {
                        Box(Modifier.padding(14.dp)) { T(m, 13.sp) }
                    }
                }
            }
            st.busy?.let { BusyOverlay(it, st.busyDetail) { st.cancelFlag.set(true) } }
        }
    }
}
