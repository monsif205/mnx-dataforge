package com.monex.dataforge.ops

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import com.monex.dataforge.core.NativeCore
import com.monex.dataforge.data.Dataset
import com.monex.dataforge.data.NewRec
import com.monex.dataforge.data.Rec
import com.monex.dataforge.data.Repo
import com.monex.dataforge.data.sha256Hex
import org.json.JSONArray
import org.json.JSONObject
import java.io.BufferedReader
import java.io.BufferedWriter
import java.io.InputStreamReader
import java.io.OutputStreamWriter
import java.security.DigestOutputStream
import java.security.MessageDigest

const val MAX_TEXT_IMPORT_BYTES = 64L * 1024 * 1024
const val DEDUP_MAX_RECORDS = 50_000

fun displayName(ctx: Context, uri: Uri): String {
    ctx.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
        if (c.moveToFirst()) return c.getString(0) ?: "file"
    }
    return uri.lastPathSegment ?: "file"
}

fun datasetValidationKind(kind: String): String = when (kind) {
    "conversation", "tool_use" -> "conversation"
    "qa" -> "qa"
    else -> "generic"
}

// ---------------------------------------------------------------- import

data class ImportResult(val imported: Int, val rejected: Int, val errors: List<String>, val cancelled: Boolean)

/** Maps common formats (ShareGPT, Alpaca, prompt/completion) to {"messages":[...]}. Returns null if not convertible. */
fun adaptToConversation(o: JSONObject): JSONObject? {
    if (o.has("messages")) return o
    if (o.has("conversations")) {
        val conv = o.optJSONArray("conversations") ?: return null
        val msgs = JSONArray()
        for (i in 0 until conv.length()) {
            val m = conv.optJSONObject(i) ?: return null
            val role = when (m.optString("from")) {
                "human", "user" -> "user"
                "gpt", "assistant", "model" -> "assistant"
                "system" -> "system"
                "tool", "function" -> "tool"
                else -> return null
            }
            msgs.put(JSONObject().put("role", role).put("content", m.optString("value")))
        }
        return JSONObject().put("messages", msgs)
    }
    val user = when {
        o.has("instruction") -> o.optString("instruction") + (o.optString("input").takeIf { it.isNotBlank() }?.let { "\n\n$it" } ?: "")
        o.has("prompt") -> o.optString("prompt")
        else -> return null
    }
    val answer = when {
        o.has("output") -> o.optString("output")
        o.has("completion") -> o.optString("completion")
        o.has("response") -> o.optString("response")
        else -> return null
    }
    val msgs = JSONArray()
    if (o.has("system") && o.optString("system").isNotBlank()) msgs.put(JSONObject().put("role", "system").put("content", o.optString("system")))
    msgs.put(JSONObject().put("role", "user").put("content", user))
    msgs.put(JSONObject().put("role", "assistant").put("content", answer))
    return JSONObject().put("messages", msgs)
}

