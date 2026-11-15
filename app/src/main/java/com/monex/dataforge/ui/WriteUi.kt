package com.monex.dataforge.ui

// Quick Write (plain-text authoring), Find (duplicates + token estimate) and SQL console.

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.OffsetMapping
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.input.TransformedText
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.style.TextDirection
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.monex.dataforge.core.NativeCore
import com.monex.dataforge.data.Dataset
import com.monex.dataforge.data.NewRec
import com.monex.dataforge.ops.datasetValidationKind
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject

// ------------------------------------------------------------------ pure script helpers

private val CONV_LABELS = listOf("System", "User", "Assistant", "Tool")

private class LabelColors(val labels: Set<String>, val accent: Color, val dim: Color) : VisualTransformation {
    override fun filter(text: AnnotatedString): TransformedText {
        val b = AnnotatedString.Builder(text.text)
        var pos = 0
        for (line in text.text.split('\n')) {
            val lead = line.length - line.trimStart().length
            val idx = line.indexOf(':')
            if (idx > 0 && line.substring(0, idx).trim().lowercase() in labels) {
                b.addStyle(SpanStyle(color = accent, fontWeight = FontWeight.Bold), pos + lead, pos + idx + 1)
            } else if (line.trim().length >= 3 && line.trim().all { it == '-' }) {
                b.addStyle(SpanStyle(color = dim), pos, pos + line.length)
            }
            pos += line.length + 1
        }
        return TransformedText(b.toAnnotatedString(), OffsetMapping.Identity)
    }
}

private fun copyText(ctx: Context, label: String, text: String) {
    val cm = ctx.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
    cm.setPrimaryClip(ClipData.newPlainText(label, text))
}

private class Built(val records: JSONArray, val warnings: List<String>, val tokens: Int, val messages: Int, val errors: Int, val firstErrors: List<String>)

// ------------------------------------------------------------------ Quick Write

@Composable
fun WriteScreen(st: AppState, dsId: String) {
    val ds by ioState<Dataset?>(null, dsId, st.rev) { st.repo.dataset(dsId) }
    val d = ds
    if (d == null) { Page("…", onBack = { st.pop() }) {}; return }
    WriteBody(st, d)
}

