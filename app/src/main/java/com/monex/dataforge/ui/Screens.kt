package com.monex.dataforge.ui

import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.monex.dataforge.data.Dataset
import com.monex.dataforge.ops.exportProjectZip
import org.json.JSONObject
import java.io.File

// ------------------------------------------------------------------ Home / dashboard

@Composable
fun HomeScreen(st: AppState) {
    val d by ioState(JSONObject(), st.rev) { st.repo.dashboard() }
    val p = LocalPalette.current
    Page(tr("Dashboard", "لوحة التحكم")) {
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            if (!d.has("datasets")) {
                T(tr("Loading…", "جار التحميل…"), color = p.textDim)
            } else {
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    StatTile(tr("Datasets", "مجموعات البيانات"), d.getLong("datasets").toString(), Modifier.weight(1f))
                    StatTile(tr("Records", "السجلات"), d.getLong("records").toString(), Modifier.weight(1f))
                }
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    StatTile(tr("Unassigned split", "بدون تقسيم"), d.getLong("unassigned").toString(), Modifier.weight(1f))
                    StatTile(tr("Storage", "التخزين"), fmtBytes(d.getLong("storage_bytes")), Modifier.weight(1f))
                }
                Glass(Modifier.fillMaxWidth(), strength = 0.55f) {
                    Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        T(tr("Validation", "التحقق"), 15.sp, FontWeight.SemiBold)
                        if (d.getLong("validated_datasets") == 0L) {
                            T(tr("No dataset has been validated yet. Open a dataset and run validation from the Quality tab.", "لم يتم التحقق من أي مجموعة بعد. افتح مجموعة وشغّل التحقق من تبويب الجودة."), 13.sp, color = p.textDim)
                        } else {
                            T(tr("Last runs: ${d.getLong("last_errors")} errors, ${d.getLong("last_warnings")} warnings across ${d.getLong("validated_datasets")} dataset(s)",
                                "آخر عمليات التحقق: ${d.getLong("last_errors")} أخطاء، ${d.getLong("last_warnings")} تحذيرات في ${d.getLong("validated_datasets")} مجموعة"), 13.sp, color = p.textDim)
                        }
                    }
                }
                val origins = d.getJSONObject("origins")
                if (origins.length() > 0) {
                    Glass(Modifier.fillMaxWidth(), strength = 0.55f) {
                        Column(Modifier.padding(14.dp)) {
                            T(tr("Composition by origin", "التركيب حسب المصدر"), 15.sp, FontWeight.SemiBold)
                            val keys = origins.keys().asSequence().toList()
                            val max = keys.maxOf { origins.getInt(it) }
                            keys.forEach { BarRow(it, origins.getInt(it), max, p.accent) }
                        }
                    }
                }
                val recent = d.getJSONArray("recent")
                if (recent.length() > 0) {
                    Glass(Modifier.fillMaxWidth(), strength = 0.55f) {
                        Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                            T(tr("Recently modified", "آخر التعديلات"), 15.sp, FontWeight.SemiBold)
                            for (i in 0 until recent.length()) {
                                val r = recent.getJSONObject(i)
                                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                                    T(r.getString("name"), 14.sp, maxLines = 1, modifier = Modifier.weight(1f))
                                    T(fmtTime(r.getLong("updated")), 12.sp, color = p.textDim)
                                }
                            }
                        }
                    }
                }
                T(tr("Active tasks: ${if (st.busy != null) 1 else 0}", "المهام النشطة: ${if (st.busy != null) 1 else 0}"), 12.sp, color = p.textDim)
            }
            Row(Modifier.padding(bottom = 120.dp)) {}
        }
    }
}

// ------------------------------------------------------------------ Data: projects and datasets

