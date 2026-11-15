package com.monex.dataforge.ui

import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.monex.dataforge.core.NativeCore
import com.monex.dataforge.data.Dataset
import com.monex.dataforge.data.NewRec
import com.monex.dataforge.data.Rec
import com.monex.dataforge.ops.ExportOptions
import com.monex.dataforge.ops.SplitPlan
import com.monex.dataforge.ops.ValidateOpts
import com.monex.dataforge.ops.exportDataset
import com.monex.dataforge.ops.expandTemplate
import com.monex.dataforge.ops.importFile
import com.monex.dataforge.ops.leakageCheck
import com.monex.dataforge.ops.planSplit
import com.monex.dataforge.ops.validateDataset
import org.json.JSONArray
import androidx.compose.animation.togetherWith
import org.json.JSONObject

private val TABS = listOf(
    "Records" to "السجلات", "Quality" to "الجودة", "Splits" to "التقسيم", "Versions" to "الإصدارات",
    "Import/Export" to "استيراد/تصدير", "Batch" to "دفعات", "Info" to "معلومات", "Find" to "بحث", "SQL" to "SQL"
)

/** Short list preview; computed by the Rust engine (falls back to raw text if the engine rejects it). */
fun previewOf(data: String): String =
    try { com.monex.dataforge.core.NativeCore.preview(data) } catch (e: Exception) { data.take(140) }

@Composable
fun DatasetScreen(st: AppState, id: String) {
    val ds by ioState<Dataset?>(null, id, st.rev) { st.repo.dataset(id) }
    var tab by remember { mutableIntStateOf(0) }
    var confirmUnlock by remember { mutableStateOf(false) }
    val d = ds
    if (d == null) { Page("…", onBack = { st.pop() }) {}; return }
    val p = LocalPalette.current
    Page(d.name, onBack = { st.pop() }, actions = {
        IconBtn(Ic.LOCK, { if (d.locked) { confirmUnlock = true } else { st.work("…", { st.repo.setLocked(d.id, true) }) } }, tint = if (d.locked) p.warn else null)
    }) {
        Column(Modifier.padding(horizontal = 16.dp)) {
            ChipRow { TABS.forEachIndexed { i, t -> Chip(if (st.lang == "ar") t.second else t.first, tab == i) { tab = i } } }
        }
        Column(Modifier.weight(1f).padding(horizontal = 16.dp)) {
            androidx.compose.animation.AnimatedContent(
                targetState = tab,
                transitionSpec = {
                    val dir = if (targetState > initialState) 1 else -1
                    (androidx.compose.animation.slideInHorizontally(androidx.compose.animation.core.tween(260)) { w -> dir * w / 6 } + androidx.compose.animation.fadeIn(androidx.compose.animation.core.tween(220))) togetherWith
                        (androidx.compose.animation.slideOutHorizontally(androidx.compose.animation.core.tween(200)) { w -> -dir * w / 8 } + androidx.compose.animation.fadeOut(androidx.compose.animation.core.tween(120)))
                },
                label = "tabs"
            ) { tabNow ->
            when (tabNow) {
                0 -> RecordsTab(st, d)
                1 -> QualityTab(st, d)
                2 -> SplitsTab(st, d)
                3 -> VersionsTab(st, d)
                4 -> TransferTab(st, d)
                5 -> BatchTab(st, d)
                6 -> InfoTab(st, d)
                7 -> FindTab(st, d)
                else -> SqlTab(st, d)
            }
            }
        }
    }
    if (confirmUnlock) ConfirmDialog(
        tr("Unlock dataset?", "فتح قفل المجموعة؟"),
        tr("While unlocked, records and split assignments can change. The unlock is recorded in the dataset log.", "عند فتح القفل يمكن تعديل السجلات والتقسيم. يسجل ذلك في سجل المجموعة."),
        tr("Unlock", "فتح القفل"), false, { st.work("…", { st.repo.setLocked(d.id, false) }) }, { confirmUnlock = false })
}

// ------------------------------------------------------------------ records