@Composable
private fun WriteBody(st: AppState, ds: Dataset) {
    val p = LocalPalette.current
    val conv = isConversationKind(ds.kind)
    val fields = remember(ds.id) { ds.required.ifEmpty { kindInfo(ds.kind).required } }
    val labels = if (conv) CONV_LABELS else fields
    val draftKey = "write_" + ds.id
    val firstLabel = if (conv) "User" else (fields.firstOrNull() ?: "text")
    var tv by remember(ds.id) {
        val saved = st.prefs.getString(draftKey, null)
        val t = if (saved.isNullOrEmpty()) "$firstLabel: " else saved
        mutableStateOf(TextFieldValue(t, TextRange(t.length)))
    }
    var built by remember { mutableStateOf<Built?>(null) }
    var live by remember { mutableStateOf<String?>(null) }
    var showBuild by remember { mutableStateOf(false) }
    var confirmClear by remember { mutableStateOf(false) }
    val ctx = st.ctx

    fun request(text: String): JSONObject {
        val o = JSONObject().put("text", text).put("mode", if (conv) "conversation" else "fields")
        if (!conv) o.put("fields", JSONArray(fields))
        return o
    }

    fun compute(text: String): Built {
        val r = NativeCore.run("script", request(text))
        val recs = r.getJSONArray("records")
        val w = r.getJSONArray("warnings")
        val warnings = (0 until w.length()).map { w.getString(it) }
        var errors = 0
        val first = ArrayList<String>()
        if (recs.length() > 0) {
            val arr = JSONArray()
            for (i in 0 until recs.length()) arr.put(JSONObject().put("id", "#${i + 1}").put("data", recs.getJSONObject(i)))
            val cfg = JSONObject().put("kind", datasetValidationKind(ds.kind)).put("required", JSONArray(ds.required)).put("dedup", false).put("near_threshold", 0)
            val v = NativeCore.run("validate", JSONObject().put("config", cfg).put("records", arr))
            errors = v.optInt("errors", 0)
            val f = v.optJSONArray("findings")
            if (f != null) for (i in 0 until f.length()) {
                val o = f.getJSONObject(i)
                if (o.optString("severity") == "error" && first.size < 3) first.add(o.optString("record", "") + " " + o.optString("message", o.optString("rule", "")))
            }
        }
        return Built(recs, warnings, r.optInt("total_tokens"), r.optInt("messages"), errors, first)
    }

    // Live counters (debounced) + draft autosave.
    LaunchedEffect(tv.text) {
        delay(300)
        st.prefs.edit().putString(draftKey, tv.text).apply()
        live = try {
            val r = withContext(Dispatchers.Default) { NativeCore.run("script", request(tv.text)) }
            val n = r.getJSONArray("records").length()
            val w = r.getJSONArray("warnings").length()
            tr("$n records · ~${r.optInt("total_tokens")} tokens" + (if (w > 0) " · $w warnings" else ""),
                "$n سجل · ~${r.optInt("total_tokens")} توكن" + (if (w > 0) " · $w تحذير" else ""))
        } catch (e: Exception) { e.message ?: "?" }
    }

    fun onType(nv: TextFieldValue) {
        val old = tv
        if (nv.text.length == old.text.length + 1 && nv.selection.collapsed) {
            val pos = nv.selection.start
            if (pos > 0 && nv.text[pos - 1] == '\n') {
                val before = nv.text.substring(0, pos - 1)
                // The Rust engine decides the next role (alternation / field cycle) and detects empty label lines.
                val r = NativeCore.run("enter", request(before).also { it.remove("text"); it.put("before", before) })
                if (r.getBoolean("empty")) { tv = old; return }
                val ins = r.getString("insert")
                val t = nv.text.substring(0, pos) + ins + nv.text.substring(pos)
                tv = TextFieldValue(t, TextRange(pos + ins.length))
                return
            }
        }
        tv = nv
    }

    fun insertAtCursor(s: String) {
        val t = tv.text; val c = tv.selection.start.coerceIn(0, t.length)
        tv = TextFieldValue(t.substring(0, c) + s + t.substring(tv.selection.end.coerceIn(c, t.length)), TextRange(c + s.length))
    }

    fun insertLabel(l: String) {
        val t = tv.text; val c = tv.selection.start.coerceIn(0, t.length)
        val atLineStart = c == 0 || t[c - 1] == '\n'
        insertAtCursor((if (atLineStart) "" else "\n") + "$l: ")
    }

    val saver = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("application/x-ndjson")) { uri ->
        val b = built
        if (uri != null && b != null) st.work(tr("Saving…", "جارٍ الحفظ…"), {
            val arr = JSONArray(); for (i in 0 until b.records.length()) arr.put(b.records.getJSONObject(i))
            val text = NativeCore.run("to_jsonl", JSONObject().put("records", arr)).getString("text")
            ctx.contentResolver.openOutputStream(uri)?.use { it.write(text.toByteArray(Charsets.UTF_8)) }
            arr.length()
        }) { n -> st.toast(tr("Saved $n lines (.jsonl)", "تم حفظ $n سطر (.jsonl)")) }
    }

    Page(ds.name, onBack = { st.pop() }, actions = {
        GButton(tr("Build", "بناء"), {
            try { built = compute(tv.text); showBuild = true } catch (e: Exception) { st.toast(e.message ?: "error") }
        }, icon = Ic.BOLT, accent = true)
    }) {
        Column(Modifier.weight(1f).fillMaxWidth().imePadding().padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            ChipRow {
                labels.forEach { l -> Chip(l, false) { insertLabel(l) } }
                Chip(tr("↵ new line", "↵ سطر جديد"), false) { insertAtCursor("\n") }
                Chip("---", false) { insertAtCursor((if (tv.text.endsWith("\n") || tv.text.isEmpty()) "" else "\n") + "---\n") }
                Chip(tr("Clear", "مسح"), false) { confirmClear = true }
            }
            Glass(Modifier.weight(1f).fillMaxWidth(), radius = 16.dp) {
                // Mixed Arabic/Latin text: each paragraph picks its own direction.
                BasicTextField(
                    value = tv, onValueChange = { onType(it) },
                    textStyle = TextStyle(color = p.text, fontSize = 16.sp, lineHeight = 24.sp, textDirection = TextDirection.Content),
                    cursorBrush = SolidColor(p.accent),
                    visualTransformation = LabelColors(labels.map { it.lowercase() }.toSet() + setOf("human", "ai"), p.accent, p.textDim),
                    modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(14.dp)
                )
            }
            T(live ?: "…", 12.sp, color = p.textDim)
            T(
                if (conv) tr("Type a message and press Enter: the next role appears automatically. Use “↵ new line” for a line break inside a message.",
                    "اكتب الرسالة ثم Enter: يظهر الدور التالي تلقائيا. استعمل «↵ سطر جديد» لسطر داخل نفس الرسالة.")
                else tr("Each Enter moves to the next field (${fields.joinToString(" → ")}).", "كل Enter ينتقل للحقل التالي (${fields.joinToString(" ← ")})."),
                12.sp, color = p.textDim
            )
        }
    }

    if (confirmClear) ConfirmDialog(tr("Clear the text?", "مسح النص؟"), tr("The draft will be erased.", "سيتم حذف المسودة."), tr("Clear", "مسح"), true,
        { tv = TextFieldValue("$firstLabel: ", TextRange(firstLabel.length + 2)) }, { confirmClear = false })

    val b = built
    if (showBuild && b != null) Modal({ showBuild = false }) {
        T(tr("Build result", "نتيجة البناء"), 18.sp, FontWeight.Bold)
        T(tr("${b.records.length()} records · ${b.messages} messages · ~${b.tokens} tokens", "${b.records.length()} سجل · ${b.messages} رسالة · ~${b.tokens} توكن"), 13.sp, color = p.textDim)
        if (b.errors > 0) T(tr("${b.errors} validation errors", "${b.errors} أخطاء تحقق") + (if (b.firstErrors.isNotEmpty()) ": " + b.firstErrors.joinToString("; ") else ""), 13.sp, color = p.danger)
        b.warnings.take(4).forEach { T("• $it", 12.sp, color = p.warn) }
        if (b.records.length() > 0) {
            val pretty = NativeCore.pretty(b.records.getJSONObject(0).toString())
            Box(Modifier.fillMaxWidth().background(p.field, androidx.compose.foundation.shape.RoundedCornerShape(12.dp)).padding(10.dp).horizontalScroll(rememberScrollState())) {
                CompositionLocalProvider(LocalLayoutDirection provides LayoutDirection.Ltr) { T(pretty.take(1500), 12.sp, mono = true) }
            }
            if (b.records.length() > 1) T(tr("… and ${b.records.length() - 1} more", "… و${b.records.length() - 1} أخرى"), 12.sp, color = p.textDim)
        }
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            GButton(tr("Add to dataset", "أضف للمجموعة"), {
                showBuild = false
                st.work(tr("Adding…", "جارٍ الإضافة…"), {
                    val items = (0 until b.records.length()).map { NewRec(b.records.getJSONObject(it), "human") }
                    st.repo.insertRecords(ds.id, items).size
                }) { n ->
                    st.bump(); st.toast(tr("Added $n records", "تمت إضافة $n سجل"))
                    tv = TextFieldValue("$firstLabel: ", TextRange(firstLabel.length + 2))
                }
            }, Modifier.weight(1f), accent = true, enabled = b.records.length() > 0 && !ds.locked)
        }
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            GButton(".jsonl", { showBuild = false; saver.launch("${ds.name}.jsonl") }, Modifier.weight(1f), icon = Ic.SAVE, enabled = b.records.length() > 0)
            GButton("JSON", {
                val arr = JSONArray(); for (i in 0 until b.records.length()) arr.put(b.records.getJSONObject(i))
                copyText(ctx, "json", NativeCore.pretty(arr.toString())); st.toast(tr("Copied", "تم النسخ"))
            }, Modifier.weight(1f), icon = Ic.COPY, enabled = b.records.length() > 0)
        }
        GButton(tr("Close", "إغلاق"), { showBuild = false }, Modifier.fillMaxWidth())
    }
}