@Composable
fun DataScreen(st: AppState) {
    val p = LocalPalette.current
    val data by ioState(Pair(emptyList<Dataset>(), emptyMap<String, Int>()), st.projectId, st.rev) {
        val list = st.repo.datasets(st.projectId)
        list to list.associate { it.id to st.repo.count(it.id) }
    }
    var showCreate by remember { mutableStateOf(false) }
    var menuDs by remember { mutableStateOf<Dataset?>(null) }
    var renameDs by remember { mutableStateOf<Dataset?>(null) }
    var deleteDs by remember { mutableStateOf<Dataset?>(null) }
    var newProject by remember { mutableStateOf(false) }
    var rename by remember { mutableStateOf(false) }
    var confirmDelete by remember { mutableStateOf(false) }
    var confirmArchive by remember { mutableStateOf(false) }
    val project = st.projects.firstOrNull { it.id == st.projectId }
    val zipSaver = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("application/zip")) { uri: Uri? ->
        if (uri != null && project != null) st.work(tr("Exporting project", "تصدير المشروع"), {
            exportProjectZip(st.ctx, uri, st.repo, project.name, data.first) { }
        }) { (d, r) -> st.toast(tr("Exported $d datasets, $r records", "تم تصدير $d مجموعات، $r سجل")) }
    }

    Page(tr("Datasets", "مجموعات البيانات"), actions = { GButton(tr("New", "جديد"), { showCreate = true }, icon = Ic.PLUS, accent = true) }) {
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            T(tr("Project", "المشروع"), 12.sp, FontWeight.SemiBold, p.textDim)
            ChipRow {
                st.projects.forEach { pr -> Chip(pr.name, pr.id == st.projectId) { st.selectProject(pr.id) } }
                Chip("+", false) { newProject = true }
            }
            ChipRow {
                GButton(tr("Rename", "إعادة تسمية"), { rename = true })
                GButton(tr("Duplicate", "نسخ"), {
                    st.work(tr("Duplicating project", "نسخ المشروع"), { st.repo.duplicateProject(st.projectId) }) { np -> st.refreshProjects(); st.selectProject(np.id) }
                })
                GButton(tr("Export ZIP", "تصدير ZIP"), { zipSaver.launch(safeFileName(project?.name ?: "project") + ".zip") }, icon = Ic.SAVE)
                GButton(tr("Archive", "أرشفة"), { confirmArchive = true })
                GButton(tr("Delete", "حذف"), { confirmDelete = true }, danger = true)
            }
            val (list, counts) = data
            if (list.isEmpty()) {
                Glass(Modifier.fillMaxWidth(), strength = 0.5f) {
                    Column(Modifier.padding(18.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        T(tr("No datasets in this project", "لا توجد مجموعات بيانات في هذا المشروع"), 16.sp, FontWeight.SemiBold)
                        T(tr("Create one from scratch, then add records by hand, by template, or by importing JSONL, JSON or CSV.", "أنشئ مجموعة جديدة ثم أضف السجلات يدويا أو بالقوالب أو باستيراد JSONL أو JSON أو CSV."), 13.sp, color = p.textDim)
                        GButton(tr("Create dataset", "إنشاء مجموعة"), { showCreate = true }, icon = Ic.PLUS, accent = true)
                    }
                }
            }
            list.forEachIndexed { idx, ds ->
                Glass(Modifier.fillMaxWidth().pressableLong(true, { st.push(Screen.DatasetS(ds.id)) }, { menuDs = ds }), strength = 0.55f, delay = minOf(idx, 8) * 40) {
                    Row(Modifier.padding(14.dp), verticalAlignment = androidx.compose.ui.Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                            T(ds.name, 16.sp, FontWeight.SemiBold, maxLines = 1)
                            Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                                Badge(kindInfo(ds.kind).let { if (st.lang == "ar") it.ar else it.en }, p.accent)
                                if (ds.language.isNotEmpty()) Badge(ds.language, p.textDim)
                                if (ds.locked) Badge(tr("locked", "مقفل"), p.warn)
                            }
                        }
                        T("${counts[ds.id] ?: 0}", 18.sp, FontWeight.Bold)
                    }
                }
            }
            Row(Modifier.padding(bottom = 120.dp)) {}
        }
    }

    if (showCreate) CreateDatasetDialog(st) { showCreate = false }
    menuDs?.let { ds ->
        Modal({ menuDs = null }) {
            T(ds.name, 18.sp, FontWeight.Bold, maxLines = 1)
            GButton(tr("Open", "فتح"), { menuDs = null; st.push(Screen.DatasetS(ds.id)) }, Modifier.fillMaxWidth())
            GButton(tr("Rename", "إعادة تسمية"), { menuDs = null; renameDs = ds }, Modifier.fillMaxWidth())
            GButton(tr("Duplicate", "نسخ"), {
                menuDs = null
                st.work(tr("Duplicating dataset", "نسخ المجموعة"), { st.repo.duplicateDataset(ds.id) }) { st.bump() }
            }, Modifier.fillMaxWidth())
            GButton(if (ds.locked) tr("Unlock", "فك القفل") else tr("Lock", "قفل"), {
                menuDs = null
                st.work(tr("Updating", "تحديث"), { st.repo.setLocked(ds.id, !ds.locked) }) { st.bump() }
            }, Modifier.fillMaxWidth())
            GButton(tr("Delete", "حذف"), { menuDs = null; deleteDs = ds }, Modifier.fillMaxWidth(), danger = true)
            GButton(tr("Close", "إغلاق"), { menuDs = null }, Modifier.fillMaxWidth())
        }
    }
    renameDs?.let { ds ->
        PromptDialog(tr("Rename dataset", "إعادة تسمية المجموعة"), ds.name, tr("Save", "حفظ"), { n ->
            st.work(tr("Renaming", "إعادة التسمية"), { st.repo.updateDatasetMeta(ds.id, n, ds.description, ds.language, ds.required) }) { st.bump() }
        }, { renameDs = null })
    }
    deleteDs?.let { ds ->
        ConfirmDialog(tr("Delete dataset \"${ds.name}\"?", "حذف المجموعة \"${ds.name}\"؟"),
            tr("Its records, versions and reports are permanently deleted.", "تحذف سجلاتها وإصداراتها وتقاريرها نهائيا."),
            tr("Delete forever", "حذف نهائي"), true, {
                st.work(tr("Deleting dataset", "حذف المجموعة"), { st.repo.deleteDataset(ds.id) }) { st.bump() }
            }, { deleteDs = null })
    }
    if (newProject) PromptDialog(tr("New project", "مشروع جديد"), "", tr("Create", "إنشاء"), { n ->
        st.work(tr("Creating project", "إنشاء المشروع"), { st.repo.createProject(n) }) { np -> st.refreshProjects(); st.selectProject(np.id) }
    }, { newProject = false })
    if (rename && project != null) PromptDialog(tr("Rename project", "إعادة تسمية المشروع"), project.name, tr("Save", "حفظ"), { n ->
        st.work(tr("Renaming", "إعادة التسمية"), { st.repo.renameProject(project.id, n) }) { st.refreshProjects() }
    }, { rename = false })
    if (confirmArchive && project != null) ConfirmDialog(tr("Archive project?", "أرشفة المشروع؟"), tr("It disappears from the list. Its data is kept in the database.", "سيختفي من القائمة وتبقى بياناته محفوظة."), tr("Archive", "أرشفة"), false, {
        st.work(tr("Archiving", "أرشفة"), { st.repo.archiveProject(project.id) }) { st.refreshProjects() }
    }, { confirmArchive = false })
    if (confirmDelete && project != null) ConfirmDialog(
        tr("Delete project \"${project.name}\"?", "حذف المشروع \"${project.name}\"؟"),
        tr("All datasets, records, versions and validation reports in it are permanently deleted. Make a backup first if unsure.", "ستحذف كل المجموعات والسجلات والإصدارات وتقارير التحقق نهائيا. أنشئ نسخة احتياطية أولا إذا لم تكن متأكدا."),
        tr("Delete forever", "حذف نهائي"), true, {
            st.work(tr("Deleting project", "حذف المشروع"), { st.repo.deleteProject(project.id) }) { st.refreshProjects() }
        }, { confirmDelete = false })
}

