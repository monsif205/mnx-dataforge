package com.monex.dataforge.core

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.Settings
import androidx.core.content.FileProvider
import org.json.JSONObject
import java.io.File
import java.net.HttpURLConnection
import java.net.URL

/**
 * Manual update from GitHub Releases. Only runs when the user presses the button (no background network).
 * Release tags are `build-<versionCode>`; parsing and version comparison are done by the Rust engine.
 * Android installs the APK only if it is signed with the same key as the installed app.
 */
object Updater {
    class Info(val available: Boolean, val versionCode: Long, val url: String, val size: Long, val notes: String)

    fun currentCode(ctx: Context): Long = ctx.packageManager.getPackageInfo(ctx.packageName, 0).longVersionCode

    private fun open(url: String): HttpURLConnection {
        val c = URL(url).openConnection() as HttpURLConnection
        c.connectTimeout = 10_000
        c.readTimeout = 20_000
        c.setRequestProperty("Accept", "application/vnd.github+json")
        c.setRequestProperty("User-Agent", "MNX-DataForge-Updater")
        return c
    }

    fun check(ctx: Context): Info {
        val c = open("https://api.github.com/repos/${Config.GITHUB_REPO}/releases/latest")
        try {
            val code = c.responseCode
            if (code == 404) throw IllegalStateException("No release found (the repository is private, or no signed build was published yet)")
            if (code != 200) throw IllegalStateException("GitHub answered HTTP $code")
            val body = c.inputStream.use { it.readBytes() }
            if (body.size > 512 * 1024) throw IllegalStateException("Unexpected response size")
            val r = NativeCore.run("release_info", JSONObject().put("json", String(body, Charsets.UTF_8)).put("current", currentCode(ctx)))
            Logger.i("update", "latest=${r.optLong("version_code")} current=${currentCode(ctx)} available=${r.optBoolean("available")}")
            return Info(r.getBoolean("available"), r.getLong("version_code"), r.getString("url"), r.getLong("size"), r.optString("notes"))
        } finally { c.disconnect() }
    }

    fun download(ctx: Context, info: Info, progress: (String) -> Unit): File {
        require(info.url.startsWith("https://")) { "Refusing a non-HTTPS download" }
        val dir = File(ctx.cacheDir, "update"); dir.mkdirs()
        val out = File(dir, "update.apk"); out.delete()
        val c = open(info.url)
        try {
            c.setRequestProperty("Accept", "application/octet-stream")
            if (c.responseCode != 200) throw IllegalStateException("Download failed: HTTP ${c.responseCode}")
            val total = if (info.size > 0) info.size else c.contentLengthLong
            var done = 0L
            c.inputStream.use { ins ->
                out.outputStream().use { os ->
                    val buf = ByteArray(64 * 1024)
                    while (true) {
                        val n = ins.read(buf)
                        if (n < 0) break
                        os.write(buf, 0, n)
                        done += n
                        if (total > 0) progress("${done * 100 / total}%  (${done / 1024} / ${total / 1024} KB)")
                    }
                }
            }
        } finally { c.disconnect() }
        if (out.length() < 100_000) throw IllegalStateException("Downloaded file is too small to be an APK")
        val pi = ctx.packageManager.getPackageArchiveInfo(out.absolutePath, 0)
        if (pi == null || pi.packageName != ctx.packageName) throw IllegalStateException("The downloaded file is not an update of this app")
        Logger.i("update", "downloaded ${out.length()} bytes, versionCode=${pi.longVersionCode}")
        return out
    }

    /** Opens the system installer. If "install unknown apps" is not allowed yet, opens that settings page instead. */
    fun install(ctx: Context, apk: File): Boolean {
        if (!ctx.packageManager.canRequestPackageInstalls()) {
            ctx.startActivity(Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:${ctx.packageName}")).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            return false
        }
        val uri = FileProvider.getUriForFile(ctx, ctx.packageName + ".files", apk)
        ctx.startActivity(
            Intent(Intent.ACTION_VIEW).setDataAndType(uri, "application/vnd.android.package-archive")
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
        )
        return true
    }
}
