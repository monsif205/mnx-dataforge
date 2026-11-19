package com.monex.dataforge.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.monex.dataforge.core.NativeCore
import com.monex.dataforge.data.Dataset
import com.monex.dataforge.data.Source
import com.monex.dataforge.ops.GATE_ALLOW
import com.monex.dataforge.ops.GATE_REVIEW_ACK
import com.monex.dataforge.ops.Gate
import com.monex.dataforge.ops.discardJob
import com.monex.dataforge.ops.fetchSource
import com.monex.dataforge.ops.runJob
import com.monex.dataforge.ops.licenseGate
import com.monex.dataforge.ops.urlProblem
import org.json.JSONArray
import org.json.JSONObject

// ------------------------------------------------------------------ sources

@Composable
fun SourcesScreen(st: AppState, datasetId: String) {
    val p = LocalPalette.current
    val ds by ioState<Dataset?>(null, datasetId, st.rev) { st.repo.dataset(datasetId) }
    val d = ds
    if (d == null) { Page("…", onBack = { st.pop() }) {}; return }
    val sources by ioState(emptyList<Source>(), datasetId, st.rev) { st.repo.sources(datasetId) }
    var adding by remember { mutableStateOf(false) }
    var report by remember { mutableStateOf<String?>(null) }
    var expanded by remember { mutableStateOf<String?>(null) }
    var keyFor by remember { mutableStateOf<Pair<Source, Boolean>?>(null) }
    var deleteId by remember { mutableStateOf<Source?>(null) }

    fun runFetch(src: Source, key: String?, full: Boolean) {
        st.work(tr("Fetching ${src.name}", "جلب ${src.name}"), {
            fetchSource(st.ctx, st.repo, d, src, key, full, st::progress) { st.cancelFlag.get() }
        }) { o ->
            report = (if (o.unchanged) o.note else tr("Imported ${o.imported}, unchanged ${o.skipped}, rejected ${o.rejected}. ${o.note}", "تم استيراد ${o.imported}، دون تغيير ${o.skipped}، ورُفض ${o.rejected}. ${o.note}")) +
                (if (o.errors.isNotEmpty()) "\n" + o.errors.take(12).joinToString("\n") else "")
        }
    }
    fun start(src: Source, full: Boolean) {
        if (src.kind == "api" && src.config.optString("header_name").isNotBlank()) keyFor = src to full else runFetch(src, null, full)
    }

    Page(tr("Sources", "المصادر"), onBack = { st.pop() }, actions = { GButton(tr("Add", "إضافة"), { adding = true }, icon = Ic.PLUS, accent = true, enabled = !d.locked) }) {
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Glass(Modifier.fillMaxWidth(), strength = 0.55f) {
                Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    T(tr("Downloads and licensed sources", "التنزيل والمصادر المرخصة"), 16.sp, FontWeight.SemiBold)
                    T(tr("A source is accepted only after the License Gate. Files are downloaded in resumable parallel chunks, verified with SHA-256, read as a stream, and each version is recorded. \"Update\" fetches only what changed. Downloads run while the app is open; if Android stops the app or the network drops, the next start continues from the saved chunks.",
                        "لا يقبل أي مصدر إلا بعد بوابة الترخيص. تنزل الملفات بأجزاء متوازية قابلة للاستئناف، وتتحقق بـ SHA-256، وتقرأ تدفقيا، ويسجل كل إصدار. \"تحديث\" يجلب ما تغير فقط. التنزيل يعمل والتطبيق مفتوح؛ إذا أوقف أندرويد التطبيق أو انقطعت الشبكة يكمل التشغيل التالي من الأجزاء المحفوظة."), 12.sp, color = p.textDim)
                    T(tr("Supported: JSONL, JSON, CSV, TSV, TXT, MD, HTML pages, Parquet, DOCX, XLSX (first sheet), PDF (text only, no OCR), and ZIP / TAR / gzip / Zstandard of those.", "المدعوم: JSONL وJSON وCSV وTSV وTXT وMD وصفحات HTML وParquet وDOCX وXLSX (الورقة الأولى) وPDF (نص فقط، بلا OCR) وأرشيفات ZIP وTAR وgzip وZstandard منها."), 12.sp, color = p.textDim)
                }
            }
            JobsCard(st, d) { report = it }
            report?.let { Glass(Modifier.fillMaxWidth(), strength = 0.7f) { Column(Modifier.padding(14.dp)) { T(it, 13.sp, mono = true) } } }
            if (sources.isEmpty()) T(tr("No sources yet. Tap Add to register a file URL or a JSON API.", "لا توجد مصادر بعد. اضغط إضافة لتسجيل رابط ملف أو واجهة JSON."), 14.sp, color = p.textDim)
            for (s in sources) {
                Glass(Modifier.fillMaxWidth(), strength = 0.55f) {
                    Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            T(s.name, 16.sp, FontWeight.SemiBold, maxLines = 1, modifier = Modifier.weight(1f))
                            Badge(s.kind.uppercase(), p.accent)
                            Badge(if (s.licenseStatus == GATE_ALLOW) s.license else s.license + " · " + tr("reviewed", "مراجَع"), if (s.licenseStatus == GATE_ALLOW) p.ok else p.warn)
                        }
                        T(s.url, 12.sp, color = p.textDim, maxLines = 2, mono = true)
                        val bits = ArrayList<String>()
                        if (s.size > 0) bits.add(fmtBytes(s.size))
                        if (!s.sha256.isNullOrEmpty()) bits.add("sha256 " + s.sha256.take(12) + "…")
                        bits.add(if (s.lastImport > 0) tr("last import ", "آخر استيراد ") + fmtTime(s.lastImport) else tr("never imported", "لم يستورد بعد"))
                        T(bits.joinToString("  ·  "), 12.sp, color = p.textDim)
                        if (s.licenseNote.isNotBlank()) T(s.licenseNote, 12.sp, color = p.textDim)
                        ChipRow {
                            GButton(if (s.lastImport > 0) tr("Update", "تحديث") else tr("Fetch", "جلب"), { start(s, false) }, icon = Ic.BOLT, accent = true, enabled = !d.locked)
                            if (s.lastImport > 0) GButton(tr("Full re-import", "إعادة كاملة"), { start(s, true) }, enabled = !d.locked)
                            GButton(if (expanded == s.id) tr("Hide history", "إخفاء السجل") else tr("History", "السجل"), { expanded = if (expanded == s.id) null else s.id })
                            GButton("", { deleteId = s }, icon = Ic.TRASH, danger = true)
                        }
                        if (expanded == s.id) SourceHistory(st, s)
                    }
                }
            }
            Box(Modifier.padding(bottom = 60.dp)) {}
        }
    }
    if (adding) AddSourceModal(st, d) { adding = false }
    keyFor?.let { (src, full) ->
        PromptDialog(tr("API key for ${src.name} (used for this run only, never stored)", "مفتاح API لـ ${src.name} (يستعمل لهذا التشغيل فقط ولا يخزن)"), "", tr("Fetch", "جلب"), { k -> runFetch(src, k.trim(), full) }) { keyFor = null }
    }
    deleteId?.let { s ->
        ConfirmDialog(tr("Remove this source?", "إزالة هذا المصدر؟"), tr("Only the source and its history are removed. Records already imported stay in the dataset.", "يزال المصدر وسجله فقط. تبقى السجلات المستوردة في المجموعة."),
            tr("Remove", "إزالة"), true, { st.work(tr("Removing", "إزالة"), { st.repo.deleteSource(s.id) }) }, { deleteId = null })
    }
}

