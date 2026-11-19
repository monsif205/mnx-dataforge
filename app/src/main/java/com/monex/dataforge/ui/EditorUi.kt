package com.monex.dataforge.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.monex.dataforge.core.NativeCore
import com.monex.dataforge.data.Dataset
import com.monex.dataforge.data.HistoryItem
import com.monex.dataforge.data.NewRec
import com.monex.dataforge.data.Rec
import com.monex.dataforge.ops.datasetValidationKind
import kotlinx.coroutines.delay
import org.json.JSONArray
import org.json.JSONObject

class UndoStack(initial: String) {
    private val items = arrayListOf(initial)
    private var idx = 0
    fun commit(s: String) {
        if (s == items[idx]) return
        while (items.size > idx + 1) items.removeAt(items.size - 1)
        items.add(s); idx++
        if (items.size > 200) { items.removeAt(0); idx-- }
    }
    fun canUndo() = idx > 0
    fun canRedo() = idx < items.size - 1
    fun undo(): String { idx--; return items[idx] }
    fun redo(): String { idx++; return items[idx] }
}

private val ROLES = listOf("system", "user", "assistant", "tool")

/** All structural message edits run in the Rust engine (op "msg_edit"). Returns the original doc if the engine rejects the edit. */
private fun msgOp(doc: String, action: String, index: Int = -1, extra: JSONObject.() -> Unit = {}): String =
    runCatching {
        val q = JSONObject().put("doc", doc).put("action", action)
        if (index >= 0) q.put("index", index)
        q.extra()
        com.monex.dataforge.core.NativeCore.run("msg_edit", q).getString("doc")
    }.getOrDefault(doc)

private fun starterDoc(ds: Dataset): String =
    if (isConversationKind(ds.kind))
        com.monex.dataforge.core.NativeCore.pretty("""{"messages":[{"role":"user","content":""},{"role":"assistant","content":""}]}""")
    else com.monex.dataforge.core.NativeCore.pretty(kindInfo(ds.kind).template.replace(Regex("\\{\\{\\w+\\}\\}"), ""))

@Composable
fun RecordEditor(st: AppState, dsId: String, recId: String?) {
    val loaded by ioState<Pair<Dataset?, Rec?>?>(null, dsId, recId) { st.repo.dataset(dsId) to (recId?.let { st.repo.record(it) }) }
    val l = loaded
    if (l == null || l.first == null) { Page("…", onBack = { st.pop() }) {}; return }
    EditorBody(st, l.first!!, l.second)
}

