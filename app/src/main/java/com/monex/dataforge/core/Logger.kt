package com.monex.dataforge.core

import android.content.Context
import android.os.Build
import android.util.Log
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * Tiny rotating file log (256 KB x 2) plus an uncaught-exception handler that records the full stack trace
 * before the process dies. Nothing leaves the device unless the user presses a send/copy/save button.
 */
object Logger {
    private const val MAX_BYTES = 256 * 1024
    private val lock = Any()
    private var dir: File? = null
    private var header = ""
    private val fmt = SimpleDateFormat("MM-dd HH:mm:ss.SSS", Locale.US)

    fun init(ctx: Context) {
        val d = File(ctx.filesDir, "logs")
        d.mkdirs()
        dir = d
        header = try {
            val pi = ctx.packageManager.getPackageInfo(ctx.packageName, 0)
            "MNX DataForge ${pi.versionName} (build ${pi.longVersionCode}) | Android ${Build.VERSION.RELEASE} (API ${Build.VERSION.SDK_INT}) | ${Build.MANUFACTURER} ${Build.MODEL} | ${Build.SUPPORTED_ABIS.firstOrNull() ?: "?"}"
        } catch (e: Exception) { "MNX DataForge | Android ${Build.VERSION.RELEASE}" }
        val prev = Thread.getDefaultUncaughtExceptionHandler()
        Thread.setDefaultUncaughtExceptionHandler { t, e ->
            try {
                write("E", "CRASH", "thread=${t.name}\n" + Log.getStackTraceString(e))
                File(d, "crashed").writeText(System.currentTimeMillis().toString())
            } catch (_: Throwable) { }
            prev?.uncaughtException(t, e)
        }
        val mb = Runtime.getRuntime().maxMemory() / 1048576
        i("app", "start | heap limit ${mb} MB")
    }

    fun headerLine(): String = header

    fun i(tag: String, msg: String) = write("I", tag, msg)
    fun w(tag: String, msg: String) = write("W", tag, msg)
    fun e(tag: String, msg: String, t: Throwable? = null) = write("E", tag, if (t == null) msg else msg + "\n" + Log.getStackTraceString(t))

    private fun write(level: String, tag: String, msg: String) {
        Log.println(when (level) { "E" -> Log.ERROR; "W" -> Log.WARN; else -> Log.INFO }, "MNX/$tag", msg)
        val d = dir ?: return
        synchronized(lock) {
            try {
                val f = File(d, "app.log")
                if (f.length() > MAX_BYTES) {
                    val old = File(d, "app.log.1")
                    old.delete()
                    f.renameTo(old)
                }
                File(d, "app.log").appendText("${fmt.format(Date())} $level/$tag: $msg\n")
            } catch (_: Throwable) { }
        }
    }

    /** Last [maxChars] characters of the log (older file first). */
    fun read(maxChars: Int = 80_000): String {
        val d = dir ?: return ""
        synchronized(lock) {
            val sb = StringBuilder()
            for (n in listOf("app.log.1", "app.log")) { val f = File(d, n); if (f.exists()) sb.append(f.readText()) }
            return if (sb.length > maxChars) sb.substring(sb.length - maxChars) else sb.toString()
        }
    }

    fun clear() {
        val d = dir ?: return
        synchronized(lock) { File(d, "app.log").delete(); File(d, "app.log.1").delete() }
    }

    fun hadCrash(): Boolean = dir?.let { File(it, "crashed").exists() } ?: false
    fun clearCrashFlag() { dir?.let { File(it, "crashed").delete() } }
}
