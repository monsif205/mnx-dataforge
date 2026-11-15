package com.monex.dataforge.ui

// Log viewer (copy / save / send to a GitHub issue) and the manual updater card.

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.monex.dataforge.core.Config
import com.monex.dataforge.core.Logger
import com.monex.dataforge.core.Updater

private fun fullReport(): String = Logger.headerLine() + "\n\n" + Logger.read()

private fun copyToClipboard(ctx: Context, text: String) {
    val cm = ctx.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
    cm.setPrimaryClip(ClipData.newPlainText("MNX log", text))
}

/** Opens a pre-filled GitHub "new issue" page in the browser. The full log is also copied to the clipboard. */
private fun sendToGitHub(ctx: Context) {
    val full = fullReport()
    copyToClipboard(ctx, full)
    val tail = if (full.length > 4500) "…\n" + full.takeLast(4500) else full
    val body = "**Log from the app** (the full log is in my clipboard; I will paste it in a comment if needed)\n\n```\n$tail\n```"
    val url = "https://github.com/${Config.GITHUB_REPO}/issues/new?title=" + Uri.encode("App log") + "&body=" + Uri.encode(body)
    ctx.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
}

@Composable
fun LogsScreen(st: AppState) {
    val p = LocalPalette.current
    var refresh by remember { mutableIntStateOf(0) }
    var confirmClear by remember { mutableStateOf(false) }
    val text by ioState("", refresh) { fullReport() }
    val saver = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("text/plain")) { uri ->
        if (uri != null) st.work(tr("Saving…", "جارٍ الحفظ…"), {
            st.ctx.contentResolver.openOutputStream(uri)?.use { it.write(fullReport().toByteArray(Charsets.UTF_8)) }
        }) { st.toast(tr("Log saved", "تم حفظ السجل")) }
    }
    Page(tr("Logs", "السجلات"), onBack = { st.pop() }) {
        Column(Modifier.weight(1f).fillMaxWidth().padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            ChipRow {
                GButton(tr("Send to GitHub", "إرسال لـ GitHub"), { try { sendToGitHub(st.ctx); st.toast(tr("Full log copied", "تم نسخ السجل كاملا")) } catch (e: Exception) { st.toast(e.message ?: "error") } }, icon = Ic.EXPORT, accent = true)
                GButton(tr("Copy", "نسخ"), { copyToClipboard(st.ctx, text); st.toast(tr("Copied", "تم النسخ")) }, icon = Ic.COPY)
                GButton(tr("Save", "حفظ"), { saver.launch("mnx-dataforge-log.txt") }, icon = Ic.SAVE)
                GButton(tr("Refresh", "تحديث"), { refresh++ })
                GButton(tr("Clear", "مسح"), { confirmClear = true }, icon = Ic.TRASH, danger = true)
            }
            Glass(Modifier.weight(1f).fillMaxWidth(), radius = 16.dp) {
                CompositionLocalProvider(LocalLayoutDirection provides LayoutDirection.Ltr) {
                    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(12.dp)) {
                        T(text.ifEmpty { tr("(empty)", "(فارغ)") }, 11.sp, color = p.text, mono = true)
                    }
                }
            }
            T(tr("Nothing is sent automatically. “Send to GitHub” opens a pre-filled issue in your browser.", "لا يُرسل شيء تلقائيا. «إرسال لـ GitHub» يفتح صفحة مشكلة جاهزة في المتصفح."), 12.sp, color = p.textDim)
        }
    }
    if (confirmClear) ConfirmDialog(tr("Clear the log?", "مسح السجل؟"), tr("The log file will be deleted.", "سيتم حذف ملف السجل."), tr("Clear", "مسح"), true, { Logger.clear(); refresh++ }, { confirmClear = false })
}

@Composable
fun UpdateCard(st: AppState) {
    val p = LocalPalette.current
    val current = remember { try { Updater.currentCode(st.ctx) } catch (e: Exception) { 0L } }
    var info by remember { mutableStateOf<Updater.Info?>(null) }
    var status by remember { mutableStateOf<String?>(null) }
    Glass(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            T(tr("App update", "تحديث التطبيق"), 16.sp, FontWeight.SemiBold)
            T(tr("Installed build $current. Updates come from GitHub Releases of ${Config.GITHUB_REPO}; the network is used only when you press the button.",
                "النسخة المثبتة $current. التحديثات من GitHub Releases للمستودع ${Config.GITHUB_REPO}، ولا يُستعمل الإنترنت إلا عند الضغط على الزر."), 13.sp, color = p.textDim)
            status?.let { T(it, 13.sp, color = p.accent) }
            val i = info
            if (i != null && i.available) {
                T(tr("New build ${i.versionCode} available (${i.size / 1048576} MB)", "نسخة جديدة ${i.versionCode} متاحة (${i.size / 1048576} MB)"), 14.sp, FontWeight.SemiBold)
                if (i.notes.isNotBlank()) T(i.notes, 12.sp, color = p.textDim)
                GButton(tr("Download and install", "تنزيل وتثبيت"), {
                    st.work(tr("Downloading update…", "جارٍ تنزيل التحديث…"), { Updater.download(st.ctx, i) { s -> st.progress(s) } }) { f ->
                        val started = Updater.install(st.ctx, f)
                        status = if (started) tr("Installer opened.", "تم فتح المثبّت.") else tr("Allow “install unknown apps” for this app, then press the button again.", "اسمح بـ «تثبيت التطبيقات المجهولة» لهذا التطبيق ثم اضغط الزر مرة أخرى.")
                    }
                }, Modifier.fillMaxWidth(), icon = Ic.IMPORT, accent = true)
            }
            GButton(tr("Check for updates", "فحص التحديثات"), {
                st.work(tr("Checking…", "جارٍ الفحص…"), { Updater.check(st.ctx) }) { r ->
                    info = r
                    status = if (r.available) null else tr("You are on the latest build.", "أنت على آخر نسخة.")
                }
            }, Modifier.fillMaxWidth(), icon = Ic.SEARCH)
        }
    }
}
