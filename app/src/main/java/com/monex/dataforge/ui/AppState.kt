package com.monex.dataforge.ui

import android.content.Context
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.monex.dataforge.data.Db
import com.monex.dataforge.data.Project
import com.monex.dataforge.data.Repo
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.util.Locale
import java.util.concurrent.atomic.AtomicBoolean

sealed class Screen {
    data class DatasetS(val id: String) : Screen()
    data class EditorS(val datasetId: String, val recordId: String?) : Screen()
    data class WriteS(val datasetId: String) : Screen()
    data class SourcesS(val datasetId: String) : Screen()
    data class IntelS(val datasetId: String) : Screen()
    object LogsS : Screen()
}

class KindInfo(val id: String, val en: String, val ar: String, val required: List<String>, val template: String)

val KINDS: List<KindInfo> = listOf(
    KindInfo("conversation", "Conversation", "محادثة", emptyList(),
        """{"messages":[{"role":"system","content":"{{system}}"},{"role":"user","content":"{{question}}"},{"role":"assistant","content":""}]}"""),
    KindInfo("qa", "Question & answer", "سؤال وجواب", listOf("question", "answer"), """{"question":"{{question}}","answer":""}"""),
    KindInfo("instruction", "Instruction", "تعليمات", listOf("instruction", "output"), """{"instruction":"{{instruction}}","input":"","output":""}"""),
    KindInfo("prompt_completion", "Prompt & completion", "موجه وإكمال", listOf("prompt", "completion"), """{"prompt":"{{prompt}}","completion":""}"""),
    KindInfo("classification", "Classification", "تصنيف", listOf("text", "label"), """{"text":"{{text}}","label":""}"""),
    KindInfo("extraction", "Information extraction", "استخراج معلومات", listOf("text", "entities"), """{"text":"{{text}}","entities":[]}"""),
    KindInfo("summarization", "Summarization", "تلخيص", listOf("document", "summary"), """{"document":"{{document}}","summary":""}"""),
    KindInfo("translation", "Translation", "ترجمة", listOf("source", "target"), """{"source":"{{source}}","target":"","source_lang":"","target_lang":""}"""),
    KindInfo("explanation", "Explanation", "شرح", listOf("topic", "explanation"), """{"topic":"{{topic}}","explanation":""}"""),
    KindInfo("reasoning", "Reasoning", "استدلال", listOf("problem", "reasoning", "answer"), """{"problem":"{{problem}}","reasoning":"","answer":""}"""),
    KindInfo("knowledge", "Structured knowledge", "معرفة منظمة", listOf("concept", "definition"),
        """{"concept":"{{concept}}","definition":"","claims":[{"claim":"","evidence":"","source":"","verification":"unverified"}],"relations":[]}"""),
    KindInfo("preference", "Preference comparison", "مقارنة تفضيلات", listOf("prompt", "chosen", "rejected"), """{"prompt":"{{prompt}}","chosen":"","rejected":""}"""),
    KindInfo("ranking", "Ranking", "ترتيب", listOf("prompt", "responses"), """{"prompt":"{{prompt}}","responses":[],"ranking":[]}"""),
    KindInfo("tool_use", "Tool use / action trace", "استخدام أدوات", emptyList(),
        """{"messages":[{"role":"user","content":"{{request}}"},{"role":"assistant","content":"","tool_calls":[{"name":"","arguments":{}}]},{"role":"tool","content":""},{"role":"assistant","content":""}]}"""),
    KindInfo("evaluation", "Evaluation", "تقييم", listOf("input", "expected"), """{"input":"{{input}}","expected":""}"""),
    KindInfo("custom", "Custom", "مخصص", emptyList(), """{"text":"{{text}}"}""")
)

fun kindInfo(id: String): KindInfo = KINDS.firstOrNull { it.id == id } ?: KINDS.last()
fun isConversationKind(kind: String) = kind == "conversation" || kind == "tool_use"

val SPLITS = listOf("train", "validation", "test", "unassigned")

class AppState(val ctx: Context) {
    val prefs = ctx.getSharedPreferences("df", Context.MODE_PRIVATE)
    val repo = Repo(Db(ctx))
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    var themeMode by mutableStateOf(prefs.getString("theme", "system") ?: "system")
    var lang by mutableStateOf(prefs.getString("lang", if (Locale.getDefault().language == "ar") "ar" else "en") ?: "en")
    var reduceMotion by mutableStateOf(prefs.getBoolean("motion_off", false))

    init { Lang.code = lang }

    var projects by mutableStateOf(listOf<Project>())
    var projectId by mutableStateOf("")
    var rev by mutableIntStateOf(0)
    var tab by mutableIntStateOf(0)
    var stack by mutableStateOf(listOf<Screen>())
    var busy by mutableStateOf<String?>(null)
    var busyDetail by mutableStateOf("")
    var message by mutableStateOf<String?>(null)
    val cancelFlag = AtomicBoolean(false)

    fun init() {
        projects = repo.ensureDefaultProject()
        val saved = prefs.getString("project", null)
        projectId = projects.firstOrNull { it.id == saved }?.id ?: projects.first().id
    }

    fun refreshProjects() {
        projects = repo.ensureDefaultProject()
        if (projects.none { it.id == projectId }) selectProject(projects.first().id)
    }

    fun selectProject(id: String) {
        projectId = id
        prefs.edit().putString("project", id).apply()
        bump()
    }

    fun setTheme(m: String) { themeMode = m; prefs.edit().putString("theme", m).apply() }
    @JvmName("applyLang")
    fun setLang(l: String) { lang = l; Lang.code = l; prefs.edit().putString("lang", l).apply() }
    fun setMotionOff(b: Boolean) { reduceMotion = b; prefs.edit().putBoolean("motion_off", b).apply() }

    fun bump() { rev++ }
    fun push(s: Screen) { com.monex.dataforge.core.Logger.i("nav", "push ${s::class.simpleName}"); stack = stack + s }
    fun pop() { if (stack.isNotEmpty()) { com.monex.dataforge.core.Logger.i("nav", "pop ${stack.last()::class.simpleName}"); stack = stack.dropLast(1) } }
    fun toast(s: String) { message = s }
    fun progress(s: String) { busyDetail = s }

    /** Runs [block] off the main thread. One task at a time. Errors are shown, never swallowed. */
    fun <T> work(label: String, block: () -> T, done: (T) -> Unit = {}) {
        if (busy != null) { message = "Another task is running"; return }
        busy = label; busyDetail = ""; cancelFlag.set(false)
        com.monex.dataforge.core.Logger.i("work", "start: $label")
        scope.launch {
            val r = try { Result.success(withContext(Dispatchers.IO) { block() }) } catch (e: Throwable) { Result.failure(e) }
            busy = null
            r.fold({
                com.monex.dataforge.core.Logger.i("work", "done: $label")
                try { done(it) } catch (e: Throwable) { com.monex.dataforge.core.Logger.e("work", "callback failed: $label", e); message = e.message ?: e.toString() }
                bump()
            }, {
                com.monex.dataforge.core.Logger.e("work", "FAILED: $label", it)
                message = it.message ?: it.toString(); bump()
            })
        }
    }
}