@Composable
fun RecordsTab(st: AppState, ds: Dataset) {
    val p = LocalPalette.current
    var query by remember { mutableStateOf("") }
    var split by remember { mutableStateOf<String?>(null) }
    var limit by remember { mutableIntStateOf(50) }
    var selected by remember { mutableStateOf(setOf<String>()) }
    var confirmDelete by remember { mutableStateOf(false) }
    LaunchedEffect(query, split) { limit = 50 }
    val total by ioState(0, ds.id, query, split, st.rev) { st.repo.count(ds.id, query, split) }
    val rows by ioState(emptyList<Rec>(), ds.id, query, split, limit, st.rev) { st.repo.page(ds.id, query, split, limit, 0) }

    Column(Modifier.fillMaxSize(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
            Field(query, { query = it }, Modifier.weight(1f), hint = tr("Search (Arabic-aware)", "بحث (يدعم العربية)"), singleLine = true)
            GButton("", { st.push(Screen.WriteS(ds.id)) }, icon = Ic.WRITE, enabled = !ds.locked)
            GButton("", { st.push(Screen.EditorS(ds.id, null)) }, icon = Ic.PLUS, accent = true, enabled = !ds.locked)
        }
        ChipRow {
            Chip(tr("All", "الكل"), split == null) { split = null }
            SPLITS.forEach { s -> Chip(s, split == s) { split = s } }
        }
        T(tr("$total records", "$total سجل"), 12.sp, color = p.textDim)
        if (selected.isNotEmpty()) {
            Glass(Modifier.fillMaxWidth(), strength = 0.7f, radius = 16.dp) {
                Column(Modifier.padding(10.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    T(tr("${selected.size} selected", "${selected.size} محدد"), 13.sp, FontWeight.SemiBold)
                    ChipRow {
                        SPLITS.forEach { s ->
                            Chip("→ $s", false) {
                                val ids = selected
                                st.work(tr("Assigning split", "تعيين التقسيم"), { st.repo.setSplits(ds.id, ids.associateWith { s }) }) { selected = emptySet() }
                            }
                        }
                        GButton(tr("Delete", "حذف"), { confirmDelete = true }, danger = true, enabled = !ds.locked)
                        GButton(tr("Clear", "مسح"), { selected = emptySet() })
                    }
                }
            }
        }
        if (rows.isEmpty() && total == 0) {
            T(if (query.isBlank() && split == null) tr("This dataset is empty. Add a record, import a file, or generate from a template.", "المجموعة فارغة. أضف سجلا أو استورد ملفا أو أنشئ من قالب.") else tr("No records match.", "لا توجد سجلات مطابقة."), 14.sp, color = p.textDim)
        }
        LazyColumn(Modifier.weight(1f, fill = false), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            items(rows, key = { it.id }) { r ->
                val sel = r.id in selected
                Glass(Modifier.fillMaxWidth().animateItem().pressable { st.push(Screen.EditorS(ds.id, r.id)) }, strength = 0.55f, radius = 16.dp) {
                    Row(Modifier.padding(12.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                        Box(
                            Modifier.size(24.dp).pressable { selected = if (sel) selected - r.id else selected + r.id }
                                .clip(RoundedCornerShape(7.dp)).background(if (sel) p.accent else p.field)
                                .border(1.dp, p.glassEdge.copy(alpha = 0.4f), RoundedCornerShape(7.dp)),
                            contentAlignment = Alignment.Center
                        ) { if (sel) Icon(Ic.CHECK, androidx.compose.ui.graphics.Color.White, 16.dp) }
                        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                            T(remember(r.id, r.data) { previewOf(r.data) }.ifBlank { "(empty)" }, 14.sp, maxLines = 2)
                            Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                                Badge(r.origin, if (r.origin == "ai_generated") p.warn else p.accent)
                                Badge(r.split, p.textDim)
                                r.label?.let { Badge(it, p.ok) }
                            }
                        }
                    }
                }
            }
            if (rows.size < total) item {
                GButton(tr("Load more (${rows.size}/$total)", "تحميل المزيد (${rows.size}/$total)"), { limit += 50 }, Modifier.fillMaxWidth())
            }
            item { Box(Modifier.padding(bottom = 120.dp)) {} }
        }
    }
    if (confirmDelete) ConfirmDialog(
        tr("Delete ${selected.size} records?", "حذف ${selected.size} سجل؟"),
        tr("Their last content is kept in the history store, but they leave the dataset.", "يحتفظ بآخر محتوى لها في سجل التاريخ لكنها تخرج من المجموعة."),
        tr("Delete", "حذف"), true, {
            val ids = selected.toList()
            st.work(tr("Deleting", "حذف"), { st.repo.deleteRecords(ids) }) { selected = emptySet() }
        }, { confirmDelete = false })
}

// ------------------------------------------------------------------ quality

