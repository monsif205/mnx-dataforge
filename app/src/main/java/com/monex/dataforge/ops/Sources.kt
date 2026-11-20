package com.monex.dataforge.ops

import android.content.Context
import com.monex.dataforge.core.Logger
import com.monex.dataforge.core.NativeCore
import com.monex.dataforge.data.Dataset
import com.monex.dataforge.data.Repo
import com.monex.dataforge.data.Source
import com.monex.dataforge.net.DlOptions
import com.monex.dataforge.net.DownloadException
import com.monex.dataforge.net.Downloader
import com.monex.dataforge.net.FetchResult
import com.monex.dataforge.net.Remote
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.net.URL
import java.net.URLEncoder

const val GATE_ALLOW = "allow"
const val GATE_REVIEW_ACK = "review_ack"

data class Gate(val id: String, val status: String, val reasons: List<String>, val commercialOk: Boolean, val attribution: Boolean, val shareAlike: Boolean, val noDerivatives: Boolean)

/** The License Gate. Decided by the Rust engine; "review" always needs an explicit human decision and "deny" blocks the source. */
fun licenseGate(license: String, hints: List<String>, attested: Boolean, allowNonCommercial: Boolean, allowShareAlike: Boolean): Gate {
    val q = JSONObject().put("license", license).put("hints", JSONArray(hints))
        .put("policy", JSONObject().put("attested", attested).put("allow_noncommercial", allowNonCommercial).put("allow_sharealike", allowShareAlike))
    val r = NativeCore.run("license_check", q)
    val rs = r.optJSONArray("reasons")
    return Gate(
        r.getString("id"), r.getString("status"), (0 until (rs?.length() ?: 0)).map { rs!!.getString(it) },
        r.optBoolean("commercial_ok"), r.optBoolean("attribution"), r.optBoolean("share_alike"), r.optBoolean("no_derivatives")
    )
}

fun looksLikeUrl(s: String): Boolean = try { val u = URL(s.trim()); u.protocol == "https" || u.protocol == "http" } catch (e: Exception) { false }

/** Web sources must be https unless the user explicitly accepts plain http. */
fun urlProblem(s: String, allowHttp: Boolean): String? {
    if (!looksLikeUrl(s)) return "not a valid http(s) URL"
    if (s.trim().startsWith("http://") && !allowHttp) return "plain http is not allowed (the data could be altered in transit); use https"
    return null
}

data class FetchOutcome(val imported: Int, val skipped: Int, val rejected: Int, val note: String, val unchanged: Boolean, val errors: List<String>, val sha256: String?)

private fun userAgent() = "MNX-DataForge/1.0"

/** robots.txt: returns null when allowed, or the reason when disallowed. The crawl delay (if any) is applied by the caller. */
private fun robotsCheck(dl: Downloader, url: String): Pair<String?, Long> {
    val u = URL(url)
    val robotsUrl = "${u.protocol}://${u.host}${if (u.port > 0 && u.port != u.defaultPort) ":" + u.port else ""}/robots.txt"
    val (code, body) = try { dl.getText(robotsUrl, 512 * 1024) } catch (e: Exception) { return null to 0L }
    if (code !in 200..299) return null to 0L // no robots.txt: nothing forbids it
    val path = (u.path.ifEmpty { "/" }) + (if (u.query != null) "?" + u.query else "")
    val r = NativeCore.run("robots_check", JSONObject().put("robots", body.toString(Charsets.UTF_8)).put("agent", "MNX-DataForge").put("path", path))
    if (!r.getBoolean("allowed")) return "the site's robots.txt does not allow automated access to this path" to 0L
    val d = r.optDouble("crawl_delay", 0.0)
    return null to (if (d.isNaN()) 0L else (d * 1000).toLong().coerceIn(0, 30_000))
}

private fun isWebPage(remote: Remote, url: String): Boolean {
    val ct = remote.contentType?.lowercase() ?: ""
    if (ct.startsWith("text/html") || ct.contains("xhtml")) return true
    val ext = url.substringBefore('?').substringAfterLast('/', "").substringAfterLast('.', "").lowercase()
    return ext == "html" || ext == "htm"
}