// ------------------------------------------------------------------ Find (duplicates + tokens)

@Composable
fun FindTab(st: AppState, ds: Dataset) {
    val p = LocalPalette.current
    var thr by remember { mutableStateOf(0.85) }
    var result by remember(ds.id) { mutableStateOf<JSONObject?>(null) }
    var tok by remember(ds.id) { mutableStateOf<JSONObject?>(null) }
    var confirmGroup by remember { mutableStateOf<List<String>?>(null) }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        SectionTitle(tr("Token estimate", "تقدير التوكنز"))
        Glass(Modifier.fillMaxWidth()) {
            Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                val t = tok
                if (t != null) {
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        StatTile(tr("total", "المجموع"), t.optInt("total").toString(), Modifier.weight(1f))
                        StatTile(tr("avg / record", "المتوسط"), "%.0f".format(t.optDouble("avg")), Modifier.weight(1f))
                        StatTile(tr("max", "الأكبر"), t.optInt("max").toString(), Modifier.weight(1f))
                    }
                }
                T(tr("A heuristic estimate (±25%), not a real tokenizer. Chat-template overhead is not included.", "تقدير تقريبي (±25%) وليس tokenizer حقيقيا، ولا يشمل إضافات قالب المحادثة."), 12.sp, color = p.textDim)
                GButton(tr("Estimate tokens", "احسب التوكنز"), { st.work(tr("Estimating…", "جارٍ الحساب…"), { st.repo.tokenTotals(ds.id) }) { tok = it } }, Modifier.fillMaxWidth(), icon = Ic.BOLT)
            }
        }
        SectionTitle(tr("Find duplicates", "البحث عن المكرر"))
        ChipRow {
            listOf(1.0 to tr("Exact only", "مطابق فقط"), 0.95 to "95%", 0.85 to "85%", 0.7 to "70%").forEach { (v, l) -> Chip(l, thr == v) { thr = v } }
        }
        GButton(tr("Find", "ابحث"), {
            st.work(tr("Searching duplicates…", "جارٍ البحث عن المكرر…"), {
                val texts = st.repo.texts(ds.id, com.monex.dataforge.ops.DEDUP_MAX_RECORDS, 4000)
                val arr = JSONArray(); texts.forEach { arr.put(JSONObject().put("id", it.first).put("text", it.second)) }
                NativeCore.run("dedup", JSONObject().put("items", arr).put("threshold", if (thr >= 1.0) 0.0 else thr))
            }) { result = it }
        }, Modifier.fillMaxWidth(), icon = Ic.SEARCH, accent = true)
        val r = result
        if (r != null) {
            val groups = r.getJSONArray("exact_groups")
            val pairs = r.getJSONArray("near_pairs")
            T(tr("${groups.length()} exact groups · ${pairs.length()} near-duplicate pairs · scanned ${r.optInt("scope")}",
                "${groups.length()} مجموعة مطابقة · ${pairs.length()} زوج متشابه · تم فحص ${r.optInt("scope")}"), 13.sp, color = p.textDim)
            for (gi in 0 until minOf(groups.length(), 50)) {
                val ids = groups.getJSONArray(gi).let { a -> (0 until a.length()).map { a.getString(it) } }
                Glass(Modifier.fillMaxWidth()) {
                    Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Badge(tr("exact · ${ids.size} copies", "مطابق · ${ids.size} نسخ"), p.danger)
                        ids.take(4).forEach { id -> DupLine(st, id) }
                        GButton(tr("Keep first, delete ${ids.size - 1}", "أبقِ الأول واحذف ${ids.size - 1}"), { confirmGroup = ids }, Modifier.fillMaxWidth(), icon = Ic.TRASH, danger = true, enabled = !ds.locked)
                    }
                }
            }
            for (pi in 0 until minOf(pairs.length(), 50)) {
                val o = pairs.getJSONObject(pi)
                Glass(Modifier.fillMaxWidth()) {
                    Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Badge(tr("similar · ${(o.getDouble("similarity") * 100).toInt()}%", "متشابه · ${(o.getDouble("similarity") * 100).toInt()}%"), p.warn)
                        DupLine(st, o.getString("a"))
                        DupLine(st, o.getString("b"))
                    }
                }
            }
        }
    }
    confirmGroup?.let { ids ->
        ConfirmDialog(tr("Delete duplicates?", "حذف المكررات؟"), tr("${ids.size - 1} records will be deleted. Their last content stays in history.", "سيتم حذف ${ids.size - 1} سجل ويبقى محتواها في السجل التاريخي."),
            tr("Delete", "حذف"), true, {
                st.work("…", { st.repo.deleteRecords(ids.drop(1)) }) { st.bump(); result = null; st.toast(tr("Deleted", "تم الحذف")) }
            }, { confirmGroup = null })
    }
}