@Composable
fun QualityTab(st: AppState, ds: Dataset) {
    val p = LocalPalette.current
    var minC by remember { mutableStateOf("") }
    var maxC by remember { mutableStateOf("") }
    var thr by remember { mutableStateOf("0.85") }
    var stats by remember { mutableStateOf<JSONObject?>(null) }
    val last by ioState<JSONObject?>(null, ds.id, st.rev) { st.repo.lastValidation(ds.id) }
    val groups by ioState(Triple(emptyMap<String, Int>(), emptyMap<String, Int>(), emptyMap<String, Int>()), ds.id, st.rev) {
        Triple(st.repo.groupCounts(ds.id, "split"), st.repo.groupCounts(ds.id, "origin"), st.repo.groupCounts(ds.id, "label"))
    }
    Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Glass(Modifier.fillMaxWidth(), strength = 0.55f) {
            Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                T(tr("Validation", "التحقق"), 16.sp, FontWeight.SemiBold)
                T(tr("Deterministic rules are exact. Heuristic rules (near duplicates, abnormal length, repetition, balance) are hints, not proof of semantic problems. Nothing is modified.",
                    "القواعد الحتمية دقيقة. القواعد الاستدلالية (تشابه، طول غير معتاد، تكرار، توازن) مجرد مؤشرات وليست إثباتا لمشكلة دلالية. لا يتم تعديل أي سجل."), 12.sp, color = p.textDim)
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Field(minC, { minC = it }, Modifier.weight(1f), hint = tr("min chars", "أقل طول"), singleLine = true, keyboard = KeyboardOptions(keyboardType = KeyboardType.Number))
                    Field(maxC, { maxC = it }, Modifier.weight(1f), hint = tr("max chars", "أقصى طول"), singleLine = true, keyboard = KeyboardOptions(keyboardType = KeyboardType.Number))
                    Field(thr, { thr = it }, Modifier.weight(1f), hint = tr("near dup", "تشابه"), singleLine = true, keyboard = KeyboardOptions(keyboardType = KeyboardType.Decimal))
                }
                GButton(tr("Run validation", "تشغيل التحقق"), {
                    val o = ValidateOpts(minC.toIntOrNull() ?: 0, maxC.toIntOrNull() ?: 0, thr.toDoubleOrNull() ?: 0.85)
                    st.work(tr("Validating", "جار التحقق"), {
                        val rep = validateDataset(st.repo, ds, o, st::progress) { st.cancelFlag.get() }
                        if (!rep.optBoolean("cancelled")) st.repo.saveValidation(ds.id, rep)
                        rep
                    }) { rep -> if (rep.optBoolean("cancelled")) st.toast(tr("Cancelled, report not saved", "أُلغي، لم يُحفظ التقرير")) }
                }, icon = Ic.CHECK, accent = true)
            }
        }
        last?.let { rep ->
            Glass(Modifier.fillMaxWidth(), strength = 0.6f) {
                Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    T(tr("Last report", "آخر تقرير"), 16.sp, FontWeight.SemiBold)
                    T(rep.getString("validation_id"), 12.sp, color = p.textDim, mono = true)
                    Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        Badge(tr("${rep.getInt("errors")} errors", "${rep.getInt("errors")} أخطاء"), if (rep.getInt("errors") > 0) p.danger else p.ok)
                        Badge(tr("${rep.getInt("warnings")} warnings", "${rep.getInt("warnings")} تحذيرات"), if (rep.getInt("warnings") > 0) p.warn else p.ok)
                    }
                    T(tr("${rep.getInt("records_examined")} records examined · ${rep.getInt("rules_passed")} rules passed · ${rep.getInt("rules_failed")} failed · ${rep.getLong("duration_ms")} ms",
                        "${rep.getInt("records_examined")} سجل · ${rep.getInt("rules_passed")} قواعد نجحت · ${rep.getInt("rules_failed")} فشلت · ${rep.getLong("duration_ms")} مللي ثانية"), 12.sp, color = p.textDim)
                    if (rep.optInt("dedup_scope") >= rep.optInt("dedup_limit", Int.MAX_VALUE)) {
                        T(tr("Duplicate detection covered only the first ${rep.getInt("dedup_limit")} records.", "كشف التكرار شمل أول ${rep.getInt("dedup_limit")} سجل فقط."), 12.sp, color = p.warn)
                    }
                    val rules = rep.getJSONArray("rules")
                    for (i in 0 until rules.length()) {
                        val r = rules.getJSONObject(i)
                        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                            Badge(r.getString("id"), p.textDim)
                            Badge(if (r.getBoolean("deterministic")) tr("exact", "حتمي") else tr("heuristic", "استدلالي"), if (r.getBoolean("deterministic")) p.accent else p.warn)
                            T(r.getString("description"), 12.sp, modifier = Modifier.weight(1f), maxLines = 1)
                            T(r.getInt("findings").toString(), 12.sp, FontWeight.Bold, if (r.getInt("findings") > 0) p.danger else p.ok)
                        }
                    }
                    if (rep.getBoolean("truncated")) T(tr("Findings list truncated; counts above are complete.", "قائمة النتائج مقتطعة؛ الأعداد أعلاه كاملة."), 12.sp, color = p.warn)
                }
            }
            val fs = rep.getJSONArray("findings")
            T(tr("Findings (first ${minOf(100, fs.length())} of ${fs.length()} listed)", "النتائج (أول ${minOf(100, fs.length())} من ${fs.length()})"), 13.sp, FontWeight.SemiBold, p.textDim)
            for (i in 0 until minOf(100, fs.length())) {
                val f = fs.getJSONObject(i)
                val rid = f.getString("record")
                Glass(Modifier.fillMaxWidth().then(if (rid != "*") Modifier.pressable { st.push(Screen.EditorS(ds.id, rid)) } else Modifier), strength = 0.5f, radius = 14.dp) {
                    Column(Modifier.padding(10.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            Badge(f.getString("rule"), p.textDim)
                            Badge(f.getString("severity"), if (f.getString("severity") == "error") p.danger else p.warn)
                            if (!f.getBoolean("deterministic")) Badge(tr("heuristic", "استدلالي"), p.warn)
                        }
                        T(f.getString("message"), 13.sp)
                    }
                }
            }
        } ?: T(tr("No validation report yet.", "لا يوجد تقرير تحقق بعد."), 13.sp, color = p.textDim)

        Glass(Modifier.fillMaxWidth(), strength = 0.55f) {
            Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                T(tr("Composition", "التركيب"), 16.sp, FontWeight.SemiBold)
                val (bySplit, byOrigin, byLabel) = groups
                listOf(tr("By split", "حسب التقسيم") to bySplit, tr("By origin", "حسب المصدر") to byOrigin, tr("By label", "حسب الوسم") to byLabel).forEach { (title, m) ->
                    if (m.isNotEmpty()) {
                        T(title, 12.sp, FontWeight.SemiBold, p.textDim)
                        val mx = m.values.max()
                        m.forEach { (k, v) -> BarRow(k, v, mx, p.accent) }
                    }
                }
                GButton(tr("Analyze text (length, script)", "تحليل النص (الطول والخط)"), {
                    st.work(tr("Analyzing", "تحليل"), {
                        val texts = st.repo.texts(ds.id, 20000, 4000).map { it.second }
                        NativeCore.run("stats", JSONObject().put("texts", JSONArray(texts))).put("sampled", texts.size)
                    }) { stats = it }
                })
                stats?.let { s ->
                    T(tr("Based on ${s.getInt("sampled")} records (max 20,000 analyzed).", "اعتمادا على ${s.getInt("sampled")} سجل (حد أقصى 20,000)."), 12.sp, color = p.textDim)
                    T(tr("Average ${"%.1f".format(s.getDouble("avg_chars"))} chars · median ${s.getInt("p50_chars")} · p95 ${s.getInt("p95_chars")} · max ${s.getInt("max_chars")}",
                        "المتوسط ${"%.1f".format(s.getDouble("avg_chars"))} حرف · الوسيط ${s.getInt("p50_chars")} · p95 ${s.getInt("p95_chars")} · الأقصى ${s.getInt("max_chars")}"), 13.sp)
                    val sc = s.getJSONObject("scripts")
                    val keys = sc.keys().asSequence().toList()
                    if (keys.isNotEmpty()) {
                        T(tr("Script (not language detection)", "نوع الكتابة (وليس تحديد اللغة)"), 12.sp, FontWeight.SemiBold, p.textDim)
                        val mx = keys.maxOf { sc.getInt(it) }
                        keys.forEach { BarRow(it, sc.getInt(it), mx, p.accent2) }
                    }
                    val h = s.getJSONArray("length_histogram")
                    T(tr("Length distribution", "توزيع الأطوال"), 12.sp, FontWeight.SemiBold, p.textDim)
                    val mx = (0 until h.length()).maxOf { h.getJSONObject(it).getInt("count") }
                    for (i in 0 until h.length()) h.getJSONObject(i).let { BarRow(it.getString("bucket"), it.getInt("count"), mx, p.ok) }
                }
            }
        }
        Box(Modifier.padding(bottom = 120.dp)) {}
    }
}

