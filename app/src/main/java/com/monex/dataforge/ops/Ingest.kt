package com.monex.dataforge.ops

import android.content.Context
import android.content.Intent
import android.net.Uri
import com.monex.dataforge.core.CoreException
import com.monex.dataforge.core.Logger
import com.monex.dataforge.core.NativeCore
import com.monex.dataforge.data.Dataset
import com.monex.dataforge.data.ImportJob
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
    val wholeFileMb: Int = 64,        // formats that need the whole text (HTML page, DOCX text, single JSON document)
    val maxEntries: Int = 100_000,    // archive entries
    val windowMb: Int = 64            // zstd window
)

private const val BATCH_RECORDS = 500
private const val BATCH_MB = 8
private const val MAX_ERRORS_KEPT = 30

fun tmpDir(ctx: Context): File = File(ctx.cacheDir, "mnx-tmp").also { it.mkdirs() }

private fun readerParams(ds: Dataset, name: String, path: String, lim: IngestLimits, ctx: Context, skipRecords: Long): JSONObject {
    val lower = name.lowercase()
    val q = JSONObject().put("path", path).put("name", name).put("tmp_dir", tmpDir(ctx).path).put("skip_records", skipRecords)
        .put("limits", JSONObject().put("max_line_mb", lim.maxLineMb).put("max_file_mb", lim.maxDecompressedMb)
            .put("whole_file_mb", lim.wholeFileMb).put("max_entries", lim.maxEntries).put("max_window_mb", lim.windowMb))
    when {
        ds.kind == "conversation" -> q.put("text_mode", "conversation")
        ds.required.size > 1 && ds.required.none { it == "text" } && ds.required.none { it.contains('.') } ->
            q.put("text_mode", "fields").put("fields", JSONArray(ds.required))
        else -> q.put("text_mode", "plain")
    }
    val paragraphs = listOf(".md", ".markdown", ".html", ".htm", ".docx").any { lower.endsWith(it) }
    q.put("text_split", if (paragraphs) "paragraphs" else "lines")
    if (lower.endsWith(".tsv")) q.put("csv_delimiter", "\\t")
    return q
}

/**
 * Streams one source (JSONL, JSON, CSV, TSV, text, Markdown, HTML, DOCX, Parquet, and ZIP / TAR / gzip / zstd containers of
 * those) through the Rust reader. A damaged record, row group or archive entry is logged and skipped; the rest is still
 * imported. Progress is saved in the same database transaction as every batch, so after a crash, a kill or a reinstall of
 * the same data the job continues from the last saved batch ([job].consumed records are skipped by the engine).
 */
fun importPath(
    ctx: Context, path: String, name: String, repo: Repo, ds: Dataset, job: ImportJob, lim: IngestLimits,
    progress: (String) -> Unit, cancelled: () -> Boolean
): ImportResult {
    val sourceLabel = job.label
    var imported = job.imported; var rejected = job.rejected; var skipped = job.skipped
    var errorsTotal = 0
    val errors = ArrayList<String>()
    val resuming = job.consumed > 0
    val skip: Set<Long>? = if (job.incremental) repo.recordFingerprints(ds.id, sourceLabel) else null
    fun err(s: String) {
        rejected++; errorsTotal++
        if (errors.size < MAX_ERRORS_KEPT) errors.add(s)
        if (errorsTotal <= 200) Logger.w("import", "$sourceLabel: $s")
    }
    val h = NativeCore.run("reader_open", readerParams(ds, name, path, lim, ctx, job.consumed))
    val handle = h.getInt("handle")
    val format = h.optString("format")
    h.optJSONArray("warnings")?.let { w -> for (i in 0 until w.length()) { errors.add(w.getString(i)); Logger.w("import", "$sourceLabel: ${w.getString(i)}") } }
    if (resuming) Logger.i("import", "$sourceLabel: resuming after ${job.consumed} records")
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
                    items.add(NewRec(o, if (changed) "transformed" else "imported", sourceLabel, job.license))
                }
            } else for (o in objs) items.add(NewRec(o, "imported", sourceLabel, job.license))
            val fresh = if (skip == null) items else items.filter { fingerprint(it.data.toString()) !in skip }
            skipped += items.size - fresh.size
            val consumed = b.optLong("total_records", job.consumed)
            // The batch and the progress marker are committed together: a crash can never import a batch twice or lose one.
            repo.tx {
                if (fresh.isNotEmpty()) { repo.insertRecords(ds.id, fresh, history = false); imported += fresh.size }
                repo.saveJobProgress(job.id, consumed, imported, skipped, rejected)
            }
            val size = b.optLong("file_size", 0); val pos = b.optLong("file_pos", 0)
            val pct = if (size > 0) "  (${(pos * 100 / size).coerceIn(0, 100)}%)" else ""
            progress(if (b.getJSONArray("records").length() == 0 && !b.getBoolean("done") && consumed < job.consumed) "$name: resuming, skipping what was already imported…$pct"
                else "$name: $imported imported, $skipped unchanged, $rejected rejected$pct")
            if (b.getBoolean("done")) break
            if (cancelled()) { wasCancelled = true; break }
        }
    } finally {
        try { NativeCore.run("reader_close", JSONObject().put("handle", handle)) } catch (_: Exception) {}
    }
    repo.logImport(ds.id, sourceLabel, job.license, imported - job.imported, rejected - job.rejected)
    if (skipped > job.skipped) repo.logDs(ds.id, "import", "$sourceLabel: ${skipped - job.skipped} records were already present and were skipped")
    if (errorsTotal > 0) repo.logDs(ds.id, "import-errors", errors.take(10).joinToString(" | ") + if (errorsTotal > 10) " … ($errorsTotal total)" else "")
    return ImportResult(imported - job.imported, rejected - job.rejected, errors, wasCancelled, skipped - job.skipped, errorsTotal, format)
}