@Composable
private fun DupLine(st: AppState, id: String) {
    val p = LocalPalette.current
    val prev by ioState("", id) { st.repo.record(id)?.let { previewOf(it.data) } ?: "" }
    Box(Modifier.fillMaxWidth().pressable { st.repo.record(id)?.let { r -> st.push(Screen.EditorS(r.datasetId, r.id)) } }
        .background(p.field, androidx.compose.foundation.shape.RoundedCornerShape(10.dp)).padding(10.dp)) {
        T(prev.ifEmpty { id }, 13.sp, maxLines = 3)
    }
}

// ------------------------------------------------------------------ SQL console

@Composable
fun SqlTab(st: AppState, ds: Dataset) {
    val p = LocalPalette.current
    val presets = listOf(
        tr("By split", "حسب التقسيم") to "SELECT split, COUNT(*) AS n FROM records WHERE dataset_id=:ds GROUP BY split",
        tr("By origin", "حسب المصدر") to "SELECT origin, COUNT(*) AS n FROM records WHERE dataset_id=:ds GROUP BY origin ORDER BY n DESC",
        tr("Longest", "الأطول") to "SELECT id, LENGTH(data) AS chars FROM records WHERE dataset_id=:ds ORDER BY chars DESC LIMIT 20",
        tr("Recent", "الأحدث") to "SELECT id, datetime(updated/1000,'unixepoch') AS updated, split FROM records WHERE dataset_id=:ds ORDER BY updated DESC LIMIT 20",
        tr("History", "السجل") to "SELECT datetime(ts/1000,'unixepoch') AS time, op, detail FROM dataset_log WHERE dataset_id=:ds ORDER BY ts DESC LIMIT 30",
        tr("Contains", "يحتوي") to "SELECT id, substr(data,1,80) AS preview FROM records WHERE dataset_id=:ds AND data LIKE '%text%' LIMIT 20"
    )
    var q by remember(ds.id) { mutableStateOf(presets[0].second) }
    var res by remember(ds.id) { mutableStateOf<com.monex.dataforge.data.Repo.SqlResult?>(null) }
    var err by remember(ds.id) { mutableStateOf<String?>(null) }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        T(tr("Read-only SQL (SELECT). Use :ds for this dataset. Tables: records, record_history, dataset_log, versions, version_records, validation_runs.",
            "SQL للقراءة فقط (SELECT). استعمل :ds لهذه المجموعة. الجداول: records, record_history, dataset_log, versions, version_records, validation_runs."), 12.sp, color = p.textDim)
        ChipRow { presets.forEach { (l, s) -> Chip(l, false) { q = s } } }
        Field(q, { q = it }, Modifier.fillMaxWidth(), mono = true, minHeight = 96.dp)
        GButton(tr("Run", "تشغيل"), {
            try { res = st.repo.sql(ds.id, q); err = null } catch (e: Exception) { res = null; err = e.message ?: "error" }
        }, Modifier.fillMaxWidth(), icon = Ic.PLAY, accent = true)
        err?.let { T(it, 13.sp, color = p.danger, mono = true) }
        res?.let { r ->
            T(tr("${r.rows.size} rows" + (if (r.truncated) " (truncated)" else ""), "${r.rows.size} صف" + (if (r.truncated) " (مقتطع)" else "")), 12.sp, color = p.textDim)
            CompositionLocalProvider(LocalLayoutDirection provides LayoutDirection.Ltr) {
                Glass(Modifier.fillMaxWidth(), radius = 14.dp) {
                    Column(Modifier.horizontalScroll(rememberScrollState()).padding(10.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        Row(horizontalArrangement = Arrangement.spacedBy(14.dp)) { r.columns.forEach { c -> T(c, 12.sp, FontWeight.Bold, p.accent, Modifier.width(120.dp), maxLines = 1, mono = true) } }
                        r.rows.forEach { row ->
                            Row(horizontalArrangement = Arrangement.spacedBy(14.dp)) { row.forEach { c -> T(c.replace('\n', ' '), 12.sp, modifier = Modifier.width(120.dp), maxLines = 2, mono = true) } }
                        }
                    }
                }
            }
        }
    }
}