@Composable
private fun SourceHistory(st: AppState, s: Source) {
    val p = LocalPalette.current
    val vs by ioState(emptyList<com.monex.dataforge.data.SourceVersion>(), s.id, st.rev) { st.repo.sourceVersions(s.id) }
    val meta = s.meta
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        SectionTitle(tr("Source metadata", "بيانات المصدر"))
        val keys = listOf("final_url", "content_type", "format", "file_name", "etag", "last_modified", "archive_entries", "api_records_fetched", "h_server", "h_x-license")
        val lines = keys.mapNotNull { k -> meta.opt(k)?.takeIf { it != JSONObject.NULL && it.toString().isNotBlank() }?.let { "$k: $it" } }
        if (lines.isEmpty()) T(tr("None yet.", "لا شيء بعد."), 12.sp, color = p.textDim) else T(lines.joinToString("\n"), 12.sp, color = p.textDim, mono = true)
        SectionTitle(tr("Versions", "الإصدارات"))
        if (vs.isEmpty()) T(tr("No fetches yet.", "لا عمليات جلب بعد."), 12.sp, color = p.textDim)
        vs.forEachIndexed { i, v ->
            T("${vs.size - i}. ${fmtTime(v.ts)}  +${v.imported}  =${v.skipped}  ✗${v.rejected}\n   ${v.note}" + if (!v.sha256.isNullOrEmpty()) "\n   sha256 ${v.sha256.take(16)}…" else "", 12.sp, color = p.textDim, mono = true)
        }
    }
}

