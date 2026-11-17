package com.monex.dataforge.net

import com.monex.dataforge.core.Logger
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.IOException
import java.io.InputStream
import java.io.RandomAccessFile
import java.net.HttpURLConnection
import java.net.SocketTimeoutException
import java.net.URL
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong

/** What the server told us about a file before (or while) downloading it. */
data class Remote(
    val url: String,            // final URL after redirects
    val size: Long,             // -1 when unknown
    val acceptRanges: Boolean,
    val etag: String?,
    val lastModified: String?,
    val contentType: String?,
    val fileName: String?,
    val headers: Map<String, String>
)

class DownloadException(message: String, val retryable: Boolean = false) : IOException(message)

sealed class FetchResult {
    class Unchanged(val remote: Remote) : FetchResult()
    class Done(val file: File, val remote: Remote, val resumedBytes: Long) : FetchResult()
}

data class DlOptions(
    val chunkBytes: Long = 8L shl 20,
    val parallel: Int = 3,
    val maxRetries: Int = 6,
    val connectTimeoutMs: Int = 20_000,
    val readTimeoutMs: Int = 30_000,
    val minHostIntervalMs: Long = 0,           // polite delay between requests to one host
    val headers: Map<String, String> = emptyMap(),
    val maxBytes: Long = 0,                    // 0 = no limit (the free-space check still applies)
    val userAgent: String = "MNX-DataForge/1.0 (dataset tool; contact via app settings)"
)

/**
 * Resumable, chunked, parallel HTTP downloader.
 *
 * - The target file is preallocated; a small sidecar (".part.json") records which chunks are complete, so after a
 *   network cut or an app restart only the missing chunks are fetched again.
 * - Servers without Range support are downloaded sequentially (and restarted from zero if interrupted).
 * - Retries use exponential backoff with jitter and honour Retry-After on 429/503.
 * - A per-host gate spaces requests when the source asks for it.
 * - If the remote ETag/Last-Modified changes between attempts the partial file is discarded, never mixed.
 *
 * Downloads run while the app is open. Android may stop a background app; the next start resumes from the sidecar.
 */
class Downloader(private val opt: DlOptions = DlOptions()) {

    companion object {
        private val hostGate = HashMap<String, Long>()
        private fun gate(host: String, minMs: Long) {
            if (minMs <= 0) return
            val wait: Long
            synchronized(hostGate) {
                val now = System.currentTimeMillis()
                val next = maxOf(now, hostGate[host] ?: 0L)
                hostGate[host] = next + minMs
                wait = next - now
            }
            if (wait > 0) Thread.sleep(wait)
        }

        fun parseRetryAfter(v: String?): Long? {
            if (v == null) return null
            v.trim().toLongOrNull()?.let { return (it * 1000).coerceIn(0, 300_000) }
            return try {
                val t = java.text.SimpleDateFormat("EEE, dd MMM yyyy HH:mm:ss zzz", java.util.Locale.US).parse(v.trim())?.time ?: return null
                (t - System.currentTimeMillis()).coerceIn(0, 300_000)
            } catch (e: Exception) { null }
        }

        fun backoffMs(attempt: Int): Long {
            val base = minOf(60_000L, 800L shl minOf(attempt, 6))
            return base / 2 + (Math.random() * base / 2).toLong()
        }

        fun fileNameFrom(disposition: String?, url: String): String? {
            if (disposition != null) {
                Regex("filename\\*=(?:UTF-8'')?([^;]+)", RegexOption.IGNORE_CASE).find(disposition)?.let {
                    return try { java.net.URLDecoder.decode(it.groupValues[1].trim().trim('"'), "UTF-8") } catch (e: Exception) { it.groupValues[1].trim('"') }
                }
                Regex("filename=\"?([^\";]+)\"?", RegexOption.IGNORE_CASE).find(disposition)?.let { return it.groupValues[1].trim() }
            }
            val path = url.substringBefore('#').substringBefore('?').trimEnd('/')
            return path.substringAfterLast('/').takeIf { it.isNotEmpty() && '.' in it }
        }
    }