@Composable
private fun EditorBody(st: AppState, ds: Dataset, rec: Rec?) {
    val p = LocalPalette.current
    val draftKey = "draft:${ds.id}:${rec?.id ?: "new"}"
    val saved0 = remember(rec?.id) { rec?.let { runCatching { com.monex.dataforge.core.NativeCore.pretty(it.data) }.getOrNull() } ?: starterDoc(ds) }
    val draft0 = remember(rec?.id) { st.prefs.getString(draftKey, null) }
    var savedDoc by remember(rec?.id) { mutableStateOf(saved0) }
    var doc by remember(rec?.id) { mutableStateOf(draft0 ?: saved0) }
    val recovered = remember(rec?.id) { draft0 != null && draft0 != saved0 }
    var label by remember(rec?.id) { mutableStateOf(rec?.label ?: "") }
    var group by remember(rec?.id) { mutableStateOf(rec?.grp ?: "") }
    var source by remember(rec?.id) { mutableStateOf(rec?.source ?: "") }
    var license by remember(rec?.id) { mutableStateOf(rec?.license ?: "") }
    var metaDirty by remember(rec?.id) { mutableStateOf(false) }
    var raw by remember(rec?.id) { mutableStateOf(!isConversationKind(ds.kind)) }
    var confirmLeave by remember { mutableStateOf(false) }
    val undo = remember(rec?.id) { UndoStack(doc) }
    var tick by remember { mutableIntStateOf(0) }
    val history by ioState(emptyList<HistoryItem>(), rec?.id, st.rev) { rec?.let { st.repo.history(it.id) } ?: emptyList() }

    LaunchedEffect(doc) {
        delay(600); undo.commit(doc); tick++
    }
    LaunchedEffect(doc) {
        delay(300)
        st.prefs.edit().apply { if (doc != savedDoc) putString(draftKey, doc) else remove(draftKey) }.apply()
    }

    val dirty = doc != savedDoc || metaDirty
    BackHandler(enabled = dirty) { confirmLeave = true }

    // Strict JSON check by the Rust parser (org.json is lenient).
    val parseErr: String? = remember(doc) {
        try {
            val r = NativeCore.run("parse_json", JSONObject().put("text", doc)).getJSONArray("records")
            if (r.optJSONObject(0) == null) "The record must be a JSON object" else null
        } catch (e: Exception) { e.message ?: "invalid JSON" }
    }
    val vkind = datasetValidationKind(ds.kind)
    val report: JSONObject? = remember(doc, ds.id) {
        if (parseErr != null) null else try {
            val cfg = JSONObject().put("kind", vkind).put("required", JSONArray(ds.required)).put("dedup", false).put("near_threshold", 0)
            NativeCore.run("validate", JSONObject().put("config", cfg).put("records", JSONArray().put(JSONObject().put("id", "this").put("data", JSONObject(doc)))))
        } catch (e: Exception) { null }
    }

    fun save() {
        val obj = JSONObject(doc)
        val lb = label; val gp = group; val sc = source; val lc = license
        st.work(tr("Saving", "حفظ"), {
            if (rec == null) st.repo.insertRecords(ds.id, listOf(NewRec(obj, "human", sc.ifBlank { null }, lc.ifBlank { null }, lb.ifBlank { null }, gp.ifBlank { null })))[0]
            else { st.repo.updateRecord(rec.id, obj); st.repo.updateMeta(rec.id, lb, gp, sc, lc); rec.id }
        }) { id ->
            savedDoc = doc; metaDirty = false
            st.prefs.edit().remove(draftKey).apply()
            if (rec == null) { st.stack = st.stack.dropLast(1) + Screen.EditorS(ds.id, id) }
            st.toast(tr("Saved", "تم الحفظ"))
        }
    }

    Page(
        if (rec == null) tr("New record", "سجل جديد") else tr("Record", "سجل"),
        onBack = { if (dirty) confirmLeave = true else st.pop() },
        actions = {
            IconBtn(Ic.UNDO, { doc = undo.undo(); tick++ }, enabled = tick >= 0 && undo.canUndo())
            IconBtn(Ic.REDO, { doc = undo.redo(); tick++ }, enabled = tick >= 0 && undo.canRedo())
            GButton("", { save() }, icon = Ic.SAVE, accent = true, enabled = dirty && parseErr == null && !ds.locked)
        }
    ) {
        Column(Modifier.weight(1f).imePadding().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                Badge(if (dirty) tr("unsaved", "غير محفوظ") else tr("saved", "محفوظ"), if (dirty) p.warn else p.ok)
                if (ds.locked) Badge(tr("locked", "مقفل"), p.warn)
                rec?.let { Badge(it.origin, if (it.origin == "ai_generated") p.warn else p.accent); Badge(it.split, p.textDim) }
            }
            if (recovered) {
                Glass(Modifier.fillMaxWidth(), strength = 0.7f, radius = 16.dp) {
                    Row(Modifier.padding(12.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        T(tr("Unsaved draft recovered from your last session.", "تمت استعادة مسودة غير محفوظة من جلستك السابقة."), 13.sp, modifier = Modifier.weight(1f))
                        GButton(tr("Discard", "تجاهل"), { doc = savedDoc }, danger = true)
                    }
                }
            }
            if (isConversationKind(ds.kind)) ChipRow {
                Chip(tr("Structured", "منظم"), !raw) { raw = false }
                Chip("JSON", raw) { raw = true }
            }
            if (raw || !isConversationKind(ds.kind)) {
                Field(doc, { doc = it }, Modifier.fillMaxWidth(), minHeight = 220.dp, mono = true, enabled = !ds.locked)
            } else {
                ConversationBody(doc, { doc = it }, ds.locked)
            }
            parseErr?.let { T(it, 13.sp, FontWeight.SemiBold, p.danger) }
            if (report != null) {
                val fs = report.getJSONArray("findings")
                if (fs.length() == 0) T(tr("No findings.", "لا توجد ملاحظات."), 13.sp, color = p.ok)
                for (i in 0 until fs.length()) {
                    val f = fs.getJSONObject(i)
                    Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.Top) {
                        Badge(f.getString("rule"), if (f.getString("severity") == "error") p.danger else p.warn)
                        T(f.getString("message"), 13.sp, modifier = Modifier.weight(1f))
                    }
                }
                T(tr("Findings are warnings only; your text is never changed automatically.", "الملاحظات تحذيرات فقط؛ لا يتغير نصك تلقائيا."), 11.sp, color = p.textDim)
            }
            Glass(Modifier.fillMaxWidth(), strength = 0.55f) {
                Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    T(tr("Metadata and provenance", "البيانات الوصفية والمصدر"), 14.sp, FontWeight.SemiBold)
                    Field(label, { label = it; metaDirty = true }, Modifier.fillMaxWidth(), hint = tr("Label / quality label", "وسم / وسم جودة"), singleLine = true)
                    Field(group, { group = it; metaDirty = true }, Modifier.fillMaxWidth(), hint = tr("Group (kept together when splitting)", "مجموعة (تبقى معا عند التقسيم)"), singleLine = true)
                    Field(source, { source = it; metaDirty = true }, Modifier.fillMaxWidth(), hint = tr("Source", "المصدر"), singleLine = true)
                    Field(license, { license = it; metaDirty = true }, Modifier.fillMaxWidth(), hint = tr("License", "الترخيص"), singleLine = true)
                    rec?.let { T(tr("Origin: ${it.origin} · updated ${fmtTime(it.updated)}", "المصدر: ${it.origin} · آخر تحديث ${fmtTime(it.updated)}"), 12.sp, color = p.textDim) }
                }
            }
            if (rec != null) {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    GButton(tr("Duplicate / branch", "نسخ / تفريع"), {
                        val obj = JSONObject(doc)
                        st.work(tr("Duplicating", "نسخ"), { st.repo.insertRecords(ds.id, listOf(NewRec(obj, rec.origin, rec.source, rec.license, rec.label, rec.grp))) }) { st.toast(tr("Copy created", "تم إنشاء نسخة")) }
                    }, icon = Ic.COPY, enabled = parseErr == null && !ds.locked)
                }
                if (history.isNotEmpty()) {
                    Glass(Modifier.fillMaxWidth(), strength = 0.5f) {
                        Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                            T(tr("History", "التاريخ"), 14.sp, FontWeight.SemiBold)
                            history.forEach { h ->
                                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                    Column(Modifier.weight(1f)) {
                                        T("${h.op} · ${fmtTime(h.ts)}", 12.sp, FontWeight.SemiBold)
                                        T(h.detail, 11.sp, color = p.textDim)
                                    }
                                    if (h.hash != null) GButton(tr("Load", "تحميل"), {
                                        st.work("…", { st.repo.blob(h.hash) }) { b -> if (b != null) doc = com.monex.dataforge.core.NativeCore.pretty(b) }
                                    })
                                }
                            }
                        }
                    }
                }
            }
            Box(Modifier.padding(bottom = 60.dp)) {}
        }
    }
    if (confirmLeave) ConfirmDialog(
        tr("Discard unsaved changes?", "تجاهل التعديلات غير المحفوظة؟"),
        tr("Your edits are kept as a draft and offered again next time you open this record.", "تحفظ تعديلاتك كمسودة وتعرض عليك عند فتح السجل مرة أخرى."),
        tr("Leave", "مغادرة"), false, { st.pop() }, { confirmLeave = false })
}

