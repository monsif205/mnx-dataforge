package com.monex.dataforge.ops

import com.monex.dataforge.core.NativeCore
import com.monex.dataforge.data.Dataset
import com.monex.dataforge.data.Repo
import org.json.JSONArray
import org.json.JSONObject

data class CleanOptions(
    val pii: List<String> = emptyList(), val boilerplate: Boolean = false, val repeatLines: Boolean = false,
    val minChars: Int = 0, val minQuality: Int = 0, val maxRepeat: Double = 0.0, val lang: String = ""
) {
    fun toJson(): JSONObject = JSONObject().put("pii", JSONArray(pii)).put("boilerplate", boilerplate).put("repeat_lines", repeatLines)
        .put("min_chars", minChars).put("min_quality", minQuality).put("max_repeat", maxRepeat).put("lang", lang)
    val isEmpty get() = pii.isEmpty() && !boilerplate && !repeatLines && minChars == 0 && minQuality == 0 && maxRepeat == 0.0 && lang.isEmpty()
}

data class CleanReport(val examined: Int, val changed: Int, val dropped: Int, val counts: JSONObject, val samples: List<String>, val applied: Boolean, val cancelled: Boolean)

/**
 * Dry run (apply = false) or real run. The Rust engine decides per record: keep, change (masked / trimmed) or drop (with reasons).
 * A real run first stores a version snapshot; every changed record keeps its previous text in its history, every deleted record keeps its final content.
 */
fun cleanDataset(repo: Repo, ds: Dataset, o: CleanOptions, apply: Boolean, progress: (String) -> Unit, cancelled: () -> Boolean): CleanReport {
    if (o.isEmpty) throw IllegalStateException("Pick at least one cleanup option.")
    if (apply && ds.locked) throw IllegalStateException("Dataset is locked (protection mode). Unlock it first.")
    if (apply) repo.snapshot(ds.id, "before cleanup")
    var examined = 0; var changed = 0; var dropped = 0
    val totals = HashMap<String, Int>()
    val samples = ArrayList<String>()
    val buf = ArrayList<com.monex.dataforge.data.Rec>()
    var stopped = false
    fun flush() {
        if (buf.isEmpty()) return
        val arr = JSONArray()
        buf.forEach { arr.put(runCatching { JSONObject(it.data) }.getOrDefault(JSONObject())) }
        val res = NativeCore.run("cleanse", JSONObject().put("records", arr).put("options", o.toJson()))
        val rs = res.getJSONArray("results")
        val drop = ArrayList<String>()
        for (i in 0 until rs.length()) {
            val r = rs.getJSONObject(i); val rec = buf[i]
            when (r.getString("action")) {
                "change" -> { changed++; if (apply) repo.updateRecord(rec.id, r.getJSONObject("record")) }
                "drop" -> {
                    dropped++; drop.add(rec.id)
                    if (samples.size < 8) samples.add("${rec.id.take(8)}: ${r.getJSONArray("reasons").join("; ").replace("\"", "")}")
                }
            }
        }
        if (apply) repo.deleteRecords(drop)
        val c = res.getJSONObject("counts")
        c.keys().forEach { k -> totals[k] = (totals[k] ?: 0) + c.getInt(k) }
        examined += buf.size
        buf.clear()
        progress("${if (apply) "cleaned" else "checked"} $examined records")
    }
    repo.pageAll(ds.id) { r ->
        if (stopped) return@pageAll
        if (cancelled()) { stopped = true; return@pageAll }
        buf.add(r)
        if (buf.size >= 200) flush()
    }
    if (!stopped) flush()
    return CleanReport(examined, changed, dropped, JSONObject(totals as Map<*, *>), samples, apply, stopped)
}