@Composable
fun CreateDatasetDialog(st: AppState, onDismiss: () -> Unit) {
    var name by remember { mutableStateOf("") }
    var kind by remember { mutableStateOf("conversation") }
    var lang by remember { mutableStateOf(if (st.lang == "ar") "ar" else "en") }
    var required by remember { mutableStateOf("") }
    var desc by remember { mutableStateOf("") }
    Modal(onDismiss) {
        T(tr("New dataset", "مجموعة جديدة"), 18.sp, FontWeight.Bold)
        Field(name, { name = it }, Modifier.fillMaxWidth(), hint = tr("Name", "الاسم"), singleLine = true)
        T(tr("Type (a template, not a limit)", "النوع (قالب وليس قيدا)"), 12.sp, FontWeight.SemiBold, LocalPalette.current.textDim)
        ChipRow { KINDS.forEach { k -> Chip(if (st.lang == "ar") k.ar else k.en, k.id == kind) { kind = k.id; required = k.required.joinToString(", ") } } }
        T(tr("Language", "اللغة"), 12.sp, FontWeight.SemiBold, LocalPalette.current.textDim)
        ChipRow { listOf("ar", "en", "fr", "mixed").forEach { l -> Chip(l, l == lang) { lang = l } } }
        T(tr("Required fields (comma separated, dotted paths allowed)", "الحقول المطلوبة (مفصولة بفاصلة)"), 12.sp, FontWeight.SemiBold, LocalPalette.current.textDim)
        Field(required, { required = it }, Modifier.fillMaxWidth(), hint = kindInfo(kind).required.joinToString(", "), singleLine = true, mono = true)
        Field(desc, { desc = it }, Modifier.fillMaxWidth(), hint = tr("Description / purpose", "الوصف / الهدف"), minHeight = 70.dp)
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            GButton(tr("Cancel", "إلغاء"), onDismiss, Modifier.weight(1f))
            GButton(tr("Create", "إنشاء"), {
                val req = (if (required.isBlank()) kindInfo(kind).required else required.split(",").map { it.trim() }.filter { it.isNotEmpty() })
                val n = name.ifBlank { kindInfo(kind).en }
                onDismiss()
                st.work(tr("Creating", "إنشاء"), { st.repo.createDataset(st.projectId, n, kind, lang, req, desc) }) { ds -> st.push(Screen.DatasetS(ds.id)) }
            }, Modifier.weight(1f), accent = true)
        }
    }
}