// ------------------------------------------------------------------ splits

@Composable
fun SplitsTab(st: AppState, ds: Dataset) {
    val p = LocalPalette.current
    var rTrain by remember { mutableStateOf("0.8") }
    var rVal by remember { mutableStateOf("0.1") }
    var rTest by remember { mutableStateOf("0.1") }
    var seed by remember { mutableStateOf("42") }
    var strat by remember { mutableStateOf(true) }
    var groups by remember { mutableStateOf(true) }
    var dup by remember { mutableStateOf(true) }
    var plan by remember { mutableStateOf<SplitPlan?>(null) }
    var leak by remember { mutableStateOf<JSONObject?>(null) }
    var confirmApply by remember { mutableStateOf(false) }
    val cur by ioState(emptyMap<String, Int>(), ds.id, st.rev) { st.repo.groupCounts(ds.id, "split") }
    val num = KeyboardOptions(keyboardType = KeyboardType.Decimal)

    Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Glass(Modifier.fillMaxWidth(), strength = 0.55f) {
            Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                T(tr("Current distribution", "التوزيع الحالي"), 16.sp, FontWeight.SemiBold)
                if (cur.isEmpty()) T(tr("No records.", "لا توجد سجلات."), 13.sp, color = p.textDim) else {
                    val mx = cur.values.max(); cur.forEach { (k, v) -> BarRow(k, v, mx, p.accent) }
                }
            }
        }
        Glass(Modifier.fillMaxWidth(), strength = 0.55f) {
            Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                T(tr("Reproducible split", "تقسيم قابل للإعادة"), 16.sp, FontWeight.SemiBold)
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Field(rTrain, { rTrain = it }, Modifier.weight(1f), hint = "train", singleLine = true, keyboard = num)
                    Field(rVal, { rVal = it }, Modifier.weight(1f), hint = "validation", singleLine = true, keyboard = num)
                    Field(rTest, { rTest = it }, Modifier.weight(1f), hint = "test", singleLine = true, keyboard = num)
                }
                Field(seed, { seed = it }, Modifier.fillMaxWidth(), hint = "seed", singleLine = true, keyboard = KeyboardOptions(keyboardType = KeyboardType.Number))
                Toggle(strat, { strat = it }, tr("Stratify by label", "توزيع طبقي حسب الوسم"))
                Toggle(groups, { groups = it }, tr("Keep records of the same group together", "إبقاء سجلات نفس المجموعة معا"))
                Toggle(dup, { dup = it }, tr("Keep duplicates together (prevents leakage)", "إبقاء المكررات معا (يمنع التسرب)"))
                GButton(tr("Preview split", "معاينة التقسيم"), {
                    val ratios = LinkedHashMap<String, Double>()
                    ratios["train"] = rTrain.toDoubleOrNull() ?: 0.0; ratios["validation"] = rVal.toDoubleOrNull() ?: 0.0; ratios["test"] = rTest.toDoubleOrNull() ?: 0.0
                    val sd = seed.toLongOrNull() ?: 42L
                    st.work(tr("Planning split", "تخطيط التقسيم"), { planSplit(st.repo, ds, ratios, sd, strat, groups, dup, 0.85) }) { plan = it }
                }, icon = Ic.SPLIT)
                plan?.let { pl ->
                    T(tr("Planned: ", "المخطط: ") + pl.counts.keys().asSequence().joinToString(" · ") { "$it ${pl.counts.getInt(it)}" }, 13.sp, FontWeight.SemiBold)
                    T(tr("${pl.units} independent units after grouping.", "${pl.units} وحدة مستقلة بعد التجميع."), 12.sp, color = p.textDim)
                    if (pl.evalMoves > 0) T(tr("Warning: ${pl.evalMoves} records currently in validation/test would move. Test records are never moved without your explicit confirmation.",
                        "تحذير: ${pl.evalMoves} سجل حاليا في validation/test سينتقل. لا تنقل سجلات الاختبار دون تأكيدك الصريح."), 12.sp, color = p.warn)
                    GButton(tr("Apply split", "تطبيق التقسيم"), { if (pl.evalMoves > 0) { confirmApply = true } else { st.work(tr("Applying split", "تطبيق"), { st.repo.setSplits(ds.id, pl.assign) }) { plan = null } } }, accent = true, enabled = !ds.locked)
                    if (ds.locked) T(tr("Dataset is locked; unlock it to apply.", "المجموعة مقفلة؛ افتح القفل للتطبيق."), 12.sp, color = p.warn)
                }
            }
        }
        Glass(Modifier.fillMaxWidth(), strength = 0.55f) {
            Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                T(tr("Leakage check", "فحص التسرب"), 16.sp, FontWeight.SemiBold)
                T(tr("Exact duplicates across train and evaluation splits are certain. Near duplicates and shared sources are heuristic hints.", "التكرار الحرفي بين التدريب والتقييم مؤكد. التشابه التقريبي والمصادر المشتركة مجرد مؤشرات."), 12.sp, color = p.textDim)
                GButton(tr("Check leakage", "فحص التسرب"), { st.work(tr("Checking leakage", "فحص التسرب"), { leakageCheck(st.repo, ds, 0.85) }) { leak = it } })
                leak?.let { l ->
                    val n = l.getInt("potential_leaks")
                    T(tr("$n potential leaks · ${l.getInt("shared_sources")} sources shared across splits", "$n تسرب محتمل · ${l.getInt("shared_sources")} مصدر مشترك بين الأقسام"), 14.sp, FontWeight.SemiBold, if (n > 0) p.danger else p.ok)
                    val f = l.getJSONArray("findings")
                    for (i in 0 until minOf(50, f.length())) {
                        val x = f.getJSONObject(i)
                        Row(Modifier.fillMaxWidth().pressable { st.push(Screen.EditorS(ds.id, x.getString("b"))) }, horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                            Badge(x.getString("kind"), if (x.getBoolean("deterministic")) p.danger else p.warn)
                            T("${x.getString("a_split")} ↔ ${x.getString("b_split")} (${"%.2f".format(x.getDouble("similarity"))})", 12.sp)
                        }
                    }
                }
            }
        }
        Box(Modifier.padding(bottom = 120.dp)) {}
    }
    if (confirmApply) plan?.let { pl ->
        ConfirmDialog(tr("Move evaluation records?", "نقل سجلات التقييم؟"),
            tr("${pl.evalMoves} records now in validation/test will be reassigned. Continue only if you intend this.", "سيعاد تعيين ${pl.evalMoves} سجل من validation/test. تابع فقط إذا كنت تقصد ذلك."),
            tr("Move them", "انقلها"), true, { st.work(tr("Applying split", "تطبيق"), { st.repo.setSplits(ds.id, pl.assign) }) { plan = null } }, { confirmApply = false })
    }
}