    private fun open(url: String, method: String, extra: Map<String, String>): HttpURLConnection {
        var u = url
        // Follow redirects ourselves so that https -> http downgrades are refused and the final URL is known.
        for (hop in 0 until 8) {
            val c = URL(u).openConnection() as HttpURLConnection
            c.instanceFollowRedirects = false
            c.requestMethod = method
            c.connectTimeout = opt.connectTimeoutMs
            c.readTimeout = opt.readTimeoutMs
            c.setRequestProperty("User-Agent", opt.userAgent)
            c.setRequestProperty("Accept-Encoding", "identity")
            opt.headers.forEach { (k, v) -> c.setRequestProperty(k, v) }
            extra.forEach { (k, v) -> c.setRequestProperty(k, v) }
            val code = c.responseCode
            if (code in 301..308 && code != 304 && code != 305 && code != 306) {
                val loc = c.getHeaderField("Location") ?: throw DownloadException("redirect without a location")
                val next = URL(URL(u), loc).toString()
                c.disconnect()
                if (u.startsWith("https://") && next.startsWith("http://")) throw DownloadException("refused a redirect from https to http")
                u = next
                continue
            }
            return c
        }
        throw DownloadException("too many redirects")
    }

    private fun remoteOf(c: HttpURLConnection, url: String): Remote {
        val h = LinkedHashMap<String, String>()
        for ((k, v) in c.headerFields) if (k != null && v != null && v.isNotEmpty()) h[k.lowercase()] = v.last()
        val range = h["content-range"]?.substringAfter('/')?.toLongOrNull()
        val len = range ?: h["content-length"]?.toLongOrNull() ?: -1L
        return Remote(
            c.url.toString().ifEmpty { url }, len,
            (h["accept-ranges"]?.contains("bytes", true) == true) || h.containsKey("content-range"),
            h["etag"], h["last-modified"], h["content-type"], fileNameFrom(h["content-disposition"], c.url.toString()), h
        )
    }

    /** Looks at the server without downloading the body. Uses HEAD, and falls back to a 1-byte range request. */
    fun probe(url: String): Remote {
        gate(URL(url).host, opt.minHostIntervalMs)
        var c: HttpURLConnection? = null
        try {
            c = open(url, "HEAD", emptyMap())
            if (c.responseCode in 200..299) return remoteOf(c, url)
        } catch (e: IOException) {
            Logger.w("net", "HEAD failed for $url: ${e.message}")
        } finally { c?.disconnect() }
        c = open(url, "GET", mapOf("Range" to "bytes=0-0"))
        try {
            if (c.responseCode !in 200..299) throw DownloadException("the server answered HTTP ${c.responseCode}")
            return remoteOf(c, url)
        } finally { try { c.inputStream.close() } catch (_: Exception) {}; c.disconnect() }
    }

    /** Small response body (robots.txt, API pages). Capped so that a wrong URL cannot fill memory. */
    fun getText(url: String, maxBytes: Int = 8 shl 20, extra: Map<String, String> = emptyMap()): Pair<Int, ByteArray> {
        gate(URL(url).host, opt.minHostIntervalMs)
        var attempt = 0
        while (true) {
            val c = open(url, "GET", extra)
            try {
                val code = c.responseCode
                if (code == 429 || code == 503) {
                    if (attempt >= opt.maxRetries) throw DownloadException("the server is rate limiting (HTTP $code)")
                    val w = parseRetryAfter(c.getHeaderField("Retry-After")) ?: backoffMs(attempt)
                    Logger.w("net", "HTTP $code on $url, waiting ${w}ms")
                    attempt++; Thread.sleep(w); continue
                }
                val ins = if (code in 200..299) c.inputStream else (c.errorStream ?: return code to ByteArray(0))
                val out = java.io.ByteArrayOutputStream()
                val buf = ByteArray(16384)
                while (true) {
                    val n = ins.read(buf); if (n < 0) break
                    out.write(buf, 0, n)
                    if (out.size() > maxBytes) throw DownloadException("the response is larger than ${maxBytes.coerceAtLeast(1) / 1024} KB; it is a file, not a small API reply")
                }
                return code to out.toByteArray()
            } catch (e: SocketTimeoutException) {
                if (attempt++ >= opt.maxRetries) throw DownloadException("timed out")
                Thread.sleep(backoffMs(attempt))
            } finally { c.disconnect() }
        }
    }