@Composable
private fun ConversationBody(doc: String, onDoc: (String) -> Unit, locked: Boolean) {
    val p = LocalPalette.current
    val ctx = androidx.compose.ui.platform.LocalContext.current
    var attachTo by remember { mutableIntStateOf(-1) }
    val attachPick = androidx.activity.compose.rememberLauncherForActivityResult(androidx.activity.result.contract.ActivityResultContracts.OpenDocument()) { uri: android.net.Uri? ->
        val target = attachTo
        attachTo = -1
        if (uri != null && target >= 0) {
            val nm = ctx.contentResolver.query(uri, arrayOf(android.provider.OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c -> if (c.moveToFirst()) c.getString(0) else null } ?: "file.txt"
            val mime = ctx.contentResolver.getType(uri) ?: "text/plain"
            val text = runCatching { ctx.contentResolver.openInputStream(uri)?.use { it.readNBytes(2 * 1024 * 1024) }?.toString(Charsets.UTF_8) }.getOrNull()
            if (text != null) onDoc(msgOp(doc, "attach_add", target) { put("name", nm); put("mime", mime); put("content", text) })
        }
    }
    val msgs: List<JSONObject>? = remember(doc) {
        try {
            val a = JSONObject(doc).optJSONArray("messages")
            if (a == null) null else (0 until a.length()).map { a.optJSONObject(it) ?: JSONObject() }
        } catch (e: Exception) { null }
    }
    if (msgs == null) {
        T(tr("This record has no \"messages\" array. Switch to JSON to fix it, or add a message below.", "هذا السجل بلا مصفوفة \"messages\". انتقل إلى JSON لإصلاحه أو أضف رسالة."), 13.sp, color = p.warn)
        if (!locked) GButton(tr("Create messages array", "إنشاء مصفوفة الرسائل"), { onDoc(msgOp(doc, "ensure")) })
        return
    }
    msgs.forEachIndexed { i, m ->
        val role = m.optString("role")
        Glass(Modifier.fillMaxWidth(), strength = 0.6f, radius = 18.dp) {
            Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                ChipRow {
                    (ROLES + listOfNotNull(role.takeIf { it.isNotEmpty() && it !in ROLES })).forEach { r ->
                        Chip(r, role == r) { if (!locked) onDoc(msgOp(doc, "role", i) { put("role", r) }) }
                    }
                }
                Field(m.optString("content"), { nc -> onDoc(msgOp(doc, "content", i) { put("content", nc) }) }, Modifier.fillMaxWidth(), hint = tr("Message ${i + 1}", "الرسالة ${i + 1}"), minHeight = 70.dp, enabled = !locked)
                if (role == "assistant") {
                    if (m.has("thinking")) {
                        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            T(tr("Thinking", "التفكير"), 12.sp, FontWeight.SemiBold, p.accent, modifier = Modifier.weight(1f))
                            IconBtn(Ic.TRASH, { onDoc(msgOp(doc, "thinking_remove", i)) }, enabled = !locked, tint = p.danger)
                        }
                        Field(m.optString("thinking"), { nc -> onDoc(msgOp(doc, "thinking", i) { put("content", nc) }) }, Modifier.fillMaxWidth(), hint = tr("The assistant's reasoning before it answers", "استدلال المساعد قبل الإجابة"), minHeight = 70.dp, enabled = !locked)
                    } else if (!locked) Chip(tr("+ Thinking", "+ تفكير"), false) { onDoc(msgOp(doc, "thinking", i) { put("content", "") }) }
                }
                if (role == "assistant") {
                    val atts = m.optJSONArray("attachments")
                    if (atts != null) for (a in 0 until atts.length()) {
                        val at = atts.optJSONObject(a) ?: continue
                        Glass(Modifier.fillMaxWidth(), strength = 0.4f, radius = 14.dp) {
                            Column(Modifier.padding(10.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                                    Box(Modifier.weight(1f)) { Field(at.optString("name"), { nc -> onDoc(msgOp(doc, "attach_edit", i) { put("attachment", a); put("name", nc) }) }, Modifier.fillMaxWidth(), hint = tr("File name", "اسم الملف"), singleLine = true, mono = true, enabled = !locked) }
                                    IconBtn(Ic.TRASH, { onDoc(msgOp(doc, "attach_remove", i) { put("attachment", a) }) }, enabled = !locked, tint = p.danger)
                                }
                                Field(at.optString("content"), { nc -> onDoc(msgOp(doc, "attach_edit", i) { put("attachment", a); put("content", nc) }) }, Modifier.fillMaxWidth(), hint = tr("File content (CSV table, JSON, text…)", "محتوى الملف (جدول CSV أو JSON أو نص…)"), minHeight = 70.dp, mono = true, enabled = !locked)
                            }
                        }
                    }
                    if (!locked) ChipRow {
                        Chip(tr("+ File", "+ ملف"), false) { onDoc(msgOp(doc, "attach_add", i) { put("name", "table.csv"); put("mime", "text/csv"); put("content", "") }) }
                        Chip(tr("+ Load file", "+ تحميل ملف"), false) { attachTo = i; attachPick.launch(arrayOf("*/*")) }
                    }
                }
                if (m.has("tool_calls") || m.has("metadata") || m.has("annotations")) T(tr("Has extra fields (edit them in JSON view).", "فيها حقول إضافية (حررها في عرض JSON)."), 11.sp, color = p.textDim)
                Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    IconBtn(Ic.UP, { onDoc(msgOp(doc, "up", i)) }, enabled = i > 0 && !locked)
                    IconBtn(Ic.DOWN, { onDoc(msgOp(doc, "down", i)) }, enabled = i < msgs.size - 1 && !locked)
                    IconBtn(Ic.COPY, { onDoc(msgOp(doc, "dup", i)) }, enabled = !locked)
                    IconBtn(Ic.SCISSORS, { onDoc(msgOp(doc, "split", i)) }, enabled = !locked && m.optString("content").contains(Regex("\n\\s*\n")))
                    IconBtn(Ic.MERGE, { onDoc(msgOp(doc, "merge", i)) }, enabled = i < msgs.size - 1 && !locked)
                    IconBtn(Ic.TRASH, { onDoc(msgOp(doc, "delete", i)) }, enabled = !locked, tint = p.danger)
                }
            }
        }
    }
    if (!locked) ChipRow {
        Chip(tr("+ turn", "+ دور"), true) { onDoc(msgOp(doc, "turns") { put("count", 1) }) }
        Chip(tr("+ 5 turns", "+ 5 أدوار"), false) { onDoc(msgOp(doc, "turns") { put("count", 5) }) }
        Chip(tr("+ 10 turns", "+ 10 أدوار"), false) { onDoc(msgOp(doc, "turns") { put("count", 10) }) }
        ROLES.forEach { r -> Chip("+ $r", false) { onDoc(msgOp(doc, "add") { put("role", r) }) } }
    }
}