fun importFile(
    ctx: Context, uri: Uri, repo: Repo, ds: Dataset, license: String,
    progress: (String) -> Unit, cancelled: () -> Boolean
): ImportResult {
    val name = displayName(ctx, uri)
    val lower = name.lowercase()
    var imported = 0
    var rejected = 0
    val errors = ArrayList<String>()
    fun err(s: String) { rejected++; if (errors.size < 20) errors.add(s) }

    fun accept(batch: List<JSONObject>) {
        val items = ArrayList<NewRec>()
        for (o in batch) {
            if (ds.kind == "conversation") {
                val adapted = adaptToConversation(o)
                if (adapted == null) { err("record has no messages and is not a known conversation format"); continue }
                val changed = adapted !== o
                items.add(NewRec(adapted, if (changed) "transformed" else "imported", name, license))
            } else {
                items.add(NewRec(o, "imported", name, license))
            }
        }
        if (items.isNotEmpty()) { repo.insertRecords(ds.id, items, history = false); imported += items.size }
    }

    fun objectsOnly(values: JSONArray, lineOf: (Int) -> String): List<JSONObject> {
        val out = ArrayList<JSONObject>()
        for (i in 0 until values.length()) {
            val o = values.optJSONObject(i)
            if (o == null) err("${lineOf(i)}: record is not a JSON object") else out.add(o)
        }
        return out
    }

    var wasCancelled = false
    when {
        lower.endsWith(".jsonl") || lower.endsWith(".ndjson") -> {
            ctx.contentResolver.openInputStream(uri)!!.use { ins ->
                val br = BufferedReader(InputStreamReader(ins, Charsets.UTF_8))
                var lineNo = 0
                val chunk = StringBuilder()
                var chunkStart = 1
                var inChunk = 0
                fun flush() {
                    if (inChunk == 0) return
                    val res = NativeCore.run("parse_jsonl", JSONObject().put("text", chunk.toString()))
                    val errs = res.getJSONArray("errors")
                    for (i in 0 until errs.length()) {
                        val e = errs.getJSONObject(i)
                        err("line ${chunkStart + e.getInt("line") - 1}: ${e.getString("message")}")
                    }
                    accept(objectsOnly(res.getJSONArray("records")) { "record $it" })
                    chunk.setLength(0); inChunk = 0
                    progress("$name: $imported imported, $rejected rejected")
                }
                while (true) {
                    val line = br.readLine() ?: break
                    lineNo++
                    if (inChunk == 0) chunkStart = lineNo
                    chunk.append(line).append('\n'); inChunk++
                    if (inChunk >= 500) { flush(); if (cancelled()) { wasCancelled = true; break } }
                }
                if (!wasCancelled) flush()
            }
        }
        lower.endsWith(".json") || lower.endsWith(".csv") || lower.endsWith(".tsv") || lower.endsWith(".txt") -> {
            val size = ctx.contentResolver.openFileDescriptor(uri, "r")?.use { it.statSize } ?: 0L
            if (size > MAX_TEXT_IMPORT_BYTES) throw IllegalStateException("$name is larger than 64 MB. Convert it to JSONL, which is imported as a stream.")
            val text = ctx.contentResolver.openInputStream(uri)!!.use { it.readBytes().toString(Charsets.UTF_8) }
            val records: List<JSONObject> = when {
                lower.endsWith(".json") -> {
                    val res = NativeCore.run("parse_json", JSONObject().put("text", text))
                    objectsOnly(res.getJSONArray("records")) { "item $it" }
                }
                lower.endsWith(".csv") || lower.endsWith(".tsv") -> {
                    val res = NativeCore.run("parse_csv", JSONObject().put("text", text).put("delimiter", if (lower.endsWith(".tsv")) "\t" else ","))
                    res.optJSONArray("errors")?.let { e -> for (i in 0 until e.length()) err("line ${e.getJSONObject(i).getInt("line")}: ${e.getJSONObject(i).getString("message")}") }
                    objectsOnly(res.getJSONArray("records")) { "row $it" }
                }
                else -> text.lines().filter { it.isNotBlank() }.map { JSONObject().put("text", it) }
            }
            var i = 0
            while (i < records.size) {
                accept(records.subList(i, minOf(i + 500, records.size)))
                i += 500
                progress("$name: $imported imported, $rejected rejected")
                if (cancelled()) { wasCancelled = true; break }
            }
        }
        else -> throw IllegalStateException("Unsupported file type: $name (use .jsonl, .json, .csv, .tsv or .txt)")
    }
    repo.logImport(ds.id, name, license, imported, rejected)
    return ImportResult(imported, rejected, errors, wasCancelled)
}

// ---------------------------------------------------------------- export

data class ExportOptions(
    val container: String,        // jsonl | json | csv | txt | md
    val profile: String,          // messages | sharegpt | alpaca | chatml | raw
    val split: String?,           // null = all
    val includeMetadata: Boolean,
    val roleMap: Map<String, String> = emptyMap()
)

data class ExportResult(val written: Int, val errors: List<String>, val sha256: String, val verified: Boolean, val note: String)

private fun metaOf(r: Rec): JSONObject = JSONObject().put("id", r.id).put("origin", r.origin).put("source", r.source ?: JSONObject.NULL)
    .put("license", r.license ?: JSONObject.NULL).put("split", r.split).put("label", r.label ?: JSONObject.NULL)

private fun profileJson(o: ExportOptions): JSONObject {
    val rm = JSONObject()
    o.roleMap.forEach { (k, v) -> if (v.isNotBlank()) rm.put(k, v) }
    return JSONObject().put("format", o.profile).put("include_metadata", o.includeMetadata).put("role_map", rm)
}