@Composable
private fun AddSourceModal(st: AppState, ds: Dataset, onDismiss: () -> Unit) {
    val p = LocalPalette.current
    var kind by remember { mutableStateOf("url") }
    var url by remember { mutableStateOf("") }
    var name by remember { mutableStateOf("") }
    var license by remember { mutableStateOf("") }
    var hint by remember { mutableStateOf("") }
    var attested by remember { mutableStateOf(false) }
    var nc by remember { mutableStateOf(false) }
    var sa by remember { mutableStateOf(false) }
    var ack by remember { mutableStateOf(false) }
    var ackNote by remember { mutableStateOf("") }
    var allowHttp by remember { mutableStateOf(false) }
    var expected by remember { mutableStateOf("") }
    var parallel by remember { mutableStateOf("3") }
    var interval by remember { mutableStateOf("0") }
    var maxGb by remember { mutableStateOf("0") }
    // API
    var recordsPath by remember { mutableStateOf("") }
    var pageParam by remember { mutableStateOf("") }
    var sizeParam by remember { mutableStateOf("") }
    var pageSize by remember { mutableStateOf("100") }
    var cursorPath by remember { mutableStateOf("") }
    var cursorParam by remember { mutableStateOf("") }
    var maxPages by remember { mutableStateOf("50") }
    var headerName by remember { mutableStateOf("") }
    var headerPrefix by remember { mutableStateOf("") }

    val gate: Gate? = remember(license, hint, attested, nc, sa) {
        if (license.isBlank()) null else try { licenseGate(license, hint.split(",").map { it.trim() }.filter { it.isNotEmpty() }, attested, nc, sa) } catch (e: Exception) { null }
    }
    val problem = if (url.isBlank()) null else urlProblem(url, allowHttp)
    val gateOk = gate != null && (gate.status == "allow" || (gate.status == "review" && ack && ackNote.trim().length >= 5))
    val canAdd = url.isNotBlank() && problem == null && gateOk

    Modal(onDismiss) {
        T(tr("Add a source", "إضافة مصدر"), 18.sp, FontWeight.Bold)
        ChipRow {
            Chip(tr("File URL / web page", "رابط ملف / صفحة"), kind == "url") { kind = "url" }
            Chip(tr("JSON API", "واجهة JSON"), kind == "api") { kind = "api" }
        }
        Field(url, { url = it }, Modifier.fillMaxWidth(), hint = "https://…", singleLine = true, mono = true, keyboard = KeyboardOptions(keyboardType = KeyboardType.Uri))
        problem?.let { T(it, 12.sp, color = p.danger) }
        Field(name, { name = it }, Modifier.fillMaxWidth(), hint = tr("Name (shown as the record source)", "الاسم (يظهر كمصدر للسجل)"), singleLine = true)

        SectionTitle(tr("License Gate", "بوابة الترخيص"))
        T(tr("Being reachable on the web does not mean you may copy it or train on it. Enter the license the owner published.", "كون المحتوى مفتوحا على الإنترنت لا يعني أنه مسموح نسخه أو التدريب عليه. أدخل الترخيص الذي نشره المالك."), 12.sp, color = p.textDim)
        Field(license, { license = it }, Modifier.fillMaxWidth(), hint = tr("License (e.g. CC0-1.0, CC-BY-4.0, MIT, Apache-2.0)", "الترخيص (مثل CC0-1.0 أو CC-BY-4.0 أو MIT)"), singleLine = true, mono = true)
        ChipRow { listOf("CC0-1.0", "CC-BY-4.0", "CC-BY-SA-4.0", "MIT", "Apache-2.0", "ODC-By", "public domain", "own data").forEach { c -> Chip(c, license == c) { license = c } } }
        Field(hint, { hint = it }, Modifier.fillMaxWidth(), hint = tr("Other license names or URLs found on the site (optional, comma separated)", "أسماء أو روابط تراخيص أخرى على الموقع (اختياري)"), singleLine = true, mono = true)
        Toggle(attested, { attested = it }, tr("I own this data or have the owner's written permission", "أملك هذه البيانات أو لدي إذن كتابي من المالك"))
        Toggle(nc, { nc = it }, tr("My use is non-commercial research only", "استعمالي للبحث غير التجاري فقط"))
        Toggle(sa, { sa = it }, tr("I accept share-alike terms", "أقبل شروط المشاركة بالمثل"))
        if (license.isNotBlank() && gate == null) T(tr("License check failed.", "فشل فحص الترخيص."), 12.sp, color = p.danger)
        gate?.let { g ->
            val color = when (g.status) { "allow" -> p.ok; "review" -> p.warn; else -> p.danger }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Badge(g.id, color)
                Badge(when (g.status) { "allow" -> tr("ALLOWED", "مسموح"); "review" -> tr("NEEDS YOUR DECISION", "يحتاج قرارك"); else -> tr("BLOCKED", "محظور") }, color)
            }
            if (g.reasons.isNotEmpty()) T(g.reasons.joinToString("\n") { "• $it" }, 12.sp, color = p.textDim)
            if (g.attribution) T(tr("Attribution is required: source and license are saved with every record.", "الإسناد مطلوب: يحفظ المصدر والترخيص مع كل سجل."), 12.sp, color = p.textDim)
            if (g.status == "review") {
                Toggle(ack, { ack = it }, tr("I have read the license and accept responsibility for this use", "قرأت الترخيص وأتحمل مسؤولية هذا الاستعمال"))
                Field(ackNote, { ackNote = it }, Modifier.fillMaxWidth(), hint = tr("Why this use is acceptable (saved with the source)", "لماذا هذا الاستعمال مقبول (يحفظ مع المصدر)"), singleLine = true)
            }
            if (g.status == "deny") T(tr("This source cannot be added. Without a clear license that allows it, do not use the content for training.", "لا يمكن إضافة هذا المصدر. بدون ترخيص واضح يسمح بذلك لا تستعمل المحتوى للتدريب."), 12.sp, color = p.danger)
        }

        if (kind == "api") {
            SectionTitle(tr("API shape", "شكل الواجهة"))
            Field(recordsPath, { recordsPath = it }, Modifier.fillMaxWidth(), hint = tr("Path to the list of records, e.g. data.items (empty = the response itself)", "مسار قائمة السجلات مثل data.items (فارغ = الاستجابة نفسها)"), singleLine = true, mono = true)
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Field(pageParam, { pageParam = it }, Modifier.weight(1f), hint = tr("page param", "معامل الصفحة"), singleLine = true, mono = true)
                Field(sizeParam, { sizeParam = it }, Modifier.weight(1f), hint = tr("size param", "معامل الحجم"), singleLine = true, mono = true)
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Field(cursorPath, { cursorPath = it }, Modifier.weight(1f), hint = tr("next-cursor path", "مسار المؤشر التالي"), singleLine = true, mono = true)
                Field(cursorParam, { cursorParam = it }, Modifier.weight(1f), hint = tr("cursor param", "معامل المؤشر"), singleLine = true, mono = true)
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Field(pageSize, { pageSize = it }, Modifier.weight(1f), hint = tr("page size", "حجم الصفحة"), singleLine = true, mono = true, keyboard = KeyboardOptions(keyboardType = KeyboardType.Number))
                Field(maxPages, { maxPages = it }, Modifier.weight(1f), hint = tr("max pages", "أقصى صفحات"), singleLine = true, mono = true, keyboard = KeyboardOptions(keyboardType = KeyboardType.Number))
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Field(headerName, { headerName = it }, Modifier.weight(1f), hint = tr("key header, e.g. Authorization", "ترويسة المفتاح مثل Authorization"), singleLine = true, mono = true)
                Field(headerPrefix, { headerPrefix = it }, Modifier.weight(1f), hint = tr("prefix, e.g. Bearer ", "بادئة مثل Bearer "), singleLine = true, mono = true)
            }
            T(tr("If a key header is set, the key is asked each time and never stored.", "إذا حددت ترويسة المفتاح يطلب المفتاح في كل مرة ولا يخزن."), 12.sp, color = p.textDim)
        } else {
            Field(expected, { expected = it }, Modifier.fillMaxWidth(), hint = tr("Expected SHA-256 (optional; a mismatch discards the file)", "SHA-256 المتوقع (اختياري؛ عدم التطابق يلغي الملف)"), singleLine = true, mono = true)
        }
        SectionTitle(tr("Download settings", "إعدادات التنزيل"))
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Field(parallel, { parallel = it }, Modifier.weight(1f), hint = tr("parallel (1-8)", "التوازي (1-8)"), singleLine = true, mono = true, keyboard = KeyboardOptions(keyboardType = KeyboardType.Number))
            Field(interval, { interval = it }, Modifier.weight(1f), hint = tr("ms between requests", "مللي ثانية بين الطلبات"), singleLine = true, mono = true, keyboard = KeyboardOptions(keyboardType = KeyboardType.Number))
            Field(maxGb, { maxGb = it }, Modifier.weight(1f), hint = tr("max GB (0 = none)", "أقصى GB (0 = بلا)"), singleLine = true, mono = true, keyboard = KeyboardOptions(keyboardType = KeyboardType.Number))
        }
        Toggle(allowHttp, { allowHttp = it }, tr("Allow plain http (not recommended)", "السماح بـ http العادي (غير مفضل)"))

        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            GButton(tr("Cancel", "إلغاء"), onDismiss, Modifier.weight(1f))
            GButton(tr("Add source", "إضافة المصدر"), {
                val g = gate!!
                val cfg = JSONObject().put("parallel", parallel.toIntOrNull() ?: 3).put("min_interval_ms", interval.toLongOrNull() ?: 0).put("max_gb", maxGb.toLongOrNull() ?: 0)
                if (kind == "api") {
                    cfg.put("records_path", recordsPath.trim()).put("page_param", pageParam.trim()).put("page_size_param", sizeParam.trim())
                        .put("page_size", pageSize.toIntOrNull() ?: 100).put("cursor_path", cursorPath.trim()).put("cursor_param", cursorParam.trim())
                        .put("max_pages", maxPages.toIntOrNull() ?: 50).put("header_name", headerName.trim()).put("header_prefix", headerPrefix)
                }
                val status = if (g.status == "allow") GATE_ALLOW else GATE_REVIEW_ACK
                val note = (if (g.status == "review") "Reviewed by user: ${ackNote.trim()} (" + g.reasons.joinToString("; ") + ")" else "") +
                    (if (attested) " Owner/permission attested." else "")
                val nm = name.trim().ifEmpty { url.trim().substringAfter("://").substringBefore('/').ifEmpty { "source" } }
                st.work(tr("Adding source", "إضافة مصدر"), { st.repo.addSource(ds.id, kind, url.trim(), nm, g.id, status, note.trim(), expected.trim().lowercase().ifEmpty { null }, cfg) }) { onDismiss() }
            }, Modifier.weight(1f), accent = true, enabled = canAdd)
        }
    }
}

