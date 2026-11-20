package com.monex.dataforge.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.monex.dataforge.data.Dataset
import com.monex.dataforge.ops.CleanOptions
import com.monex.dataforge.ops.CleanReport
import com.monex.dataforge.ops.cleanDataset

/** Dataset cleanup (Rust engine): PII masking, boilerplate, repeated lines, and filters. Preview first, then apply. */
@Composable
fun CleanCard(st: AppState, ds: Dataset, onReport: (String) -> Unit) {
    val p = LocalPalette.current
    var pii by remember { mutableStateOf(setOf<String>()) }
    var boiler by remember { mutableStateOf(false) }
    var repeat by remember { mutableStateOf(false) }
    var minChars by remember { mutableStateOf(0) }
    var minQ by remember { mutableStateOf(0) }
    var maxRep by remember { mutableStateOf(0.0) }
    var lang by remember { mutableStateOf("") }
    fun opts() = CleanOptions(pii.toList(), boiler, repeat, minChars, minQ, maxRep, lang)
    fun show(r: CleanReport) {
        val c = r.counts
        onReport(
            (if (r.applied) tr("Cleanup applied", "تم التنظيف") else tr("Preview only (nothing changed)", "معاينة فقط (لم يتغير شيء)")) + (if (r.cancelled) tr(" (stopped early)", " (توقف مبكرا)") else "") + "\n" +
                tr("${r.examined} records checked, ${r.changed} changed, ${r.dropped} dropped.", "فحص ${r.examined} سجل، تغير ${r.changed}، وحذف ${r.dropped}.") + "\n" +
                "email ${c.optInt("email")}, phone ${c.optInt("phone")}, ip ${c.optInt("ip")}, url ${c.optInt("url")}, boilerplate ${c.optInt("boilerplate_lines")}, repeated ${c.optInt("repeated_lines")}" +
                (if (r.samples.isNotEmpty()) "\n" + r.samples.joinToString("\n") else "") +
                (if (r.applied) "\n" + tr("A version snapshot \"before cleanup\" was saved; edits keep their history.", "حفظت نسخة \"before cleanup\" وتبقى التعديلات في السجل.") else "")
        )
    }
    fun run(apply: Boolean) {
        val o = opts()
        st.work(if (apply) tr("Cleaning", "تنظيف") else tr("Checking", "فحص"), { cleanDataset(st.repo, ds, o, apply, st::progress) { st.cancelFlag.get() } }) { show(it) }
    }
    Glass(Modifier.fillMaxWidth(), strength = 0.55f) {
        Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            T(tr("Cleanup", "التنظيف"), 16.sp, FontWeight.SemiBold)
            T(tr("Mask personal data, remove page boilerplate and repeated lines, and drop weak records. Always preview first.", "إخفاء البيانات الشخصية، وحذف نصوص الصفحات المتكررة والأسطر المكررة، وإسقاط السجلات الضعيفة. عاين دائما أولا."), 12.sp, color = p.textDim)
            T(tr("Mask", "إخفاء"), 12.sp, FontWeight.SemiBold, p.textDim)
            ChipRow { listOf("email", "phone", "ip", "url").forEach { k -> Chip(k, k in pii) { pii = if (k in pii) pii - k else pii + k } } }
            T(tr("Text", "النص"), 12.sp, FontWeight.SemiBold, p.textDim)
            ChipRow {
                Chip(tr("boilerplate", "نصوص الصفحة"), boiler) { boiler = !boiler }
                Chip(tr("repeated lines", "أسطر مكررة"), repeat) { repeat = !repeat }
            }
            T(tr("Drop records", "إسقاط السجلات"), 12.sp, FontWeight.SemiBold, p.textDim)
            ChipRow { listOf(0, 20, 50, 100).forEach { n -> Chip(if (n == 0) tr("any length", "أي طول") else tr("< $n chars", "< $n حرف"), minChars == n) { minChars = n } } }
            ChipRow { listOf(0, 40, 60).forEach { n -> Chip(if (n == 0) tr("any quality", "أي جودة") else tr("quality < $n", "جودة < $n"), minQ == n) { minQ = n } } }
            ChipRow { listOf(0.0, 0.5, 0.7).forEach { n -> Chip(if (n == 0.0) tr("repeats ok", "التكرار مقبول") else tr("repeated words > ${(n * 100).toInt()}%", "كلمات مكررة > ${(n * 100).toInt()}%"), maxRep == n) { maxRep = n } } }
            ChipRow { listOf("", "ar", "en", "fr").forEach { l -> Chip(if (l.isEmpty()) tr("any language", "أي لغة") else tr("only $l", "$l فقط"), lang == l) { lang = l } } }
            ChipRow {
                GButton(tr("Preview", "معاينة"), { run(false) }, enabled = !opts().isEmpty)
                GButton(tr("Apply", "تطبيق"), { run(true) }, accent = true, enabled = !opts().isEmpty && !ds.locked)
            }
        }
    }
}