private fun openUri(ctx: Context, uri: Uri) = ctx.contentResolver.openFileDescriptor(uri, "r") ?: throw IllegalStateException("Cannot open $uri")

private fun importUri(ctx: Context, uri: Uri, repo: Repo, ds: Dataset, job: ImportJob, progress: (String) -> Unit, cancelled: () -> Boolean): ImportResult {
    val lim = IngestLimits()
    val name = job.name
    val pfd = openUri(ctx, uri)
    try {
        return try {
            importPath(ctx, "/proc/self/fd/${pfd.fd}", name, repo, ds, job, lim, progress, cancelled)
        } catch (e: CoreException) {
            // Some providers hand out a pipe instead of a real file. Archives and Parquet need seeking: copy once to cache.
            Logger.w("import", "direct read failed for $name (${e.message}); copying to cache")
            val size = pfd.statSize
            if (size > 0 && ctx.cacheDir.usableSpace < size + (64L shl 20)) throw IllegalStateException("$name cannot be read directly and there is not enough free storage to copy it (${size shr 20} MB needed).")
            val tmp = File(tmpDir(ctx), "copy-" + System.nanoTime() + "-" + name.replace(Regex("[^A-Za-z0-9._-]"), "_"))
            try {
                progress("$name: copying to cache…")
                ctx.contentResolver.openInputStream(uri)!!.use { ins -> tmp.outputStream().buffered(256 * 1024).use { out -> ins.copyTo(out) } }
                importPath(ctx, tmp.path, name, repo, ds, job, lim, progress, cancelled)
            } finally { tmp.delete() }
        }
    } finally {
        try { pfd.close() } catch (_: Exception) {}
    }
}

/**
 * Runs (or resumes) an import job. The job row is the single source of truth: it is removed only when the import
 * completed; a cancelled job stays "paused", a failed one stays "failed", and both can be resumed later.
 */
fun runJob(ctx: Context, repo: Repo, ds: Dataset, job: ImportJob, progress: (String) -> Unit, cancelled: () -> Boolean): ImportResult {
    repo.setJobState(job.id, "running", "")
    val r = try {
        if (job.kind == "uri") importUri(ctx, Uri.parse(job.location), repo, ds, job, progress, cancelled)
        else {
            val f = File(job.location)
            if (!f.exists()) throw IllegalStateException("The saved file for this import is gone (${job.name}). Discard the job and start it again.")
            importPath(ctx, f.path, job.name, repo, ds, job, IngestLimits(maxDecompressedMb = job.extra.optLong("max_decompressed_mb", 51_200)), progress, cancelled)
        }
    } catch (e: Throwable) {
        repo.setJobState(job.id, "failed", e.message ?: e.toString())
        throw e
    }
    val now = repo.job(job.id)
    if (r.cancelled) {
        repo.setJobState(job.id, "paused", "paused by you at ${now?.consumed ?: 0} records")
        return r
    }
    // Finished: record the source version (if this job came from a registered source), then remove the job and its file.
    val x = job.extra
    if (job.sourceId != null) {
        repo.addSourceVersion(
            job.sourceId, x.optString("etag").ifEmpty { null }, x.optString("last_modified").ifEmpty { null }, x.optLong("size"),
            x.optString("sha256").ifEmpty { null }, (now?.imported ?: r.imported), (now?.skipped ?: r.skipped), (now?.rejected ?: r.rejected), x.optString("note"), x.optJSONObject("meta")
        )
    }
    repo.deleteJob(job.id)
    if (job.kind == "file") try { File(job.location).delete(); File(job.location).parentFile?.takeIf { it.listFiles().isNullOrEmpty() }?.delete() } catch (_: Exception) {}
    return r
}

/** Discards a job without importing the rest. Records already imported stay in the dataset. */
fun discardJob(repo: Repo, job: ImportJob) {
    if (job.kind == "file") try { File(job.location).delete() } catch (_: Exception) {}
    repo.deleteJob(job.id)
}

/** Imports a file chosen with the system picker. The engine reads it by file descriptor, so a multi-GB file is never copied or loaded. */
fun importFile(
    ctx: Context, uri: Uri, repo: Repo, ds: Dataset, license: String,
    progress: (String) -> Unit, cancelled: () -> Boolean
): ImportResult {
    val name = displayName(ctx, uri)
    // Keep read access across app restarts so an interrupted import can continue without picking the file again.
    try { ctx.contentResolver.takePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION) } catch (_: Exception) {}
    val job = repo.createJob(ds.id, "uri", uri.toString(), name, license, name, null, false, JSONObject())
    return runJob(ctx, repo, ds, job, progress, cancelled)
}