    // ------------------------------------------------------------ sidecar state

    private class State(val url: String, val size: Long, val chunk: Long, val etag: String?, val lastModified: String?, val done: BooleanArray) {
        fun toJson(): String = JSONObject().put("url", url).put("size", size).put("chunk", chunk)
            .put("etag", etag ?: JSONObject.NULL).put("last_modified", lastModified ?: JSONObject.NULL)
            .put("done", JSONArray().also { a -> done.forEachIndexed { i, b -> if (b) a.put(i) } }).toString()
    }

    private fun loadState(f: File, url: String, r: Remote, chunks: Int): State? = try {
        val j = JSONObject(f.readText())
        val ok = j.getString("url") == url && j.getLong("size") == r.size && j.getLong("chunk") == opt.chunkBytes &&
            (j.opt("etag") as? String) == r.etag && (j.opt("last_modified") as? String) == r.lastModified
        if (!ok) null else {
            val d = BooleanArray(chunks)
            val a = j.getJSONArray("done")
            for (i in 0 until a.length()) { val ix = a.getInt(i); if (ix in 0 until chunks) d[ix] = true }
            State(url, r.size, opt.chunkBytes, r.etag, r.lastModified, d)
        }
    } catch (e: Exception) { null }

    // ------------------------------------------------------------ download

    /**
     * Downloads [url] into [dest] (a ".part" file is used until the download is complete). If [ifEtag] or
     * [ifModified] still match, nothing is downloaded and [FetchResult.Unchanged] is returned.
     * [progress] receives (bytes done, total or -1). [cancelled] is polled often.
     */
    fun fetch(
        url: String, dest: File, ifEtag: String?, ifModified: String?,
        progress: (Long, Long) -> Unit, cancelled: () -> Boolean
    ): FetchResult {
        val remote = probe(url)
        if (ifEtag != null && remote.etag != null && ifEtag == remote.etag) return FetchResult.Unchanged(remote)
        if (ifEtag == null && ifModified != null && remote.lastModified != null && ifModified == remote.lastModified) return FetchResult.Unchanged(remote)
        // Very large files use bigger chunks so the progress sidecar and thread work stay small (100 GB = 1600 chunks of 64 MB).
        val eff = when { remote.size > (20L shl 30) -> 64L shl 20; remote.size > (2L shl 30) -> 32L shl 20; else -> opt.chunkBytes }
        if (eff > opt.chunkBytes) return Downloader(opt.copy(chunkBytes = eff)).fetch(url, dest, ifEtag, ifModified, progress, cancelled)
        if (opt.maxBytes > 0 && remote.size > opt.maxBytes) throw DownloadException("the file is ${remote.size shr 20} MB, above the ${opt.maxBytes shr 20} MB limit you set")
        dest.parentFile?.mkdirs()
        if (remote.size > 0) {
            val free = dest.parentFile?.usableSpace ?: Long.MAX_VALUE
            val part0 = File(dest.path + ".part").takeIf { it.exists() }?.length() ?: 0L
            if (free + part0 < remote.size + (64L shl 20)) throw DownloadException("not enough free storage: needs ${remote.size shr 20} MB plus a safety margin")
        }
        val part = File(dest.path + ".part")
        val side = File(dest.path + ".part.json")
        val target = remote.url
        return if (remote.size > 0 && remote.acceptRanges) chunked(target, remote, dest, part, side, progress, cancelled)
        else sequential(target, remote, dest, part, side, progress, cancelled)
    }