// ------------------------------------------------------------------ Backup & recovery

@Composable
fun BackupScreen(st: AppState) {
    val p = LocalPalette.current
    var pendingRestore by remember { mutableStateOf<Pair<File, JSONObject>?>(null) }
    var lastInfo by remember { mutableStateOf<String?>(null) }

    val create = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("application/octet-stream")) { uri: Uri? ->
        if (uri != null) st.work(tr("Creating backup", "إنشاء نسخة احتياطية"), {
            val out = st.ctx.contentResolver.openOutputStream(uri, "wt") ?: throw IllegalStateException("Cannot open destination")
            out.use { st.repo.backupTo(it, st.ctx.cacheDir) }
        }) { info ->
            lastInfo = tr("Backup written and verified: ${info.getInt("datasets")} datasets, ${info.getInt("records")} records, ${fmtBytes(info.getLong("bytes"))}. Integrity check: ${info.getString("integrity")}.",
                "تم إنشاء النسخة والتحقق منها: ${info.getInt("datasets")} مجموعات، ${info.getInt("records")} سجلات، ${fmtBytes(info.getLong("bytes"))}. فحص السلامة: ${info.getString("integrity")}.")
        }
    }
    val pick = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri: Uri? ->
        if (uri != null) st.work(tr("Verifying backup", "التحقق من النسخة"), {
            val tmp = File(st.ctx.cacheDir, "restore-preview.db")
            st.ctx.contentResolver.openInputStream(uri)!!.use { i -> tmp.outputStream().use { o -> i.copyTo(o) } }
            try { tmp to st.repo.verifyBackup(tmp) } catch (e: Exception) { tmp.delete(); throw e }
        }) { pendingRestore = it }
    }

    Page(tr("Backup & recovery", "النسخ الاحتياطي والاستعادة")) {
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Glass(Modifier.fillMaxWidth(), strength = 0.55f) {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    T(tr("Full workspace backup", "نسخة احتياطية كاملة"), 16.sp, FontWeight.SemiBold)
                    T(tr("Writes one consistent SQLite file containing every project, dataset, record, version and report. The file is integrity-checked before it is accepted, and you choose where it is saved (including a cloud folder through the system picker).",
                        "ينشئ ملف SQLite واحدا متسقا يحوي كل المشاريع والمجموعات والسجلات والإصدارات والتقارير. يتم فحص سلامته قبل اعتماده وتختار أنت مكان الحفظ."), 13.sp, color = p.textDim)
                    GButton(tr("Create backup", "إنشاء نسخة"), { create.launch("dataforge-backup-${System.currentTimeMillis() / 1000}.db") }, icon = Ic.BACKUP, accent = true)
                    lastInfo?.let { T(it, 13.sp, color = p.ok) }
                }
            }
            Glass(Modifier.fillMaxWidth(), strength = 0.55f) {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    T(tr("Restore", "استعادة"), 16.sp, FontWeight.SemiBold)
                    T(tr("The file is verified and previewed first. Restoring replaces the whole workspace; the current database is kept next to it as a \".pre-restore\" safety copy.",
                        "يتم التحقق من الملف ومعاينته أولا. الاستعادة تستبدل كل مساحة العمل، وتحفظ النسخة الحالية كنسخة أمان بامتداد .pre-restore."), 13.sp, color = p.textDim)
                    GButton(tr("Choose backup file", "اختيار ملف النسخة"), { pick.launch(arrayOf("*/*")) }, icon = Ic.IMPORT)
                }
            }
            Row(Modifier.padding(bottom = 120.dp)) {}
        }
    }

    pendingRestore?.let { (file, info) ->
        ConfirmDialog(
            tr("Restore this backup?", "استعادة هذه النسخة؟"),
            tr("Integrity: ${info.getString("integrity")}. Contains ${info.getInt("datasets")} datasets, ${info.getInt("records")} records, ${info.getInt("versions")} versions. Your current workspace will be replaced.",
                "السلامة: ${info.getString("integrity")}. تحتوي ${info.getInt("datasets")} مجموعات و${info.getInt("records")} سجلات و${info.getInt("versions")} إصدارات. سيتم استبدال مساحة العمل الحالية."),
            tr("Restore", "استعادة"), true, {
                st.work(tr("Restoring", "الاستعادة"), { file.inputStream().use { st.repo.restoreFrom(it, st.ctx.cacheDir) } }) {
                    st.stack = emptyList(); st.refreshProjects(); st.toast(tr("Workspace restored", "تمت استعادة مساحة العمل"))
                }
                pendingRestore = null
            }, { file.delete(); pendingRestore = null }
        )
    }
}