private fun metaFor(remote: Remote, file: File, sha: String): JSONObject {
    val m = JSONObject().put("final_url", remote.url).put("size", file.length()).put("sha256", sha)
        .put("content_type", remote.contentType ?: JSONObject.NULL).put("etag", remote.etag ?: JSONObject.NULL)
        .put("last_modified", remote.lastModified ?: JSONObject.NULL).put("file_name", remote.fileName ?: JSONObject.NULL)
        .put("accept_ranges", remote.acceptRanges)
    for (k in listOf("server", "content-language", "x-license", "link", "x-content-license")) remote.headers[k]?.let { m.put("h_$k", it) }
    try {
        val p = NativeCore.run("probe", JSONObject().put("path", file.path).put("name", remote.fileName ?: file.name))
        m.put("format", p.optString("kind")).put("extension_format", p.optString("extension_format"))
        p.optJSONArray("entries")?.let { m.put("archive_entries", it.length()) }
    } catch (e: Exception) { m.put("probe_error", e.message ?: "") }
    return m
}

/**
 * Fetches a registered source and imports what is new. The License Gate decision stored with the source is
 * re-checked here, so a source that was never cleared cannot be fetched.
 */
fun fetchSource(
    ctx: Context, repo: Repo, ds: Dataset, src: Source, apiKey: String?, forceFull: Boolean,
    progress: (String) -> Unit, cancelled: () -> Boolean
): FetchOutcome {
    if (repo.jobs(ds.id).any { it.sourceId == src.id }) throw IllegalStateException("This source has an unfinished import. Resume or discard it first (see \"Unfinished imports\").")
    val cfg = src.config
    val opt = DlOptions(
        chunkBytes = cfg.optLong("chunk_mb", 8).coerceIn(1, 64) shl 20,
        parallel = cfg.optInt("parallel", 3).coerceIn(1, 8),
        minHostIntervalMs = cfg.optLong("min_interval_ms", 0).coerceIn(0, 60_000),
        maxBytes = cfg.optLong("max_gb", 0).coerceIn(0, 4096) shl 30,
        headers = if (src.kind == "api" && !apiKey.isNullOrBlank() && cfg.optString("header_name").isNotBlank())
            mapOf(cfg.getString("header_name") to cfg.optString("header_prefix") + apiKey) else emptyMap()
    )
    val dl = Downloader(opt)
    val label = src.name.ifBlank { src.url }
    val lim = IngestLimits(maxDecompressedMb = cfg.optLong("max_decompressed_gb", 50).coerceIn(1, 1024) * 1024)
    // A "JSON API" source that actually points at a big file (Parquet, JSONL, archive...) is downloaded as a file.
    var asApi = src.kind == "api"
    if (asApi) {
        val pr = try { dl.probe(src.url) } catch (e: Exception) { null }
        if (pr != null && pr.size > 0 && (pr.size > (24L shl 20) || pr.contentType?.contains("json", true) != true)) {
            Logger.i("sources", "$label: treating the API URL as a file download (${pr.size} bytes, ${pr.contentType})")
            asApi = false
        }
    }
    return if (asApi) fetchApi(ctx, repo, ds, src, dl, label, lim, progress, cancelled)
    else fetchUrl(ctx, repo, ds, src, dl, label, lim, forceFull, opt, progress, cancelled)
}