    private fun chunked(url: String, r: Remote, dest: File, part: File, side: File, progress: (Long, Long) -> Unit, cancelled: () -> Boolean): FetchResult {
        val chunks = ((r.size + opt.chunkBytes - 1) / opt.chunkBytes).toInt()
        val loaded = if (part.exists() && part.length() == r.size && side.exists()) loadState(side, url, r, chunks) else null
        var resumed = 0L
        val state: State
        if (loaded == null) {
            part.delete(); side.delete()
            RandomAccessFile(part, "rw").use { it.setLength(r.size) }
            state = State(url, r.size, opt.chunkBytes, r.etag, r.lastModified, BooleanArray(chunks))
        } else {
            state = loaded
            for (i in 0 until chunks) if (state.done[i]) resumed += minOf(opt.chunkBytes, r.size - i * opt.chunkBytes)
            Logger.i("net", "resuming ${dest.name}: $resumed of ${r.size} bytes already present")
        }
        val doneBytes = AtomicLong(resumed)
        val failure = java.util.concurrent.atomic.AtomicReference<Throwable?>(null)
        val stop = AtomicBoolean(false)
        val todo = java.util.concurrent.ConcurrentLinkedQueue<Int>((0 until chunks).filter { !state.done[it] })
        val workers = minOf(opt.parallel.coerceIn(1, 8), maxOf(1, todo.size))
        val pool = Executors.newFixedThreadPool(workers)
        val sideLock = Any()
        var lastSave = 0L
        fun saveSide(force: Boolean = true) = synchronized(sideLock) {
            val t = System.currentTimeMillis()
            if (force || t - lastSave > 2000) { lastSave = t; try { side.writeText(state.toJson()) } catch (_: IOException) {} }
        }
        saveSide()
        val host = URL(url).host
        repeat(workers) {
            pool.execute {
                try {
                    RandomAccessFile(part, "rw").use { raf ->
                        while (!stop.get()) {
                            val ix = todo.poll() ?: break
                            val from = ix * opt.chunkBytes
                            val to = minOf(r.size, from + opt.chunkBytes) - 1
                            var attempt = 0
                            while (true) {
                                if (stop.get()) return@use
                                try {
                                    downloadRange(url, host, from, to, raf, r, doneBytes, progress, stop)
                                    synchronized(state) { state.done[ix] = true }
                                    saveSide(false)
                                    break
                                } catch (e: DownloadException) {
                                    if (!e.retryable || attempt >= opt.maxRetries) throw e
                                    attempt++
                                    Logger.w("net", "chunk $ix failed (${e.message}); retry $attempt")
                                    sleepCancelable(backoffMs(attempt), stop)
                                } catch (e: IOException) {
                                    if (attempt >= opt.maxRetries) throw e
                                    attempt++
                                    Logger.w("net", "chunk $ix network error (${e.message}); retry $attempt")
                                    sleepCancelable(backoffMs(attempt), stop)
                                }
                            }
                        }
                    }
                } catch (e: Throwable) {
                    failure.compareAndSet(null, e); stop.set(true)
                }
            }
        }
        pool.shutdown()
        while (!pool.awaitTermination(250, TimeUnit.MILLISECONDS)) {
            if (cancelled()) stop.set(true)
            progress(doneBytes.get(), r.size)
        }
        saveSide()
        if (cancelled() && failure.get() == null && state.done.any { !it }) throw DownloadException("cancelled; the partial download is kept and will resume next time")
        failure.get()?.let { throw if (it is IOException) it else DownloadException(it.message ?: it.toString()) }
        if (state.done.any { !it }) throw DownloadException("download incomplete; it will resume next time", true)
        if (part.length() != r.size) throw DownloadException("file size mismatch after download")
        dest.delete()
        if (!part.renameTo(dest)) throw DownloadException("could not finalize the downloaded file")
        side.delete()
        progress(r.size, r.size)
        return FetchResult.Done(dest, r, resumed)
    }

    private fun sleepCancelable(ms: Long, stop: AtomicBoolean) {
        var left = ms
        while (left > 0 && !stop.get()) { val s = minOf(left, 200L); Thread.sleep(s); left -= s }
    }