/** Converted JSON lines for one chunk, plus per-record errors. */
private fun convertChunk(rows: List<Rec>, o: ExportOptions): Pair<List<String>, List<String>> {
    val arr = JSONArray()
    rows.forEach { arr.put(JSONObject().put("id", it.id).put("data", JSONObject(it.data)).put("meta", metaOf(it))) }
    val res = NativeCore.run("export", JSONObject().put("profile", profileJson(o)).put("records", arr))
    val lines = res.getJSONArray("lines").let { a -> (0 until a.length()).map { a.getString(it) } }
    val errs = res.getJSONArray("errors").let { a -> (0 until a.length()).map { a.getJSONObject(it).getString("id") + ": " + a.getJSONObject(it).getString("message") } }
    return lines to errs
}

private fun plainText(r: Rec, markdown: Boolean, n: Int): String {
    val d = JSONObject(r.data)
    val sb = StringBuilder()
    if (markdown) sb.append("## Record $n\n\n") else sb.append("--- Record $n ---\n")
    val msgs = d.optJSONArray("messages")
    if (msgs != null) {
        for (i in 0 until msgs.length()) {
            val m = msgs.optJSONObject(i) ?: continue
            val role = m.optString("role"); val c = m.optString("content")
            if (markdown) sb.append("**").append(role).append("**: ").append(c).append("\n\n") else sb.append('[').append(role).append("] ").append(c).append("\n")
        }
    } else {
        val keys = d.keys()
        while (keys.hasNext()) {
            val k = keys.next()
            if (markdown) sb.append("**").append(k).append("**: ") else sb.append(k).append(": ")
            sb.append(d.opt(k).toString()).append(if (markdown) "\n\n" else "\n")
        }
    }
    sb.append('\n')
    return sb.toString()
}

fun exportDataset(
    ctx: Context, uri: Uri, repo: Repo, ds: Dataset, o: ExportOptions,
    progress: (String) -> Unit, cancelled: () -> Boolean
): ExportResult {
    val md = MessageDigest.getInstance("SHA-256")
    val rawOut = ctx.contentResolver.openOutputStream(uri, "wt") ?: throw IllegalStateException("Cannot open the destination file")
    val errors = ArrayList<String>()
    var written = 0
    var lineCount = 0
    fun addErr(e: String) { if (errors.size < 50) errors.add(e) }

    // CSV needs the full column set before the first row, so it takes a first pass over the data.
    var columns: List<String> = emptyList()
    if (o.container == "csv") {
        val cols = LinkedHashSet<String>()
        val buf = ArrayList<Rec>()
        fun passFlush() {
            if (buf.isEmpty()) return
            val (lines, _) = convertChunk(buf, o)
            lines.forEach { l -> val j = JSONObject(l); val it = j.keys(); while (it.hasNext()) cols.add(it.next()) }
            buf.clear()
        }
        repo.pageAll(ds.id, o.split) { r -> buf.add(r); if (buf.size >= 500) passFlush() }
        passFlush()
        columns = cols.toList()
    }

    BufferedWriter(OutputStreamWriter(DigestOutputStream(rawOut, md), Charsets.UTF_8), 64 * 1024).use { w ->
        fun line(s: String) { w.write(s); w.write("\n"); lineCount++ }
        if (o.container == "json") line("[")
        if (o.container == "csv") {
            val head = NativeCore.run("to_csv", JSONObject().put("records", JSONArray()).put("columns", JSONArray(columns))).getString("text")
            w.write(head)
        }
        val buf = ArrayList<Rec>()
        var first = true
        var n = 0
        fun flush() {
            if (buf.isEmpty()) return
            when (o.container) {
                "txt", "md" -> buf.forEach { r -> n++; w.write(plainText(r, o.container == "md", n)); written++ }
                else -> {
                    val (lines, errs) = convertChunk(buf, o)
                    errs.forEach { addErr(it) }
                    when (o.container) {
                        "jsonl" -> lines.forEach { line(it); written++ }
                        "json" -> lines.forEach { l -> line((if (first) "" else ",") + l); first = false; written++ }
                        "csv" -> {
                            val recs = JSONArray(); lines.forEach { recs.put(JSONObject(it)) }
                            val t = NativeCore.run("to_csv", JSONObject().put("records", recs).put("columns", JSONArray(columns))).getString("text")
                            // drop the header line that to_csv always writes
                            w.write(t.substring(t.indexOf("\r\n") + 2)); written += lines.size
                        }
                    }
                }
            }
            buf.clear()
            progress("exported $written")
        }
        repo.pageAll(ds.id, o.split) { r ->
            if (!cancelled()) { buf.add(r); if (buf.size >= 500) flush() }
        }
        flush()
        if (o.container == "json") line("]")
    }
    val writerHash = md.digest().joinToString("") { "%02x".format(it) }

    // Integrity check: read the file back and compare bytes + line count.
    var verified = false
    var note = ""
    try {
        val md2 = MessageDigest.getInstance("SHA-256")
        var lines = 0
        ctx.contentResolver.openInputStream(uri)!!.use { ins ->
            val buf = ByteArray(64 * 1024)
            while (true) {
                val k = ins.read(buf); if (k < 0) break
                md2.update(buf, 0, k)
                for (i in 0 until k) if (buf[i] == '\n'.code.toByte()) lines++
            }
        }
        val readHash = md2.digest().joinToString("") { "%02x".format(it) }
        verified = readHash == writerHash && (o.container !in listOf("jsonl", "json") || lines == lineCount)
        note = if (verified) "re-read matches (sha256 + line count)" else "re-read MISMATCH"
    } catch (e: Exception) {
        note = "could not re-read destination: ${e.message}"
    }
    return ExportResult(written, errors, writerHash, verified, note)
}