// ------------------------------------------------------------------ Settings

@Composable
fun SettingsScreen(st: AppState) {
    val p = LocalPalette.current
    Page(tr("Settings", "الإعدادات")) {
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Glass(Modifier.fillMaxWidth(), strength = 0.55f) {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    T(tr("Appearance", "المظهر"), 16.sp, FontWeight.SemiBold)
                    ChipRow {
                        Chip(tr("System", "النظام"), st.themeMode == "system") { st.setTheme("system") }
                        Chip(tr("Light", "فاتح"), st.themeMode == "light") { st.setTheme("light") }
                        Chip(tr("Dark", "داكن"), st.themeMode == "dark") { st.setTheme("dark") }
                    }
                    ChipRow {
                        Chip("English", st.lang == "en") { st.setLang("en") }
                        Chip("العربية", st.lang == "ar") { st.setLang("ar") }
                    }
                    Toggle(st.reduceMotion, { st.setMotionOff(it) }, tr("Reduce motion", "تقليل الحركة"))
                }
            }
            UpdateCard(st)
            Glass(Modifier.fillMaxWidth(), strength = 0.55f) {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    T(tr("Logs", "السجلات"), 16.sp, FontWeight.SemiBold)
                    T(tr("Crash reports and operation logs are kept on this device. Open them to copy, save or send to GitHub.", "تقارير الأعطال وسجل العمليات محفوظة على الجهاز. افتحها للنسخ أو الحفظ أو الإرسال إلى GitHub."), 13.sp, color = p.textDim)
                    GButton(tr("Open logs", "فتح السجلات"), { st.push(Screen.LogsS) }, Modifier.fillMaxWidth(), icon = Ic.CODE)
                }
            }
            Glass(Modifier.fillMaxWidth(), strength = 0.55f) {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    T(tr("Privacy", "الخصوصية"), 16.sp, FontWeight.SemiBold)
                    T(tr("The internet permission is used only when you press “Check for updates” or “Download and install”. Files are opened and saved only through the system file picker. No telemetry, no accounts, nothing is uploaded.",
                        "صلاحية الإنترنت تُستعمل فقط عند الضغط على «فحص التحديثات» أو «تنزيل وتثبيت». تفتح الملفات وتحفظ عبر منتقي الملفات فقط. لا تتبع ولا حسابات ولا رفع."), 13.sp, color = p.textDim)
                }
            }
            Glass(Modifier.fillMaxWidth(), strength = 0.55f) {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    T(tr("AI-assisted generation", "التوليد بمساعدة الذكاء الاصطناعي"), 16.sp, FontWeight.SemiBold)
                    T(tr("Not implemented in this version. The app is fully usable without it, and no external provider is contacted.", "غير مُنفّذ في هذا الإصدار. التطبيق يعمل كاملا بدونه ولا يتصل بأي مزود خارجي."), 13.sp, color = p.textDim)
                }
            }
            T(com.monex.dataforge.core.Logger.headerLine(), 11.sp, color = p.textDim)
            Row(Modifier.padding(bottom = 120.dp)) {}
        }
    }
}

private fun safeFileName(s: String): String = s.replace(Regex("[^\\p{L}\\p{N}._-]+"), "_").trim('_').ifEmpty { "project" }
