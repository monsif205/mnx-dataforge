package com.monex.dataforge.ops

import android.content.Context
import android.net.Uri
import com.monex.dataforge.core.Logger
import com.monex.dataforge.core.NativeCore
import com.monex.dataforge.data.Dataset
import com.monex.dataforge.data.NewRec
import com.monex.dataforge.data.Repo
import com.monex.dataforge.data.fingerprint
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

/** Limits applied to every parser. The engine enforces them while streaming; none of them loads a whole large file. */
data class IngestLimits(
    val maxLineMb: Int = 16,          // one record (line / row) larger than this is rejected, not loaded
    val maxDecompressedMb: Long = 51_200, // per decompressed stream: stops decompression bombs
    val wholeFileMb: Int = 64,        // formats that need the whole text (HTML page, single JSON document)
    val maxEntries: Int = 100_000,    // archive entries
    val windowMb: Int = 64            // zstd window
)

private const val BATCH_RECORDS = 500
private const val BATCH_MB = 8
private const val MAX_ERRORS_KEPT = 30

fun tmpDir(ctx: Context): File = File(ctx.cacheDir, "mnx-tmp").also { it.mkdirs() }

private fun readerParams(ds: Dataset, name: String, path: String, lim: IngestLimits, ctx: Context): JSONObject {
    val lower = name.lowercase()
    val q = JSONObject().put("path", path).put("name", name).put("tmp_dir", tmpDir(ctx).path)
        .put("limits", JSONObject().put("max_line_mb", lim.maxLineMb).put("max_file_mb", lim.maxDecompressedMb)
            .put("whole_file_mb", lim.wholeFileMb).put("max_entries", lim.maxEntries).put("max_window_mb", lim.windowMb))
    when {
        ds.kind == "conversation" -> q.put("text_mode", "conversation")
        ds.required.size > 1 && ds.required.none { it == "text" } && ds.required.none { it.contains('.') } ->
            q.put("text_mode", "fields").put("fields", JSONArray(ds.required))
        else -> q.put("text_mode", "plain")
    }
    q.put("text_split", if (lower.endsWith(".md") || lower.endsWith(".markdown") || lower.endsWith(".html") || lower.endsWith(".htm")) "paragraphs" else "lines")
    if (lower.endsWith(".tsv")) q.put("csv_delimiter", "\\t")
    return q
}

/**
 * Streams any supported source (JSONL, JSON, CSV, TSV, text, Markdown, HTML, Parquet, and ZIP / TAR / gzip / zstd
 * containers of those) through the Rust reader. A damaged record, row group or archive entry is logged and skipped;
 * the rest of the data is still imported. [skip] holds fingerprints of records already imported from the same source
 * (incremental update).
 */