// ---------------------------------------------------------------- validation

data class ValidateOpts(val minChars: Int, val maxChars: Int, val nearThreshold: Double)

fun validateDataset(repo: Repo, ds: Dataset, o: ValidateOpts, progress: (String) -> Unit, cancelled: () -> Boolean): JSONObject {
    val t0 = System.currentTimeMillis()
    val cfg = JSONObject().put("kind", datasetValidationKind(ds.kind)).put("required", JSONArray(ds.required))
        .put("min_chars", o.minChars).put("max_chars", o.maxChars).put("near_threshold", o.nearThreshold)
    var errors = 0; var warnings = 0; var infos = 0; var examined = 0
    var truncated = false
    val findings = JSONArray()
    val ruleCounts = LinkedHashMap<String, Int>()
    val ruleMeta = LinkedHashMap<String, JSONObject>()
    val chunkIds = StringBuilder()
    val cap = 5000

    fun addFinding(f: JSONObject) {
        if (findings.length() < cap) findings.put(f) else truncated = true
    }
    fun count(rule: String, severity: String) {
        ruleCounts[rule] = (ruleCounts[rule] ?: 0) + 1
        when (severity) { "error" -> errors++; "warning" -> warnings++; else -> infos++ }
    }

    val buf = ArrayList<Rec>()
    fun flush() {
        if (buf.isEmpty()) return
        val arr = JSONArray()
        buf.forEach { r -> arr.put(JSONObject().put("id", r.id).put("data", runCatching { JSONObject(r.data) }.getOrNull() ?: r.data)) }
        val c = JSONObject(cfg.toString()).put("dedup", false).put("max_findings", cap)
        val rep = NativeCore.run("validate", JSONObject().put("config", c).put("records", arr))
        examined += rep.getInt("records_examined")
        chunkIds.append(rep.getString("validation_id"))
        val rules = rep.getJSONArray("rules")
        for (i in 0 until rules.length()) {
            val r = rules.getJSONObject(i)
            ruleMeta[r.getString("id")] = r
            ruleCounts[r.getString("id")] = (ruleCounts[r.getString("id")] ?: 0) + r.getInt("findings")
        }
        val fs = rep.getJSONArray("findings")
        for (i in 0 until fs.length()) addFinding(fs.getJSONObject(i))
        if (rep.getBoolean("truncated")) truncated = true
        errors += rep.getInt("errors"); warnings += rep.getInt("warnings"); infos += rep.getInt("infos")
        buf.clear()
        progress("validated $examined records")
    }
    repo.pageAll(ds.id) { r -> if (!cancelled()) { buf.add(r); if (buf.size >= 1000) flush() } }
    flush()

    // Dataset-wide duplicate detection (texts only, bounded).
    var dedupScope = 0
    if (!cancelled()) {
        progress("detecting duplicates")
        val texts = repo.texts(ds.id, DEDUP_MAX_RECORDS, 2000)
        dedupScope = texts.size
        val items = JSONArray()
        texts.forEach { items.put(JSONObject().put("id", it.first).put("text", it.second)) }
        val res = NativeCore.run("dedup", JSONObject().put("items", items).put("threshold", o.nearThreshold))
        val groups = res.getJSONArray("exact_groups")
        for (g in 0 until groups.length()) {
            val ids = groups.getJSONArray(g)
            for (k in 1 until ids.length()) {
                count("R004", "warning")
                addFinding(JSONObject().put("record", ids.getString(k)).put("rule", "R004").put("severity", "warning").put("deterministic", true)
                    .put("message", "exact duplicate of record ${ids.getString(0)}"))
            }
        }
        val near = res.getJSONArray("near_pairs")
        for (i in 0 until near.length()) {
            val p = near.getJSONObject(i)
            count("R005", "warning")
            addFinding(JSONObject().put("record", p.getString("b")).put("rule", "R005").put("severity", "warning").put("deterministic", false)
                .put("message", "near duplicate of record ${p.getString("a")} (similarity ${"%.2f".format(p.getDouble("similarity"))}, heuristic)"))
        }
        chunkIds.append("dedup:").append(groups.length()).append(':').append(near.length())
    }

    // Dataset-level heuristics and provenance checks.
    val labels = repo.groupCounts(ds.id, "label").filterKeys { it != "(none)" }
    if (labels.size >= 2 && labels.values.max() > 10 * labels.values.min()) {
        count("D001", "warning")
        addFinding(JSONObject().put("record", "*").put("rule", "D001").put("severity", "warning").put("deterministic", false)
            .put("message", "label imbalance: largest class has more than 10x the records of the smallest (heuristic)"))
    }
    val prov = repo.provenanceSummary(ds.id)
    if (prov.getInt("unknown_license") > 0) {
        count("D002", "warning")
        addFinding(JSONObject().put("record", "*").put("rule", "D002").put("severity", "warning").put("deterministic", true)
            .put("message", "${prov.getInt("unknown_license")} imported/transformed records have an unknown license"))
    }
    if (prov.getInt("missing_source") > 0) {
        count("D003", "warning")
        addFinding(JSONObject().put("record", "*").put("rule", "D003").put("severity", "warning").put("deterministic", true)
            .put("message", "${prov.getInt("missing_source")} imported/transformed records have no source"))
    }
    ruleMeta["R004"] = JSONObject().put("id", "R004").put("deterministic", true).put("description", "exact duplicate (normalized text)")
    ruleMeta["R005"] = JSONObject().put("id", "R005").put("deterministic", false).put("description", "near duplicate (shingle similarity)")
    ruleMeta["D001"] = JSONObject().put("id", "D001").put("deterministic", false).put("description", "category balance")
    ruleMeta["D002"] = JSONObject().put("id", "D002").put("deterministic", true).put("description", "license known for imported records")
    ruleMeta["D003"] = JSONObject().put("id", "D003").put("deterministic", true).put("description", "source known for imported records")

    val rulesArr = JSONArray()
    ruleMeta.values.forEach { rulesArr.put(JSONObject(it.toString()).put("findings", ruleCounts[it.getString("id")] ?: 0)) }
    val failed = ruleMeta.keys.count { (ruleCounts[it] ?: 0) > 0 }
    val id = "val-" + sha256Hex(cfg.toString() + chunkIds).take(16)
    return JSONObject()
        .put("validation_id", id).put("records_examined", examined)
        .put("errors", errors).put("warnings", warnings).put("infos", infos)
        .put("rules_executed", ruleMeta.size).put("rules_failed", failed).put("rules_passed", ruleMeta.size - failed)
        .put("rules", rulesArr).put("findings", findings).put("truncated", truncated)
        .put("dedup_scope", dedupScope).put("dedup_limit", DEDUP_MAX_RECORDS)
        .put("duration_ms", System.currentTimeMillis() - t0).put("config", cfg).put("cancelled", cancelled())
}