private fun fetchUrl(
    ctx: Context, repo: Repo, ds: Dataset, src: Source, dl0: Downloader, label: String, lim: IngestLimits, forceFull: Boolean, opt: DlOptions,
    progress: (String) -> Unit, cancelled: () -> Boolean
): FetchOutcome {
    var dl = dl0
    progress("$label: checking the server…")
    val probe = dl.probe(src.url)
    if (isWebPage(probe, src.url)) {
        val (reason, delay) = robotsCheck(dl, src.url)
        if (reason != null) throw IllegalStateException(reason)
        if (delay > opt.minHostIntervalMs) dl = Downloader(opt.copy(minHostIntervalMs = delay))
    }
    val dir = File(ctx.filesDir, "downloads/${src.id}").also { it.mkdirs() }
    val fileName = (probe.fileName ?: "download").replace(Regex("[^A-Za-z0-9._-]"), "_").take(100).ifEmpty { "download" }
    val dest = File(dir, fileName)
    val conditional = !forceFull && src.lastImport > 0
    val res = dl.fetch(
        src.url, dest, if (conditional) src.etag else null, if (conditional) src.lastModified else null,
        { done, total -> progress("$label: downloading ${fmtMb(done)}" + if (total > 0) " of ${fmtMb(total)} (${done * 100 / total}%)" else "") }, cancelled
    )
    when (res) {
        is FetchResult.Unchanged -> {
            repo.touchSource(src.id)
            repo.addSourceVersion(src.id, res.remote.etag ?: src.etag, res.remote.lastModified ?: src.lastModified, src.size, src.sha256, 0, 0, 0, "unchanged: the server reports the same version", null)
            return FetchOutcome(0, 0, 0, "No changes: the server reports the same version.", true, emptyList(), src.sha256)
        }
        is FetchResult.Done -> {
            val f = res.file
            val extra0 = JSONObject().put("etag", res.remote.etag ?: "").put("last_modified", res.remote.lastModified ?: "").put("size", f.length())
                .put("max_decompressed_mb", lim.maxDecompressedMb)
            // The job is saved before the checksum is computed, so even a kill during hashing keeps the finished download.
            val job0 = repo.createJob(ds.id, "file", f.path, fileName, src.license, label, src.id, conditional, extra0)
            progress("$label: verifying checksum…")
            val sha = NativeCore.run("file_hash", JSONObject().put("path", f.path).put("algo", "sha256")).getString("hex")
            val expected = src.expectedSha256?.trim()?.lowercase().orEmpty()
            if (expected.isNotEmpty() && expected != sha) {
                discardJob(repo, job0)
                throw DownloadException("checksum mismatch: expected ${expected.take(16)}…, got ${sha.take(16)}…. The file was discarded.")
            }
            val meta = metaFor(res.remote, f, sha)
            if (conditional && sha == src.sha256) {
                discardJob(repo, job0)
                repo.addSourceVersion(src.id, res.remote.etag, res.remote.lastModified, f.length(), sha, 0, 0, 0, "unchanged: identical content (checksum)", meta)
                return FetchOutcome(0, 0, 0, "No changes: the content is identical (same checksum).", true, emptyList(), sha)
            }
            val note = (if (res.resumedBytes > 0) "resumed (${fmtMb(res.resumedBytes)} reused); " else "") + "checksum ${sha.take(12)}…" + if (conditional) "; incremental" else ""
            repo.setJobExtra(job0.id, JSONObject(extra0.toString()).put("sha256", sha).put("note", note).put("meta", meta))
            val r = runJob(ctx, repo, ds, repo.job(job0.id)!!, progress, cancelled)
            return FetchOutcome(r.imported, r.skipped, r.rejected, if (r.cancelled) "Paused. Resume it from the unfinished imports list." else note, false, r.errors, sha)
        }
    }
    @Suppress("UNREACHABLE_CODE")
    throw IllegalStateException("unreachable")
}

private fun fmtMb(b: Long) = if (b >= 1L shl 30) "%.2f GB".format(java.util.Locale.US, b / 1073741824.0) else "%.1f MB".format(java.util.Locale.US, b / 1048576.0)

private fun jsonPath(root: Any?, path: String): Any? {
    var cur: Any? = root
    if (path.isBlank()) return cur
    for (part in path.split('.').filter { it.isNotEmpty() }) {
        cur = when (cur) {
            is JSONObject -> cur.opt(part)
            is JSONArray -> part.toIntOrNull()?.let { cur.opt(it) }
            else -> null
        } ?: return null
    }
    return cur
}

/**
 * JSON API source: pages are fetched with retry / rate-limit handling, each record is written as one JSON line to a
 * temporary file, and that file goes through the same streaming importer as any other file. The API key is used for
 * this run only and is never stored.
 */