fun importPath(
    ctx: Context, path: String, name: String, repo: Repo, ds: Dataset, license: String, sourceLabel: String,
    lim: IngestLimits, skip: Set<Long>?, progress: (String) -> Unit, cancelled: () -> Boolean
): ImportResult {
    var imported = 0; var rejected = 0; var skipped = 0
    var errorsTotal = 0
    val errors = ArrayList<String>()
    fun err(s: String) {
        rejected++; errorsTotal++
        if (errors.size < MAX_ERRORS_KEPT) errors.add(s)
        if (errorsTotal <= 200) Logger.w("import", "$sourceLabel: $s")
    }
    val h = NativeCore.run("reader_open", readerParams(ds, name, path, lim, ctx))
    val handle = h.getInt("handle")
    val format = h.optString("format")
    h.optJSONArray("warnings")?.let { w -> for (i in 0 until w.length()) { errors.add(w.getString(i)); Logger.w("import", "$sourceLabel: ${w.getString(i)}") } }
    var wasCancelled = false
    try {
        while (true) {
            val b = NativeCore.run("reader_next", JSONObject().put("handle", handle).put("max_records", BATCH_RECORDS).put("max_mb", BATCH_MB))
            val recs = b.getJSONArray("records")
            val srcs = b.getJSONArray("sources")
            val es = b.getJSONArray("errors")
            for (i in 0 until es.length()) { val e = es.getJSONObject(i); err("${e.getString("where")}: ${e.getString("message")}") }
            val dropped = b.optInt("errors_dropped", 0)
            if (dropped > 0) { rejected += dropped; errorsTotal += dropped }
            val objs = ArrayList<JSONObject>(recs.length())
            val where = ArrayList<String>(recs.length())
            for (i in 0 until recs.length()) {
                val o = recs.optJSONObject(i)
                if (o == null) err("${srcs.optString(i)}: record is not an object") else { objs.add(o); where.add(srcs.optString(i)) }
            }
            val items = ArrayList<NewRec>(objs.size)
            if (ds.kind == "conversation" && objs.isNotEmpty()) {
                val adapted = adaptBatch(objs)
                for ((i, pair) in adapted.withIndex()) {
                    val (o, changed) = pair
                    if (o == null) { err("${where[i]}: record has no messages and is not a known conversation format"); continue }
                    items.add(NewRec(o, if (changed) "transformed" else "imported", sourceLabel, license))
                }
            } else for (o in objs) items.add(NewRec(o, "imported", sourceLabel, license))
            val fresh = if (skip == null) items else items.filter { fingerprint(it.data.toString()) !in skip }
            skipped += items.size - fresh.size
            if (fresh.isNotEmpty()) { repo.insertRecords(ds.id, fresh, history = false); imported += fresh.size }
            val size = b.optLong("file_size", 0); val pos = b.optLong("file_pos", 0)
            progress("$name: $imported imported, $skipped unchanged, $rejected rejected" + if (size > 0) "  (${(pos * 100 / size).coerceIn(0, 100)}%)" else "")
            if (b.getBoolean("done")) break
            if (cancelled()) { wasCancelled = true; break }
        }
    } finally {
        try { NativeCore.run("reader_close", JSONObject().put("handle", handle)) } catch (_: Exception) {}
    }
    repo.logImport(ds.id, sourceLabel, license, imported, rejected)
    if (skipped > 0) repo.logDs(ds.id, "import", "$sourceLabel: $skipped records were already present and were skipped")
    if (errorsTotal > 0) repo.logDs(ds.id, "import-errors", errors.take(10).joinToString(" | ") + if (errorsTotal > 10) " … ($errorsTotal total)" else "")
    return ImportResult(imported, rejected, errors, wasCancelled, skipped, errorsTotal, format)
}

/** Imports a file chosen with the system picker. The engine reads it by file descriptor, so a multi-GB file is never copied or loaded. */
fun importFile(
    ctx: Context, uri: Uri, repo: Repo, ds: Dataset, license: String,
    progress: (String) -> Unit, cancelled: () -> Boolean
): ImportResult {
    val lim = IngestLimits()
    val name = displayName(ctx, uri)
    val pfd = ctx.contentResolver.openFileDescriptor(uri, "r") ?: throw IllegalStateException("Cannot open $name")
    try {
        return try {
            importPath(ctx, "/proc/self/fd/${pfd.fd}", name, repo, ds, license, name, lim, null, progress, cancelled)
        } catch (e: com.monex.dataforge.core.CoreException) {
            // Some providers hand out a pipe instead of a real file. Archives and Parquet need seeking: copy once to cache.
            Logger.w("import", "direct read failed for $name (${e.message}); copying to cache")
            val size = pfd.statSize
            val free = ctx.cacheDir.usableSpace
            if (size > 0 && free < size + (64L shl 20)) throw IllegalStateException("$name cannot be read directly and there is not enough free storage to copy it (${size shr 20} MB needed).")
            val tmp = File(tmpDir(ctx), "copy-" + System.nanoTime() + "-" + name.replace(Regex("[^A-Za-z0-9._-]"), "_"))
            try {
                progress("$name: copying to cache…")
                ctx.contentResolver.openInputStream(uri)!!.use { ins -> tmp.outputStream().buffered(256 * 1024).use { out -> ins.copyTo(out) } }
                importPath(ctx, tmp.path, name, repo, ds, license, name, lim, null, progress, cancelled)
            } finally { tmp.delete() }
        }
    } finally {
        try { pfd.close() } catch (_: Exception) {}
        tmpDir(ctx).listFiles()?.forEach { if (it.isFile && it.name.startsWith("copy-")) it.delete() }
    }
}