// ------------------------------------------------------------------ dataset intelligence

private fun dateOf(d: JSONObject, fallback: Long): Any {
    for (k in listOf("date", "created_at", "published", "published_at", "timestamp", "time", "created")) {
        val v = d.opt(k)
        if (v is String && v.isNotBlank()) return v
        if (v is Number) return v.toLong()
    }
    return fallback
}

private fun urlOf(d: JSONObject, source: String?): String {
    for (k in listOf("url", "link", "source_url")) d.optString(k).takeIf { it.startsWith("http") }?.let { return it }
    return if (source != null && source.startsWith("http")) source else ""
}

@Composable
fun IntelScreen(st: AppState, datasetId: String) {
    val p = LocalPalette.current
    val ds by ioState<Dataset?>(null, datasetId, st.rev) { st.repo.dataset(datasetId) }
    val d = ds
    if (d == null) { Page("…", onBack = { st.pop() }) {}; return }
    var report by remember { mutableStateOf<JSONObject?>(null) }

    fun analyze() {
        st.work(tr("Analyzing dataset", "تحليل المجموعة"), {
            val h = NativeCore.run("intel_open", JSONObject()).getInt("handle")
            var after = 0L
            var n = 0
            try {
                while (true) {
                    val page = st.repo.pageForIntel(d.id, after, 400)
                    if (page.isEmpty()) break
                    after = page.last().seq
                    val arr = JSONArray()
                    val datas = ArrayList<JSONObject>()
                    for (r in page) { val o = try { JSONObject(r.data) } catch (e: Exception) { JSONObject() }; datas.add(o); arr.put(JSONObject().put("id", r.id).put("data", o)) }
                    val info = NativeCore.run("record_info", JSONObject().put("records", arr)).getJSONArray("items")
                    val items = JSONArray()
                    for (i in page.indices) {
                        val r = page[i]
                        items.put(JSONObject().put("text", info.getJSONObject(i).getString("text")).put("source", r.source ?: "").put("license", r.license ?: "")
                            .put("url", urlOf(datas[i], r.source)).put("date", dateOf(datas[i], r.updated)))
                    }
                    NativeCore.run("intel_add", JSONObject().put("handle", h).put("items", items))
                    n += page.size
                    st.progress("$n records")
                    if (st.cancelFlag.get()) break
                }
            } finally { /* the handle is released by intel_finish below */ }
            NativeCore.run("intel_finish", JSONObject().put("handle", h))
        }) { report = it }
    }

    Page(tr("Dataset Intelligence", "ذكاء المجموعة"), onBack = { st.pop() }) {
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Glass(Modifier.fillMaxWidth(), strength = 0.55f) {
                Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    T(d.name, 16.sp, FontWeight.SemiBold)
                    T(tr("Reads every record in batches (low memory) and measures length, readability, quality, language, source, license, domain, time and duplicates. Quality and language are heuristics, not guarantees.", "يقرأ كل سجل بدفعات (ذاكرة قليلة) ويقيس الطول وسهولة القراءة والجودة واللغة والمصدر والترخيص والنطاق والزمن والتكرار. الجودة واللغة تقديرات وليست ضمانا."), 12.sp, color = p.textDim)
                    GButton(tr("Analyze", "تحليل"), { analyze() }, icon = Ic.PLAY, accent = true)
                }
            }
            report?.let { r ->
                fun num(k: String) = r.optDouble(k, 0.0)
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    StatTile(tr("documents", "وثائق"), "%,d".format(num("documents").toLong()), Modifier.weight(1f))
                    StatTile(tr("words", "كلمات"), "%,d".format(num("words").toLong()), Modifier.weight(1f))
                }
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    StatTile(tr("avg words/doc", "متوسط كلمات/وثيقة"), "%.1f".format(num("avg_words_per_document")), Modifier.weight(1f))
                    StatTile(tr("paragraphs", "فقرات"), "%,d".format(num("paragraphs").toLong()), Modifier.weight(1f))
                }
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    StatTile(tr("duplicates", "مكرر"), "%.1f%%".format(num("duplicate_percent")), Modifier.weight(1f))
                    StatTile(tr("avg quality", "متوسط الجودة"), "%.1f".format(num("avg_quality")), Modifier.weight(1f))
                }
                if (!r.optBoolean("duplicate_tracking_complete", true)) T(tr("Duplicate tracking reached its memory cap; the percentage is a lower bound.", "بلغ تتبع التكرار حد الذاكرة؛ النسبة حد أدنى."), 12.sp, color = p.warn)
                T(tr("Low-quality documents: ", "وثائق ضعيفة الجودة: ") + "%.1f%%".format(num("low_quality_percent")) + "   " + tr("Empty: ", "فارغة: ") + num("empty_documents").toLong(), 13.sp, color = p.textDim)
                HistSection(tr("Document length (words)", "طول الوثيقة (كلمات)"), r.optJSONArray("document_length_words"))
                HistSection(tr("Paragraph length (characters)", "طول الفقرة (حروف)"), r.optJSONArray("paragraph_length_chars"))
                HistSection(tr("Readability (LIX, lower is easier)", "سهولة القراءة (LIX، الأقل أسهل)"), r.optJSONArray("readability_lix"))
                HistSection(tr("Readability (Flesch, higher is easier)", "سهولة القراءة (Flesch، الأعلى أسهل)"), r.optJSONArray("readability_flesch"))
                HistSection(tr("Quality score", "درجة الجودة"), r.optJSONArray("quality"))
                HistSection(tr("Language", "اللغة"), r.optJSONArray("language"))
                HistSection(tr("Source", "المصدر"), r.optJSONArray("source"))
                HistSection(tr("License", "الترخيص"), r.optJSONArray("license"))
                HistSection(tr("Domain", "النطاق"), r.optJSONArray("domain"))
                HistSection(tr("Year", "السنة"), r.optJSONArray("year"))
                HistSection(tr("Month", "الشهر"), r.optJSONArray("month"), 36)
            }
            Box(Modifier.padding(bottom = 60.dp)) {}
        }
    }
}