// ------------------------------------------------------------------ versions

@Composable
fun VersionsTab(st: AppState, ds: Dataset) {
    val p = LocalPalette.current
    var label by remember { mutableStateOf("") }
    var sel by remember { mutableStateOf(listOf<String>()) }
    var diff by remember { mutableStateOf<JSONObject?>(null) }
    var restoreId by remember { mutableStateOf<String?>(null) }
    val versions by ioState(emptyList<com.monex.dataforge.data.VersionInfo>(), ds.id, st.rev) { st.repo.versions(ds.id) }
    Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Glass(Modifier.fillMaxWidth(), strength = 0.55f) {
            Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                T(tr("Snapshots", "اللقطات"), 16.sp, FontWeight.SemiBold)
                T(tr("A snapshot stores every record's content by hash (identical content is stored once) together with split, origin, source and license, plus the schema settings.", "اللقطة تخزن محتوى كل سجل بالتجزئة (المحتوى المتطابق يخزن مرة واحدة) مع التقسيم والمصدر والترخيص وإعدادات المخطط."), 12.sp, color = p.textDim)
                Field(label, { label = it }, Modifier.fillMaxWidth(), hint = tr("Label (e.g. before cleaning)", "وصف (مثال: قبل التنظيف)"), singleLine = true)
                GButton(tr("Create snapshot", "إنشاء لقطة"), { val l = label.ifBlank { "snapshot" }; st.work(tr("Snapshotting", "إنشاء لقطة"), { st.repo.snapshot(ds.id, l) }) { label = "" } }, icon = Ic.CLOCK, accent = true)
            }
        }
        if (versions.size >= 2) T(tr("Select two versions to compare", "اختر إصدارين للمقارنة"), 12.sp, color = p.textDim)
        versions.forEach { v ->
            val on = v.id in sel
            Glass(Modifier.fillMaxWidth().pressable { sel = if (on) sel - v.id else (sel + v.id).takeLast(2); diff = null }, strength = if (on) 0.85f else 0.5f, radius = 16.dp) {
                Row(Modifier.padding(12.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Column(Modifier.weight(1f)) {
                        T("v${v.number} · ${v.label}", 14.sp, FontWeight.SemiBold, maxLines = 1)
                        T("${fmtTime(v.created)} · ${v.count}", 12.sp, color = p.textDim)
                    }
                    if (on) Badge("✓", p.accent)
                    GButton(tr("Restore", "استعادة"), { restoreId = v.id }, enabled = !ds.locked)
                }
            }
        }
        if (sel.size == 2) {
            GButton(tr("Compare selected", "قارن المحدد"), {
                val vs = versions.filter { it.id in sel }.sortedBy { it.number }
                st.work(tr("Comparing", "مقارنة"), { st.repo.diff(vs[0].id, vs[1].id) }) { diff = it }
            }, accent = true)
        }
        diff?.let { d ->
            Glass(Modifier.fillMaxWidth(), strength = 0.6f) {
                Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    T(tr("Older → newer", "الأقدم ← الأحدث"), 14.sp, FontWeight.SemiBold)
                    BarRow(tr("added", "مضاف"), d.getInt("added"), maxOf(1, d.getInt("added"), d.getInt("removed"), d.getInt("modified")), p.ok)
                    BarRow(tr("removed", "محذوف"), d.getInt("removed"), maxOf(1, d.getInt("added"), d.getInt("removed"), d.getInt("modified")), p.danger)
                    BarRow(tr("modified", "معدل"), d.getInt("modified"), maxOf(1, d.getInt("added"), d.getInt("removed"), d.getInt("modified")), p.warn)
                    T(tr("split changed: ${d.getInt("split_changed")}", "تغيير التقسيم: ${d.getInt("split_changed")}"), 13.sp, color = p.textDim)
                }
            }
        }
        if (versions.isEmpty()) T(tr("No snapshots yet.", "لا توجد لقطات بعد."), 13.sp, color = p.textDim)
        Box(Modifier.padding(bottom = 120.dp)) {}
    }
    restoreId?.let { rid ->
        ConfirmDialog(tr("Restore this version?", "استعادة هذا الإصدار؟"),
            tr("The current state is snapshotted automatically first, then replaced by this version.", "تؤخذ لقطة تلقائية للحالة الحالية أولا ثم تستبدل بهذا الإصدار."),
            tr("Restore", "استعادة"), true, { st.work(tr("Restoring", "استعادة"), { st.repo.restore(ds.id, rid) }) }, { restoreId = null })
    }
}