    private fun downloadRange(url: String, host: String, from: Long, to: Long, raf: RandomAccessFile, r: Remote, total: AtomicLong, progress: (Long, Long) -> Unit, stop: AtomicBoolean) {
        gate(host, opt.minHostIntervalMs)
        val extra = HashMap<String, String>()
        extra["Range"] = "bytes=$from-$to"
        // If the file changed on the server since we probed it, the server answers 200/412 and we stop mixing versions.
        (r.etag?.takeIf { !it.startsWith("W/") } ?: r.lastModified)?.let { extra["If-Range"] = it }
        val c = open(url, "GET", extra)
        try {
            val code = c.responseCode
            when {
                code == 429 || code == 503 -> {
                    val w = parseRetryAfter(c.getHeaderField("Retry-After")) ?: backoffMs(2)
                    Logger.w("net", "HTTP $code, server asks to wait ${w}ms")
                    sleepCancelable(w, stop)
                    throw DownloadException("rate limited (HTTP $code)", true)
                }
                code == 206 -> {}
                code in 500..599 -> throw DownloadException("server error HTTP $code", true)
                code == 200 || code == 412 || code == 416 -> throw DownloadException("the file changed on the server or does not support ranges (HTTP $code); start the download again")
                else -> throw DownloadException("HTTP $code")
            }
            val cr = c.getHeaderField("Content-Range") ?: throw DownloadException("missing Content-Range", true)
            val start = cr.substringAfter("bytes ").substringBefore('-').trim().toLongOrNull()
            if (start != from) throw DownloadException("server returned the wrong range", true)
            var pos = from
            val buf = ByteArray(64 * 1024)
            c.inputStream.use { ins ->
                while (pos <= to) {
                    if (stop.get()) throw DownloadException("stopped")
                    val n = ins.read(buf, 0, minOf(buf.size.toLong(), to - pos + 1).toInt())
                    if (n < 0) throw DownloadException("connection closed early", true)
                    synchronized(raf) { raf.seek(pos); raf.write(buf, 0, n) }
                    pos += n
                    total.addAndGet(n.toLong())
                }
            }
        } finally { c.disconnect() }
    }

    private fun sequential(url: String, r: Remote, dest: File, part: File, side: File, progress: (Long, Long) -> Unit, cancelled: () -> Boolean): FetchResult {
        side.delete(); part.delete()
        var attempt = 0
        val host = URL(url).host
        while (true) {
            gate(host, opt.minHostIntervalMs)
            val c = open(url, "GET", emptyMap())
            try {
                val code = c.responseCode
                if (code == 429 || code == 503) {
                    if (attempt++ >= opt.maxRetries) throw DownloadException("rate limited (HTTP $code)")
                    Thread.sleep(parseRetryAfter(c.getHeaderField("Retry-After")) ?: backoffMs(attempt)); continue
                }
                if (code !in 200..299) { if (code in 500..599 && attempt++ < opt.maxRetries) { Thread.sleep(backoffMs(attempt)); continue }; throw DownloadException("HTTP $code") }
                var n = 0L
                val limit = opt.maxBytes
                part.outputStream().buffered(256 * 1024).use { out ->
                    val ins: InputStream = c.inputStream
                    val buf = ByteArray(64 * 1024)
                    while (true) {
                        if (cancelled()) throw DownloadException("cancelled")
                        val k = ins.read(buf); if (k < 0) break
                        out.write(buf, 0, k); n += k
                        if (limit > 0 && n > limit) throw DownloadException("download exceeded the ${limit shr 20} MB limit you set")
                        progress(n, r.size)
                    }
                }
                if (r.size > 0 && n != r.size) throw DownloadException("connection closed early", true)
                dest.delete()
                if (!part.renameTo(dest)) throw DownloadException("could not finalize the downloaded file")
                return FetchResult.Done(dest, r, 0)
            } catch (e: IOException) {
                part.delete()
                if (e is DownloadException && !e.retryable) throw e
                if (attempt++ >= opt.maxRetries) throw e
                Logger.w("net", "download interrupted (${e.message}); retry $attempt from the start (server has no range support)")
                Thread.sleep(backoffMs(attempt))
            } finally { c.disconnect() }
        }
    }
}