@Composable
private fun HistSection(title: String, a: JSONArray?, cap: Int = 100) {
    val p = LocalPalette.current
    if (a == null || a.length() == 0) return
    val rows = (0 until a.length()).map { a.getJSONObject(it) }
    val shown = if (rows.size > cap) rows.takeLast(cap) else rows
    val max = shown.maxOfOrNull { it.optLong("count", 0) }?.coerceAtMost(Int.MAX_VALUE.toLong())?.toInt() ?: 0
    if (max == 0) return
    Glass(Modifier.fillMaxWidth(), strength = 0.55f) {
        Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            T(title, 15.sp, FontWeight.SemiBold)
            for (o in shown) BarRow(o.optString("label") + "  (" + "%.1f".format(o.optDouble("pct", 0.0)) + "%)", o.optLong("count", 0).coerceAtMost(Int.MAX_VALUE.toLong()).toInt(), max, p.accent)
        }
    }
}


/** Imports that were interrupted (app closed, crash, cancel, error). Each continues from its last saved batch. */
@Composable
fun JobsCard(st: AppState, ds: Dataset, onReport: (String) -> Unit) {
    val p = LocalPalette.current
    val jobs by ioState(emptyList<com.monex.dataforge.data.ImportJob>(), ds.id, st.rev) { st.repo.jobs(ds.id) }
    var discard by remember { mutableStateOf<com.monex.dataforge.data.ImportJob?>(null) }
    if (jobs.isEmpty()) return
    Glass(Modifier.fillMaxWidth(), strength = 0.7f) {
        Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            T(tr("Unfinished imports", "عمليات استيراد غير مكتملة"), 15.sp, FontWeight.SemiBold)
            T(tr("Progress is saved after every batch. Resume continues where it stopped; nothing is imported twice.", "يحفظ التقدم بعد كل دفعة. الاستئناف يكمل من حيث توقف ولا يستورد شيئا مرتين."), 12.sp, color = p.textDim)
            for (j in jobs) {
                Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        T(j.name, 14.sp, FontWeight.SemiBold, maxLines = 1, modifier = Modifier.weight(1f))
                        Badge(if (j.state == "running") tr("interrupted", "متوقف") else j.state, if (j.state == "failed") p.danger else p.warn)
                    }
                    T(tr("${j.consumed} records read, ${j.imported} imported, ${j.skipped} unchanged, ${j.rejected} rejected", "${j.consumed} سجل مقروء، ${j.imported} مستورد، ${j.skipped} دون تغيير، ${j.rejected} مرفوض") +
                        (if (j.message.isNotBlank()) "\n${j.message}" else ""), 12.sp, color = p.textDim)
                    ChipRow {
                        GButton(tr("Resume", "استئناف"), {
                            st.work(tr("Resuming ${j.name}", "استئناف ${j.name}"), { runJob(st.ctx, st.repo, ds, j, st::progress) { st.cancelFlag.get() } }) { r ->
                                onReport(tr("Resumed: imported ${r.imported} more, rejected ${r.rejected}${if (r.cancelled) " (paused again)" else ""}.", "تم الاستئناف: استورد ${r.imported} إضافية، رُفض ${r.rejected}${if (r.cancelled) " (توقف مجددا)" else ""}."))
                            }
                        }, icon = Ic.PLAY, accent = true, enabled = !ds.locked)
                        GButton(tr("Discard", "تجاهل"), { discard = j }, icon = Ic.TRASH, danger = true)
                    }
                }
            }
        }
    }
    discard?.let { j ->
        ConfirmDialog(tr("Discard this import?", "تجاهل هذا الاستيراد؟"), tr("The rest of the file will not be imported. Records already imported stay.", "لن يستورد باقي الملف. تبقى السجلات المستوردة."),
            tr("Discard", "تجاهل"), true, { st.work(tr("Discarding", "تجاهل"), { discardJob(st.repo, j) }) }, { discard = null })
    }
}