// ------------------------------------------------------------------ import / export

@Composable
fun TransferTab(st: AppState, ds: Dataset) {
    val p = LocalPalette.current
    var license by remember { mutableStateOf("unknown") }
    var container by remember { mutableStateOf("jsonl") }
    var profile by remember { mutableStateOf(if (isConversationKind(ds.kind)) "messages" else "raw") }
    var split by remember { mutableStateOf<String?>(null) }
    var meta by remember { mutableStateOf(false) }
    var roles by remember { mutableStateOf("") }
    var report by remember { mutableStateOf<String?>(null) }
    val last by ioState<JSONObject?>(null, ds.id, st.rev) { st.repo.lastValidation(ds.id) }

    val importPick = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri: Uri? ->
        if (uri != null) st.work(tr("Importing", "استيراد"), {
            importFile(st.ctx, uri, st.repo, ds, license.ifBlank { "unknown" }, st::progress) { st.cancelFlag.get() }
        }) { r ->
            report = tr("Imported ${r.imported}, rejected ${r.rejected}${if (r.cancelled) " (cancelled)" else ""}.", "تم استيراد ${r.imported}، ورُفض ${r.rejected}${if (r.cancelled) " (ألغي)" else ""}.") +
                (if (r.errors.isNotEmpty()) "\n" + r.errors.joinToString("\n") else "")
        }
    }
    val exportPick = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("application/octet-stream")) { uri: Uri? ->
        if (uri != null) {
            val rm = roles.split(",").mapNotNull { it.split("=").takeIf { kv -> kv.size == 2 }?.let { kv -> kv[0].trim() to kv[1].trim() } }.toMap()
            val opts = ExportOptions(container, profile, split, meta, rm)
            st.work(tr("Exporting", "تصدير"), { exportDataset(st.ctx, uri, st.repo, ds, opts, st::progress) { st.cancelFlag.get() } }) { r ->
                report = tr("Exported ${r.written} records. Integrity: ${r.note}. sha256 ${r.sha256.take(16)}…", "تم تصدير ${r.written} سجل. السلامة: ${r.note}. sha256 ${r.sha256.take(16)}…") +
                    (if (r.errors.isNotEmpty()) "\n" + tr("Skipped:", "تم تخطي:") + "\n" + r.errors.joinToString("\n") else "")
            }
        }
    }
    Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Glass(Modifier.fillMaxWidth(), strength = 0.55f) {
            Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                T(tr("Import", "استيراد"), 16.sp, FontWeight.SemiBold)
                T(tr("JSONL is streamed in batches of 500. JSON, CSV, TSV and TXT are loaded whole (max 64 MB). ShareGPT, Alpaca and prompt/completion records are converted to messages and marked \"transformed\". The original file is never modified.",
                    "JSONL يستورد تدفقيا بدفعات 500. أما JSON وCSV وTSV وTXT فتقرأ كاملة (حتى 64 ميغابايت). تحوّل سجلات ShareGPT وAlpaca وprompt/completion إلى رسائل وتوسم \"transformed\". لا يُعدّل الملف الأصلي."), 12.sp, color = p.textDim)
                Field(license, { license = it }, Modifier.fillMaxWidth(), hint = tr("License of this data (kept per record)", "ترخيص هذه البيانات (يحفظ لكل سجل)"), singleLine = true)
                GButton(tr("Choose file…", "اختيار ملف…"), { importPick.launch(arrayOf("*/*")) }, icon = Ic.IMPORT, accent = true, enabled = !ds.locked)
            }
        }
        Glass(Modifier.fillMaxWidth(), strength = 0.55f) {
            Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                T(tr("Export", "تصدير"), 16.sp, FontWeight.SemiBold)
                T(tr("File", "الملف"), 12.sp, FontWeight.SemiBold, p.textDim)
                ChipRow { listOf("jsonl", "json", "csv", "txt", "md").forEach { c -> Chip(c, container == c) { container = c } } }
                if (container in listOf("jsonl", "json", "csv")) {
                    T(tr("Structure", "البنية"), 12.sp, FontWeight.SemiBold, p.textDim)
                    ChipRow { listOf("messages", "sharegpt", "alpaca", "chatml", "raw").forEach { c -> Chip(c, profile == c) { profile = c } } }
                    Field(roles, { roles = it }, Modifier.fillMaxWidth(), hint = tr("Role names, e.g. assistant=model, user=human", "أسماء الأدوار مثل assistant=model, user=human"), singleLine = true, mono = true)
                }
                T(tr("Split", "القسم"), 12.sp, FontWeight.SemiBold, p.textDim)
                ChipRow { Chip(tr("All", "الكل"), split == null) { split = null }; SPLITS.forEach { s -> Chip(s, split == s) { split = s } } }
                Toggle(meta, { meta = it }, tr("Include provenance metadata (origin, source, license)", "تضمين بيانات المصدر (المصدر، الترخيص)"))
                if (last == null) T(tr("This dataset has not been validated. Consider running validation first.", "لم يتم التحقق من هذه المجموعة. يفضل تشغيل التحقق أولا."), 12.sp, color = p.warn)
                else if (last!!.getInt("errors") > 0) T(tr("The last validation found ${last!!.getInt("errors")} errors.", "آخر تحقق وجد ${last!!.getInt("errors")} أخطاء."), 12.sp, color = p.warn)
                if (ds.locked || split == "test" || split == "validation") T(tr("This is evaluation data. Keep it out of training files.", "هذه بيانات تقييم. أبقها خارج ملفات التدريب."), 12.sp, color = p.warn)
                GButton(tr("Export…", "تصدير…"), { exportPick.launch("${ds.name.replace(Regex("[^\\p{L}\\p{N}_-]+"), "_")}.$container") }, icon = Ic.EXPORT, accent = true)
            }
        }
        report?.let { Glass(Modifier.fillMaxWidth(), strength = 0.7f) { Column(Modifier.padding(14.dp)) { T(it, 13.sp, mono = true) } } }
        Box(Modifier.padding(bottom = 120.dp)) {}
    }
}