private fun fetchApi(
    ctx: Context, repo: Repo, ds: Dataset, src: Source, dl: Downloader, label: String, lim: IngestLimits,
    progress: (String) -> Unit, cancelled: () -> Boolean
): FetchOutcome {
    val cfg = src.config
    val recordsPath = cfg.optString("records_path")
    val pageParam = cfg.optString("page_param")
    val pageStart = cfg.optInt("page_start", 1)
    val sizeParam = cfg.optString("page_size_param")
    val pageSize = cfg.optInt("page_size", 100).coerceIn(1, 10_000)
    val cursorPath = cfg.optString("cursor_path")
    val cursorParam = cfg.optString("cursor_param")
    val maxPages = cfg.optInt("max_pages", 50).coerceIn(1, 100_000)
    val maxRecords = cfg.optInt("max_records", 100_000).coerceIn(1, 20_000_000)
    val tmp = File(File(ctx.filesDir, "downloads/${src.id}").also { it.mkdirs() }, "api.jsonl")
    var total = 0
    val errors = ArrayList<String>()
    try {
        tmp.bufferedWriter(Charsets.UTF_8).use { w ->
            var page = pageStart
            var cursor: String? = null
            var pages = 0
            while (pages < maxPages && total < maxRecords) {
                if (cancelled()) break
                val sb = StringBuilder(src.url)
                fun add(k: String, v: String) { sb.append(if ('?' in sb) '&' else '?').append(URLEncoder.encode(k, "UTF-8")).append('=').append(URLEncoder.encode(v, "UTF-8")) }
                if (sizeParam.isNotBlank()) add(sizeParam, pageSize.toString())
                if (cursorParam.isNotBlank() && cursor != null) add(cursorParam, cursor)
                else if (pageParam.isNotBlank()) add(pageParam, page.toString())
                progress("$label: API page ${pages + 1} ($total records)")
                val (code, body) = dl.getText(sb.toString(), 16 shl 20, mapOf("Accept" to "application/json"))
                if (code == 401 || code == 403) throw IllegalStateException("The API refused access (HTTP $code). Check the key and that you are allowed to use this API.")
                if (code !in 200..299) throw IllegalStateException("The API answered HTTP $code.")
                val root = try { JSONObject(String(body, Charsets.UTF_8)) } catch (e: Exception) { try { JSONArray(String(body, Charsets.UTF_8)) } catch (e2: Exception) { throw IllegalStateException("The API did not return JSON.") } }
                val list = jsonPath(root, recordsPath)
                val arr = list as? JSONArray ?: throw IllegalStateException("No list of records found at '${recordsPath.ifBlank { "(root)" }}'.")
                if (arr.length() == 0) break
                for (i in 0 until arr.length()) {
                    val o = arr.opt(i)
                    if (o is JSONObject) { w.write(o.toString()); w.write("\n"); total++ }
                    else if (errors.size < 20) errors.add("page ${pages + 1}: a record is not an object")
                    if (total >= maxRecords) break
                }
                pages++; page++
                if (cursorPath.isNotBlank()) {
                    val next = jsonPath(root, cursorPath)
                    cursor = if (next == null || next == JSONObject.NULL || next.toString().isBlank()) break else next.toString()
                } else if (pageParam.isBlank()) break
            }
        }
        if (total == 0) { tmp.delete(); return FetchOutcome(0, 0, 0, "The API returned no records.", true, errors, null) }
        val sha = NativeCore.run("file_hash", JSONObject().put("path", tmp.path)).getString("hex")
        val incremental = src.lastImport > 0
        val meta = JSONObject().put("api_records_fetched", total).put("sha256", sha).put("size", tmp.length())
        val note = "API: $total records fetched" + if (incremental) "; incremental" else ""
        val extra = JSONObject().put("size", tmp.length()).put("sha256", sha).put("note", note).put("meta", meta).put("max_decompressed_mb", lim.maxDecompressedMb)
        val job = repo.createJob(ds.id, "file", tmp.path, "api.jsonl", src.license, label, src.id, incremental, extra)
        val r = runJob(ctx, repo, ds, job, progress, cancelled)
        Logger.i("sources", "$label: $note, imported ${r.imported}, skipped ${r.skipped}")
        return FetchOutcome(r.imported, r.skipped, r.rejected, note, false, errors + r.errors, sha)
    } catch (e: Throwable) {
        // A half-written page file is useless; a finished one is already owned by a saved job.
        if (repo.jobs(ds.id).none { it.location == tmp.path }) tmp.delete()
        throw e
    }
}