// ---------------------------------------------------------------- splits & leakage

data class SplitPlan(val counts: JSONObject, val assign: Map<String, String>, val evalMoves: Int, val units: Int)

fun planSplit(
    repo: Repo, ds: Dataset, ratios: LinkedHashMap<String, Double>, seed: Long,
    stratify: Boolean, useGroups: Boolean, dupAware: Boolean, threshold: Double
): SplitPlan {
    val texts: Map<String, String> = if (dupAware) repo.texts(ds.id, DEDUP_MAX_RECORDS, 2000).toMap() else emptyMap()
    val recs = JSONArray()
    val current = HashMap<String, String>()
    repo.pageAll(ds.id) { r ->
        current[r.id] = r.split
        val o = JSONObject().put("id", r.id)
        if (stratify && !r.label.isNullOrEmpty()) o.put("label", r.label)
        if (useGroups && !r.grp.isNullOrEmpty()) o.put("group", r.grp)
        if (dupAware) o.put("text", texts[r.id] ?: "")
        recs.put(o)
    }
    val rj = JSONObject()
    ratios.forEach { (k, v) -> rj.put(k, v) }
    val res = NativeCore.run(
        "split",
        JSONObject().put("records", recs).put("ratios", rj).put("seed", seed).put("dup_aware", dupAware).put("near_threshold", threshold)
    )
    val a = res.getJSONArray("assignments")
    val assign = LinkedHashMap<String, String>()
    var evalMoves = 0
    for (i in 0 until a.length()) {
        val x = a.getJSONObject(i)
        val id = x.getString("id"); val sp = x.getString("split")
        assign[id] = sp
        val cur = current[id]
        if ((cur == "test" || cur == "validation") && cur != sp) evalMoves++
    }
    return SplitPlan(res.getJSONObject("counts"), assign, evalMoves, res.getInt("units"))
}