// ------------------------------------------------------------------ batch from template

@Composable
fun BatchTab(st: AppState, ds: Dataset) {
    val p = LocalPalette.current
    var template by remember { mutableStateOf(kindInfo(ds.kind).template) }
    var vars by remember { mutableStateOf("") }
    var limit by remember { mutableStateOf("100") }
    var preview by remember { mutableStateOf<Pair<List<JSONObject>, List<String>>?>(null) }

    fun parseVars(): Map<String, List<String>> {
        val out = LinkedHashMap<String, List<String>>()
        vars.lines().forEach { line ->
            val i = line.indexOf(':')
            if (i > 0) out[line.substring(0, i).trim()] = line.substring(i + 1).split("|").map { it.trim() }.filter { it.isNotEmpty() }
        }
        return out
    }
    Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Glass(Modifier.fillMaxWidth(), strength = 0.55f) {
            Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                T(tr("Template batch", "دفعة من قالب"), 16.sp, FontWeight.SemiBold)
                T(tr("Write a JSON template with {{variables}}. Every combination of the values you list becomes one record. Preview first; exact duplicates are dropped. Records are created with origin \"template\" and stay fully editable. Empty fields (like an empty answer) are left for you to write and will show up in validation.",
                    "اكتب قالب JSON فيه {{متغيرات}}. كل تركيبة من القيم التي تدخلها تصبح سجلا. عاين أولا؛ تحذف المكررات الحرفية. تنشأ السجلات بمصدر \"template\" وتبقى قابلة للتحرير. الحقول الفارغة (مثل جواب فارغ) تترك لك لتكتبها وتظهر في التحقق."), 12.sp, color = p.textDim)
                Field(template, { template = it }, Modifier.fillMaxWidth(), minHeight = 110.dp, mono = true)
                Field(vars, { vars = it }, Modifier.fillMaxWidth(), hint = "question: What is X? | What is Y?\nsystem: Be brief.", minHeight = 90.dp, mono = true)
                Field(limit, { limit = it }, Modifier.fillMaxWidth(), hint = tr("Max records", "أقصى عدد"), singleLine = true, keyboard = KeyboardOptions(keyboardType = KeyboardType.Number))
                GButton(tr("Preview", "معاينة"), { preview = expandTemplate(template, parseVars(), (limit.toIntOrNull() ?: 100).coerceIn(1, 5000)) })
                preview?.let { (list, problems) ->
                    T(tr("${list.size} records would be created", "سيتم إنشاء ${list.size} سجل"), 14.sp, FontWeight.SemiBold)
                    problems.forEach { T(it, 12.sp, color = p.warn) }
                    list.take(3).forEach { T(it.toString().take(300), 12.sp, color = p.textDim, mono = true) }
                    GButton(tr("Create ${list.size} records", "إنشاء ${list.size} سجل"), {
                        val items = list.map { NewRec(it, "template", "template:${ds.name}", "user-authored") }
                        st.work(tr("Creating records", "إنشاء السجلات"), { items.chunked(500).forEach { st.repo.insertRecords(ds.id, it) } }) { preview = null; st.toast(tr("Records created", "تم إنشاء السجلات")) }
                    }, accent = true, enabled = list.isNotEmpty() && !ds.locked)
                }
            }
        }
        Box(Modifier.padding(bottom = 120.dp)) {}
    }
}

