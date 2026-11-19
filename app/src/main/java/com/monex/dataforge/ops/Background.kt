package com.monex.dataforge.ops

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.IBinder
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat
import com.monex.dataforge.MainActivity
import com.monex.dataforge.core.Logger
import com.monex.dataforge.data.Shared
import java.util.concurrent.ConcurrentLinkedQueue
import java.util.concurrent.atomic.AtomicBoolean

/** Work that continues while the app is closed. Progress is checkpointed by the import engine, so an interrupted task resumes later. */
sealed class BgTask {
    abstract val label: String
    data class Fetch(val datasetId: String, val sourceId: String, val full: Boolean, override val label: String) : BgTask()
    data class Resume(val datasetId: String, val jobId: String, override val label: String) : BgTask()
}

object BgQueue {
    val pending = ConcurrentLinkedQueue<BgTask>()
    val stop = AtomicBoolean(false)
    var active by mutableStateOf(false)
    var status by mutableStateOf("")
    var lastReport by mutableStateOf("")

    fun enqueue(ctx: Context, t: BgTask) {
        if (pending.any { it == t }) return
        pending.add(t)
        ContextCompat.startForegroundService(ctx, Intent(ctx, BackgroundService::class.java))
    }
    fun cancelAll() { pending.clear(); stop.set(true) }
}

class BackgroundService : Service() {
    private var worker: Thread? = null
    private val chan = "df_background"

    override fun onBind(i: Intent?): IBinder? = null

    private fun notif(text: String, done: Boolean = false): Notification {
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP), PendingIntent.FLAG_IMMUTABLE)
        val stop = PendingIntent.getService(this, 1, Intent(this, BackgroundService::class.java).setAction("stop"), PendingIntent.FLAG_IMMUTABLE)
        val b = NotificationCompat.Builder(this, chan)
            .setSmallIcon(if (done) android.R.drawable.stat_sys_download_done else android.R.drawable.stat_sys_download)
            .setContentTitle("MNX DataForge")
            .setContentText(text)
            .setStyle(NotificationCompat.BigTextStyle().bigText(text))
            .setContentIntent(open)
            .setOnlyAlertOnce(true)
            .setOngoing(!done)
            .setSilent(true)
        if (!done) b.addAction(0, "Stop", stop)
        return b.build()
    }

    private fun show(text: String, done: Boolean = false) {
        BgQueue.status = text
        getSystemService(NotificationManager::class.java).notify(NID, notif(text, done))
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == "stop") { BgQueue.cancelAll(); return START_NOT_STICKY }
        val nm = getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(NotificationChannel(chan, "Background downloads", NotificationManager.IMPORTANCE_LOW))
        startForeground(NID, notif("Starting…"), ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        if (worker?.isAlive == true) return START_NOT_STICKY
        BgQueue.stop.set(false)
        BgQueue.active = true
        worker = Thread({
            val repo = Shared.repo(applicationContext)
            var done = 0
            var failed = 0
            val report = StringBuilder()
            try {
                while (true) {
                    val t = BgQueue.pending.poll() ?: break
                    if (BgQueue.stop.get()) break
                    show(t.label)
                    try {
                        val ds = repo.dataset(when (t) { is BgTask.Fetch -> t.datasetId; is BgTask.Resume -> t.datasetId }) ?: throw IllegalStateException("dataset not found")
                        val prog: (String) -> Unit = { s -> show("${t.label}\n$s") }
                        val line = when (t) {
                            is BgTask.Fetch -> {
                                val src = repo.source(t.sourceId) ?: throw IllegalStateException("source not found")
                                val o = fetchSource(applicationContext, repo, ds, src, null, t.full, prog) { BgQueue.stop.get() }
                                if (o.unchanged) o.note else "imported ${o.imported}, unchanged ${o.skipped}, rejected ${o.rejected}. ${o.note}"
                            }
                            is BgTask.Resume -> {
                                val job = repo.job(t.jobId) ?: throw IllegalStateException("the unfinished import no longer exists")
                                val r = runJob(applicationContext, repo, ds, job, prog) { BgQueue.stop.get() }
                                "imported ${r.imported} more, rejected ${r.rejected}${if (r.cancelled) " (paused)" else ""}"
                            }
                        }
                        done++
                        report.append(t.label).append(": ").append(line).append('\n')
                        Logger.i("background", "${t.label}: $line")
                    } catch (e: Throwable) {
                        failed++
                        report.append(t.label).append(": FAILED ").append(e.message ?: e.toString()).append('\n')
                        Logger.e("background", "FAILED: ${t.label}", e)
                    }
                }
            } finally {
                BgQueue.lastReport = report.toString().trim()
                BgQueue.active = false
                val summary = if (BgQueue.stop.get()) "Stopped. Unfinished imports can be resumed in the app." else "Finished: $done ok, $failed failed."
                stopForeground(STOP_FOREGROUND_DETACH)
                show(summary, done = true)
                stopSelf()
            }
        }, "df-background").also { it.start() }
        return START_NOT_STICKY
    }

    /** Android 15 limits data-sync services to about 6 hours per day; stop cleanly, the checkpoint keeps the progress. */
    override fun onTimeout(startId: Int, fgsType: Int) {
        Logger.w("background", "foreground service time limit reached; progress is saved")
        BgQueue.pending.clear(); BgQueue.stop.set(true)
    }

    override fun onDestroy() {
        BgQueue.stop.set(true)
        super.onDestroy()
    }

    companion object { private const val NID = 4201 }
}