fun leakageCheck(repo: Repo, ds: Dataset, threshold: Double): JSONObject {
    val texts = repo.texts(ds.id, DEDUP_MAX_RECORDS, 2000).toMap()
    val recs = JSONArray()
    repo.pageAll(ds.id) { r ->
        if (texts.containsKey(r.id)) recs.put(JSONObject().put("id", r.id).put("split", r.split).put("text", texts[r.id]).put("source", r.source ?: JSONObject.NULL))
    }
    return NativeCore.run("leakage", JSONObject().put("records", recs).put("threshold", threshold).put("eval_splits", JSONArray(listOf("validation", "test"))))
}

// ---------------------------------------------------------------- template batch creation

/** Cartesian expansion of {{var}} placeholders into JSON records, with preview/limit and duplicate prevention. */
fun expandTemplate(template: String, vars: Map<String, List<String>>, limit: Int): Pair<List<JSONObject>, List<String>> {
    val names = vars.keys.toList()
    val problems = ArrayList<String>()
    val out = ArrayList<JSONObject>()
    val seen = HashSet<String>()
    var combos = 1L
    names.forEach { combos *= vars[it]!!.size.coerceAtLeast(1) }
    if (combos > 1_000_000) { problems.add("too many combinations ($combos); reduce variable values"); return out to problems }
    val idx = IntArray(names.size)
    var produced = 0L
    while (produced < combos && out.size < limit) {
        var t = template
        names.forEachIndexed { i, n ->
            val v = vars[n]!!.getOrElse(idx[i]) { "" }
            t = t.replace("{{${n}}}", JSONObject.quote(v).let { it.substring(1, it.length - 1) })
        }
        produced++
        try {
            val o = JSONObject(t)
            val key = sha256Hex(o.toString())
            if (seen.add(key)) out.add(o)
        } catch (e: Exception) {
            if (problems.size < 5) problems.add("combination $produced is not valid JSON: ${e.message}")
        }
        var k = names.size - 1
        while (k >= 0) {
            idx[k]++
            if (idx[k] < vars[names[k]]!!.size) break
            idx[k] = 0; k--
        }
        if (k < 0) break
    }
    if (out.size >= limit && produced < combos) problems.add("limited to $limit records (of $combos combinations)")
    return out to problems
}