// ------------------------------------------------------------------ info

@Composable
fun InfoTab(st: AppState, ds: Dataset) {
    val p = LocalPalette.current
    var name by remember(ds.id) { mutableStateOf(ds.name) }
    var desc by remember(ds.id) { mutableStateOf(ds.description) }
    var lang by remember(ds.id) { mutableStateOf(ds.language) }
    var req by remember(ds.id) { mutableStateOf(ds.required.joinToString(", ")) }
    var confirmDelete by remember { mutableStateOf(false) }
    val prov by ioState(JSONObject(), ds.id, st.rev) { st.repo.provenanceSummary(ds.id) }
    val log by ioState(emptyList<com.monex.dataforge.data.HistoryItem>(), ds.id, st.rev) { st.repo.datasetLog(ds.id) }
    Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Glass(Modifier.fillMaxWidth(), strength = 0.55f) {
            Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                T(tr("Settings", "الإعدادات"), 16.sp, FontWeight.SemiBold)
                Field(name, { name = it }, Modifier.fillMaxWidth(), hint = tr("Name", "الاسم"), singleLine = true)
                Field(desc, { desc = it }, Modifier.fillMaxWidth(), hint = tr("Description, purpose, writing guidelines", "الوصف والهدف وإرشادات الكتابة"), minHeight = 90.dp)
                ChipRow { listOf("ar", "en", "fr", "mixed").forEach { l -> Chip(l, lang == l) { lang = l } } }
                Field(req, { req = it }, Modifier.fillMaxWidth(), hint = tr("Required fields", "الحقول المطلوبة"), singleLine = true, mono = true)
                GButton(tr("Save", "حفظ"), {
                    val r = req.split(",").map { it.trim() }.filter { it.isNotEmpty() }
                    st.work(tr("Saving", "حفظ"), { st.repo.updateDatasetMeta(ds.id, name.ifBlank { ds.name }, desc, lang, r) })
                }, icon = Ic.SAVE, accent = true)
            }
        }
        Glass(Modifier.fillMaxWidth(), strength = 0.55f) {
            Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                T(tr("Provenance", "المصدر"), 16.sp, FontWeight.SemiBold)
                T(tr("Imported/transformed records with unknown license: ${prov.optInt("unknown_license")}", "سجلات مستوردة/محولة بترخيص غير معروف: ${prov.optInt("unknown_license")}"), 13.sp, color = if (prov.optInt("unknown_license") > 0) p.warn else p.ok)
                T(tr("Imported/transformed records without a source: ${prov.optInt("missing_source")}", "سجلات مستوردة/محولة بلا مصدر: ${prov.optInt("missing_source")}"), 13.sp, color = if (prov.optInt("missing_source") > 0) p.warn else p.ok)
                T(tr("Per-record origin, source, license and edit history are shown in each record's editor.", "مصدر كل سجل وترخيصه وتاريخ تعديله يظهر في محرر السجل."), 12.sp, color = p.textDim)
            }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            GButton(tr("Duplicate", "نسخ"), { st.work(tr("Duplicating", "نسخ"), { st.repo.duplicateDataset(ds.id) }) { st.toast(tr("Copy created", "تم إنشاء نسخة")) } }, icon = Ic.COPY)
            GButton(tr("Delete dataset", "حذف المجموعة"), { confirmDelete = true }, icon = Ic.TRASH, danger = true)
        }
        Glass(Modifier.fillMaxWidth(), strength = 0.5f) {
            Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                T(tr("Dataset log", "سجل المجموعة"), 16.sp, FontWeight.SemiBold)
                log.forEach { T("${fmtTime(it.ts)}  ${it.op}  ${it.detail}", 12.sp, color = p.textDim, mono = true) }
            }
        }
        Box(Modifier.padding(bottom = 120.dp)) {}
    }
    if (confirmDelete) ConfirmDialog(
        tr("Delete \"${ds.name}\"?", "حذف \"${ds.name}\"؟"),
        tr("All records, snapshots and reports of this dataset are permanently deleted.", "تحذف كل سجلات المجموعة ولقطاتها وتقاريرها نهائيا."),
        tr("Delete forever", "حذف نهائي"), true, { st.work(tr("Deleting", "حذف"), { st.repo.deleteDataset(ds.id) }) { st.pop() } }, { confirmDelete = false })
}
