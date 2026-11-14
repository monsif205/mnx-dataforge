package com.monex.dataforge.data

import android.content.ContentValues
import android.content.Context
import android.database.sqlite.SQLiteDatabase
import android.database.sqlite.SQLiteOpenHelper
import android.database.sqlite.SQLiteStatement
import com.monex.dataforge.core.NativeCore
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.InputStream
import java.io.OutputStream
import java.security.MessageDigest
import java.util.UUID

const val DB_NAME = "dataforge.db"
const val DB_VERSION = 1

class Db(private val ctx: Context) : SQLiteOpenHelper(ctx, DB_NAME, null, DB_VERSION) {
    override fun onConfigure(db: SQLiteDatabase) {
        db.enableWriteAheadLogging()
    }

    override fun onCreate(db: SQLiteDatabase) {
        val ddl = listOf(
            "CREATE TABLE projects(id TEXT PRIMARY KEY, name TEXT NOT NULL, created INTEGER, updated INTEGER, archived INTEGER DEFAULT 0)",
            "CREATE TABLE datasets(id TEXT PRIMARY KEY, project_id TEXT NOT NULL, name TEXT NOT NULL, kind TEXT NOT NULL, description TEXT DEFAULT '', language TEXT DEFAULT '', required TEXT DEFAULT '[]', locked INTEGER DEFAULT 0, created INTEGER, updated INTEGER)",
            "CREATE INDEX idx_ds_project ON datasets(project_id)",
            "CREATE TABLE records(seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT UNIQUE NOT NULL, dataset_id TEXT NOT NULL, data TEXT NOT NULL, split TEXT DEFAULT 'unassigned', origin TEXT DEFAULT 'human', source TEXT, license TEXT, label TEXT, grp TEXT, created INTEGER, updated INTEGER)",
            "CREATE INDEX idx_rec_ds ON records(dataset_id, seq)",
            "CREATE INDEX idx_rec_split ON records(dataset_id, split)",
            "CREATE VIRTUAL TABLE records_fts USING fts4(body, tokenize=unicode61)",
            "CREATE TABLE record_history(id INTEGER PRIMARY KEY AUTOINCREMENT, record_id TEXT NOT NULL, ts INTEGER, op TEXT, detail TEXT, hash TEXT)",
            "CREATE INDEX idx_hist_rec ON record_history(record_id)",
            "CREATE TABLE dataset_log(id INTEGER PRIMARY KEY AUTOINCREMENT, dataset_id TEXT NOT NULL, ts INTEGER, op TEXT, detail TEXT)",
            "CREATE INDEX idx_log_ds ON dataset_log(dataset_id)",
            "CREATE TABLE blobs(hash TEXT PRIMARY KEY, data TEXT NOT NULL)",
            "CREATE TABLE versions(id TEXT PRIMARY KEY, dataset_id TEXT NOT NULL, number INTEGER, label TEXT, created INTEGER, record_count INTEGER, schema_json TEXT)",
            "CREATE INDEX idx_ver_ds ON versions(dataset_id)",
            "CREATE TABLE version_records(version_id TEXT NOT NULL, record_id TEXT NOT NULL, hash TEXT NOT NULL, split TEXT, origin TEXT, source TEXT, license TEXT, label TEXT, grp TEXT, PRIMARY KEY(version_id, record_id))",
            "CREATE TABLE validation_runs(id TEXT PRIMARY KEY, dataset_id TEXT NOT NULL, created INTEGER, errors INTEGER, warnings INTEGER, report TEXT)",
            "CREATE INDEX idx_val_ds ON validation_runs(dataset_id)",
            "CREATE TABLE imports(id TEXT PRIMARY KEY, dataset_id TEXT NOT NULL, source TEXT, license TEXT, ts INTEGER, imported INTEGER, rejected INTEGER)"
        )
        ddl.forEach { db.execSQL(it) }
    }

    override fun onUpgrade(db: SQLiteDatabase, oldVersion: Int, newVersion: Int) {
        // Version 1 is the first schema. Future migrations go here as explicit, ordered steps:
        // if (oldVersion < 2) { db.execSQL("ALTER TABLE ..."); }
    }

    fun dbFile(): File = ctx.getDatabasePath(DB_NAME)
}

data class Project(val id: String, val name: String)
data class Dataset(
    val id: String, val projectId: String, val name: String, val kind: String, val description: String,
    val language: String, val required: List<String>, val locked: Boolean
)

data class Rec(
    val seq: Long, val id: String, val datasetId: String, val data: String, val split: String, val origin: String,
    val source: String?, val license: String?, val label: String?, val grp: String?, val updated: Long
)

data class NewRec(
    val data: JSONObject, val origin: String, val source: String? = null, val license: String? = null,
    val label: String? = null, val grp: String? = null, val split: String = "unassigned"
)

data class VersionInfo(val id: String, val number: Int, val label: String, val created: Long, val count: Int)
data class HistoryItem(val ts: Long, val op: String, val detail: String, val hash: String?)

fun sha256Hex(s: String): String {
    val d = MessageDigest.getInstance("SHA-256").digest(s.toByteArray(Charsets.UTF_8))
    return d.joinToString("") { "%02x".format(it) }
}

private fun SQLiteStatement.bindS(i: Int, s: String?) {
    if (s == null) bindNull(i) else bindString(i, s)
}

class Repo(private val helper: Db) {
    private val w: SQLiteDatabase get() = helper.writableDatabase
    private fun now() = System.currentTimeMillis()
    private fun uid() = UUID.randomUUID().toString()

    fun close() = helper.close()

    // ---------- projects ----------
    fun ensureDefaultProject(): List<Project> {
        if (projects().isEmpty()) createProject("Default")
        return projects()
    }

    fun projects(): List<Project> {
        val out = ArrayList<Project>()
        w.rawQuery("SELECT id,name FROM projects WHERE archived=0 ORDER BY created", null).use { c ->
            while (c.moveToNext()) out.add(Project(c.getString(0), c.getString(1)))
        }
        return out
    }

    fun createProject(name: String): Project {
        val p = Project(uid(), name.trim().ifEmpty { "Project" })
        w.execSQL("INSERT INTO projects(id,name,created,updated) VALUES(?,?,?,?)", arrayOf(p.id, p.name, now(), now()))
        return p
    }

    fun renameProject(id: String, name: String) {
        w.execSQL("UPDATE projects SET name=?, updated=? WHERE id=?", arrayOf(name.trim(), now(), id))
    }

    fun archiveProject(id: String) {
        w.execSQL("UPDATE projects SET archived=1, updated=? WHERE id=?", arrayOf(now(), id))
    }

    fun deleteProject(id: String) {
        datasets(id).forEach { deleteDataset(it.id) }
        w.execSQL("DELETE FROM projects WHERE id=?", arrayOf(id))
    }

    fun duplicateProject(id: String): Project {
        val src = projects().first { it.id == id }
        val np = createProject(src.name + " (copy)")
        datasets(id).forEach { duplicateDataset(it.id, np.id) }
        return np
    }

    // ---------- datasets ----------
    private fun readDataset(c: android.database.Cursor) = Dataset(
        c.getString(0), c.getString(1), c.getString(2), c.getString(3), c.getString(4) ?: "", c.getString(5) ?: "",
        JSONArray(c.getString(6) ?: "[]").let { a -> (0 until a.length()).map { a.getString(it) } }, c.getInt(7) == 1
    )

    private val dsCols = "id,project_id,name,kind,description,language,required,locked"

    fun datasets(projectId: String): List<Dataset> {
        val out = ArrayList<Dataset>()
        w.rawQuery("SELECT $dsCols FROM datasets WHERE project_id=? ORDER BY created", arrayOf(projectId)).use { c ->
            while (c.moveToNext()) out.add(readDataset(c))
        }
        return out
    }

    fun dataset(id: String): Dataset? =
        w.rawQuery("SELECT $dsCols FROM datasets WHERE id=?", arrayOf(id)).use { c -> if (c.moveToFirst()) readDataset(c) else null }

    fun createDataset(projectId: String, name: String, kind: String, language: String, required: List<String>, description: String = ""): Dataset {
        val id = uid()
        w.execSQL(
            "INSERT INTO datasets(id,project_id,name,kind,description,language,required,locked,created,updated) VALUES(?,?,?,?,?,?,?,0,?,?)",
            arrayOf(id, projectId, name.trim().ifEmpty { "Dataset" }, kind, description, language, JSONArray(required).toString(), now(), now())
        )
        logDs(id, "create", kind)
        return dataset(id)!!
    }

    fun updateDatasetMeta(id: String, name: String, description: String, language: String, required: List<String>) {
        w.execSQL(
            "UPDATE datasets SET name=?, description=?, language=?, required=?, updated=? WHERE id=?",
            arrayOf(name.trim(), description, language, JSONArray(required).toString(), now(), id)
        )
        logDs(id, "meta", "name/description/language/required updated")
    }

    fun setLocked(id: String, locked: Boolean) {
        w.execSQL("UPDATE datasets SET locked=?, updated=? WHERE id=?", arrayOf(if (locked) 1 else 0, now(), id))
        logDs(id, if (locked) "lock" else "unlock", "")
    }

    fun deleteDataset(id: String) {
        w.beginTransaction()
        try {
            w.execSQL("DELETE FROM records_fts WHERE docid IN (SELECT seq FROM records WHERE dataset_id=?)", arrayOf(id))
            w.execSQL("DELETE FROM records WHERE dataset_id=?", arrayOf(id))
            w.execSQL("DELETE FROM version_records WHERE version_id IN (SELECT id FROM versions WHERE dataset_id=?)", arrayOf(id))
            w.execSQL("DELETE FROM versions WHERE dataset_id=?", arrayOf(id))
            w.execSQL("DELETE FROM validation_runs WHERE dataset_id=?", arrayOf(id))
            w.execSQL("DELETE FROM dataset_log WHERE dataset_id=?", arrayOf(id))
            w.execSQL("DELETE FROM imports WHERE dataset_id=?", arrayOf(id))
            w.execSQL("DELETE FROM datasets WHERE id=?", arrayOf(id))
            w.execSQL("DELETE FROM blobs WHERE hash NOT IN (SELECT hash FROM version_records) AND hash NOT IN (SELECT hash FROM record_history WHERE hash IS NOT NULL)")
            w.setTransactionSuccessful()
        } finally {
            w.endTransaction()
        }
    }

    fun duplicateDataset(id: String, targetProject: String? = null): Dataset {
        val src = dataset(id)!!
        val nd = createDataset(targetProject ?: src.projectId, src.name + " (copy)", src.kind, src.language, src.required, src.description)
        val items = ArrayList<NewRec>()
        pageAll(id) { r ->
            items.add(NewRec(JSONObject(r.data), r.origin, r.source, r.license, r.label, r.grp, r.split))
            if (items.size >= 500) {
                insertRecords(nd.id, items.toList(), history = false); items.clear()
            }
        }
        if (items.isNotEmpty()) insertRecords(nd.id, items.toList(), history = false)
        logDs(nd.id, "duplicate", "from ${src.id}")
        return nd
    }

    // ---------- records ----------
    private fun readRec(c: android.database.Cursor) = Rec(
        c.getLong(0), c.getString(1), c.getString(2), c.getString(3), c.getString(4), c.getString(5),
        c.getString(6), c.getString(7), c.getString(8), c.getString(9), c.getLong(10)
    )

    private val recCols = "seq,id,dataset_id,data,split,origin,source,license,label,grp,updated"

    /** Normalized (Arabic-folded, lowercase) text for the full-text index, computed by the Rust engine. */
    private fun norms(datas: List<JSONObject>): List<String> {
        if (datas.isEmpty()) return emptyList()
        val arr = JSONArray()
        datas.forEachIndexed { i, d -> arr.put(JSONObject().put("id", i.toString()).put("data", d)) }
        val items = NativeCore.run("record_info", JSONObject().put("records", arr)).getJSONArray("items")
        return (0 until items.length()).map { items.getJSONObject(it).getString("norm") }
    }

    fun ftsQuery(q: String): String? {
        if (q.isBlank()) return null
        val n = NativeCore.run("normalize", JSONObject().put("text", q)).getString("text")
        val toks = n.split(' ').filter { it.isNotEmpty() }
        if (toks.isEmpty()) return null
        return toks.joinToString(" ") { "$it*" }
    }

    private fun whereFor(ds: String, query: String?, split: String?): Pair<String, Array<String>> {
        val args = ArrayList<String>()
        val sb = StringBuilder("dataset_id=?")
        args.add(ds)
        if (split != null) { sb.append(" AND split=?"); args.add(split) }
        val fts = query?.let { ftsQuery(it) }
        if (fts != null) {
            sb.append(" AND seq IN (SELECT docid FROM records_fts WHERE records_fts MATCH ?)")
            args.add(fts)
        }
        return sb.toString() to args.toTypedArray()
    }

    fun page(ds: String, query: String?, split: String?, limit: Int, offset: Int): List<Rec> {
        val (wh, args) = whereFor(ds, query, split)
        val out = ArrayList<Rec>()
        w.rawQuery("SELECT $recCols FROM records WHERE $wh ORDER BY seq LIMIT $limit OFFSET $offset", args).use { c ->
            while (c.moveToNext()) out.add(readRec(c))
        }
        return out
    }

    fun count(ds: String, query: String? = null, split: String? = null): Int {
        val (wh, args) = whereFor(ds, query, split)
        return w.rawQuery("SELECT COUNT(*) FROM records WHERE $wh", args).use { c -> if (c.moveToFirst()) c.getInt(0) else 0 }
    }

    fun record(id: String): Rec? =
        w.rawQuery("SELECT $recCols FROM records WHERE id=?", arrayOf(id)).use { c -> if (c.moveToFirst()) readRec(c) else null }

    /** Streams all records of a dataset in seq order, 500 rows at a time (never loads everything). */
    fun pageAll(ds: String, split: String? = null, each: (Rec) -> Unit) {
        var last = 0L
        while (true) {
            val args = if (split != null) arrayOf(ds, split, last.toString()) else arrayOf(ds, last.toString())
            val extra = if (split != null) " AND split=?" else ""
            val rows = ArrayList<Rec>()
            w.rawQuery("SELECT $recCols FROM records WHERE dataset_id=?$extra AND seq>? ORDER BY seq LIMIT 500", args).use { c ->
                while (c.moveToNext()) rows.add(readRec(c))
            }
            if (rows.isEmpty()) break
            rows.forEach(each)
            last = rows.last().seq
        }
    }

    private fun requireUnlocked(ds: String) {
        if (dataset(ds)?.locked == true) throw IllegalStateException("Dataset is locked (protection mode). Unlock it explicitly first.")
    }

    private fun putBlob(data: String): String {
        val h = sha256Hex(data)
        w.execSQL("INSERT OR IGNORE INTO blobs(hash,data) VALUES(?,?)", arrayOf(h, data))
        return h
    }

    private fun addHistory(recordId: String, op: String, detail: String, hash: String? = null) {
        val cv = ContentValues()
        cv.put("record_id", recordId); cv.put("ts", now()); cv.put("op", op); cv.put("detail", detail)
        if (hash != null) cv.put("hash", hash)
        w.insert("record_history", null, cv)
    }

    fun insertRecords(ds: String, items: List<NewRec>, history: Boolean = true): List<String> {
        requireUnlocked(ds)
        val ids = ArrayList<String>()
        val nm = norms(items.map { it.data })
        w.beginTransaction()
        try {
            items.forEachIndexed { i, r ->
                val id = uid()
                val cv = ContentValues()
                cv.put("id", id); cv.put("dataset_id", ds); cv.put("data", r.data.toString()); cv.put("split", r.split)
                cv.put("origin", r.origin); cv.put("source", r.source); cv.put("license", r.license)
                cv.put("label", r.label); cv.put("grp", r.grp); cv.put("created", now()); cv.put("updated", now())
                val seq = w.insertOrThrow("records", null, cv)
                w.execSQL("INSERT INTO records_fts(docid, body) VALUES(?,?)", arrayOf(seq, nm[i]))
                if (history) addHistory(id, "create", "origin=${r.origin}")
                ids.add(id)
            }
            w.execSQL("UPDATE datasets SET updated=? WHERE id=?", arrayOf(now(), ds))
            w.setTransactionSuccessful()
        } finally {
            w.endTransaction()
        }
        return ids
    }

    fun updateRecord(id: String, data: JSONObject, label: String? = null) {
        val cur = record(id) ?: throw IllegalStateException("Record not found")
        requireUnlocked(cur.datasetId)
        val nm = norms(listOf(data))[0]
        w.beginTransaction()
        try {
            val prev = putBlob(cur.data)
            val origin = if (cur.origin == "imported") "edited_import" else cur.origin
            w.execSQL(
                "UPDATE records SET data=?, origin=?, label=COALESCE(?,label), updated=? WHERE id=?",
                arrayOf<Any?>(data.toString(), origin, label, now(), id)
            )
            w.execSQL("UPDATE records_fts SET body=? WHERE docid=?", arrayOf(nm, cur.seq))
            addHistory(id, "edit", "origin ${cur.origin} -> $origin; previous version stored", prev)
            w.setTransactionSuccessful()
        } finally {
            w.endTransaction()
        }
    }

    fun updateMeta(id: String, label: String?, grp: String?, source: String?, license: String?) {
        val cur = record(id) ?: throw IllegalStateException("Record not found")
        requireUnlocked(cur.datasetId)
        fun n(s: String?) = s?.trim()?.ifEmpty { null }
        w.execSQL(
            "UPDATE records SET label=?, grp=?, source=?, license=?, updated=? WHERE id=?",
            arrayOf<Any?>(n(label), n(grp), n(source), n(license), now(), id)
        )
        addHistory(id, "meta", "label/group/source/license updated")
    }

    fun deleteRecords(ids: List<String>) {
        if (ids.isEmpty()) return
        w.beginTransaction()
        try {
            ids.forEach { id ->
                val r = record(id) ?: return@forEach
                requireUnlocked(r.datasetId)
                val h = putBlob(r.data)
                addHistory(id, "delete", "deleted; final content kept as blob", h)
                w.execSQL("DELETE FROM records_fts WHERE docid=?", arrayOf(r.seq))
                w.execSQL("DELETE FROM records WHERE id=?", arrayOf(id))
            }
            w.setTransactionSuccessful()
        } finally {
            w.endTransaction()
        }
    }

    fun setSplits(ds: String, assign: Map<String, String>) {
        requireUnlocked(ds)
        w.beginTransaction()
        try {
            val st = w.compileStatement("UPDATE records SET split=? WHERE id=? AND dataset_id=?")
            assign.forEach { (id, split) ->
                st.bindString(1, split); st.bindString(2, id); st.bindString(3, ds); st.executeUpdateDelete()
            }
            logDs(ds, "split", "${assign.size} records assigned")
            w.setTransactionSuccessful()
        } finally {
            w.endTransaction()
        }
    }

    fun history(recordId: String): List<HistoryItem> {
        val out = ArrayList<HistoryItem>()
        w.rawQuery("SELECT ts,op,detail,hash FROM record_history WHERE record_id=? ORDER BY id DESC", arrayOf(recordId)).use { c ->
            while (c.moveToNext()) out.add(HistoryItem(c.getLong(0), c.getString(1), c.getString(2) ?: "", c.getString(3)))
        }
        return out
    }

    fun blob(hash: String): String? =
        w.rawQuery("SELECT data FROM blobs WHERE hash=?", arrayOf(hash)).use { c -> if (c.moveToFirst()) c.getString(0) else null }

    fun logDs(ds: String, op: String, detail: String) {
        w.execSQL("INSERT INTO dataset_log(dataset_id,ts,op,detail) VALUES(?,?,?,?)", arrayOf(ds, now(), op, detail))
    }

    fun datasetLog(ds: String, limit: Int = 50): List<HistoryItem> {
        val out = ArrayList<HistoryItem>()
        w.rawQuery("SELECT ts,op,detail FROM dataset_log WHERE dataset_id=? ORDER BY id DESC LIMIT $limit", arrayOf(ds)).use { c ->
            while (c.moveToNext()) out.add(HistoryItem(c.getLong(0), c.getString(1), c.getString(2) ?: "", null))
        }
        return out
    }

    fun logImport(ds: String, source: String, license: String, imported: Int, rejected: Int) {
        w.execSQL("INSERT INTO imports(id,dataset_id,source,license,ts,imported,rejected) VALUES(?,?,?,?,?,?,?)", arrayOf(uid(), ds, source, license, now(), imported, rejected))
        logDs(ds, "import", "$source: $imported imported, $rejected rejected, license=$license")
    }

    // ---------- statistics ----------
    fun groupCounts(ds: String, column: String): Map<String, Int> {
        require(column in listOf("split", "origin", "label", "license", "source"))
        val out = LinkedHashMap<String, Int>()
        w.rawQuery("SELECT COALESCE($column,'(none)'), COUNT(*) FROM records WHERE dataset_id=? GROUP BY 1 ORDER BY 2 DESC", arrayOf(ds)).use { c ->
            while (c.moveToNext()) out[c.getString(0)] = c.getInt(1)
        }
        return out
    }

    fun texts(ds: String, maxRecords: Int, maxChars: Int): List<Pair<String, String>> {
        val out = ArrayList<Pair<String, String>>()
        var offset = 0
        while (out.size < maxRecords) {
            val rows = page(ds, null, null, 500, offset)
            if (rows.isEmpty()) break
            val arr = JSONArray()
            rows.forEach { arr.put(JSONObject().put("id", it.id).put("data", JSONObject(it.data))) }
            val items = NativeCore.run("record_info", JSONObject().put("records", arr)).getJSONArray("items")
            for (i in 0 until items.length()) {
                if (out.size >= maxRecords) break
                val it2 = items.getJSONObject(i)
                out.add(it2.getString("id") to it2.getString("text").take(maxChars))
            }
            offset += rows.size
        }
        return out
    }

    /** Result of a read-only SQL query for the SQL console. */
    class SqlResult(val columns: List<String>, val rows: List<List<String>>, val truncated: Boolean)

    /**
     * Runs one SELECT/WITH statement. `:ds` is replaced by the dataset id literal. Anything else is rejected
     * before it reaches SQLite, and rawQuery only ever runs a single statement.
     */
    fun sql(ds: String, query: String, maxRows: Int = 200): SqlResult {
        val q = query.trim().trimEnd(';').trim()
        require(q.isNotEmpty()) { "Empty query" }
        require(!q.contains(';')) { "Only one statement is allowed" }
        val head = q.lowercase().takeWhile { !it.isWhitespace() }
        require(head == "select" || head == "with") { "Only SELECT queries are allowed" }
        require(ds.all { it.isLetterOrDigit() || it == '-' || it == '_' }) { "bad dataset id" }
        val bound = q.replace(":ds", "'$ds'")
        w.rawQuery(bound, null).use { c ->
            val cols = c.columnNames.toList()
            val rows = ArrayList<List<String>>()
            var truncated = false
            while (c.moveToNext()) {
                if (rows.size >= maxRows) { truncated = true; break }
                rows.add(cols.indices.map { i -> if (c.isNull(i)) "NULL" else try { c.getString(i) } catch (e: Exception) { "<blob>" } })
            }
            return SqlResult(cols, rows, truncated)
        }
    }

    /** Total estimated tokens of a dataset (reads texts through the Rust engine, bounded). */
    fun tokenTotals(ds: String, maxRecords: Int = 20000): JSONObject {
        val t = texts(ds, maxRecords, 20000)
        val arr = JSONArray(); t.forEach { arr.put(it.second) }
        return NativeCore.run("tokens", JSONObject().put("texts", arr))
    }

    fun provenanceSummary(ds: String): JSONObject {
        val unknownLicense = w.rawQuery(
            "SELECT COUNT(*) FROM records WHERE dataset_id=? AND origin IN ('imported','edited_import','transformed') AND (license IS NULL OR license='' OR LOWER(license)='unknown')",
            arrayOf(ds)
        ).use { c -> if (c.moveToFirst()) c.getInt(0) else 0 }
        val missingSource = w.rawQuery(
            "SELECT COUNT(*) FROM records WHERE dataset_id=? AND origin IN ('imported','edited_import','transformed') AND (source IS NULL OR source='')",
            arrayOf(ds)
        ).use { c -> if (c.moveToFirst()) c.getInt(0) else 0 }
        return JSONObject().put("unknown_license", unknownLicense).put("missing_source", missingSource)
    }

    fun dashboard(): JSONObject {
        fun scalar(sql: String) = w.rawQuery(sql, null).use { c -> if (c.moveToFirst()) c.getLong(0) else 0L }
        val size = helper.dbFile().length() + File(helper.dbFile().path + "-wal").let { if (it.exists()) it.length() else 0L }
        val lastRuns = w.rawQuery(
            "SELECT COALESCE(SUM(errors),0), COALESCE(SUM(warnings),0) FROM validation_runs v WHERE created = (SELECT MAX(created) FROM validation_runs WHERE dataset_id=v.dataset_id)", null
        ).use { c -> if (c.moveToFirst()) c.getLong(0) to c.getLong(1) else (0L to 0L) }
        val recent = JSONArray()
        w.rawQuery("SELECT name, updated FROM datasets ORDER BY updated DESC LIMIT 5", null).use { c ->
            while (c.moveToNext()) recent.put(JSONObject().put("name", c.getString(0)).put("updated", c.getLong(1)))
        }
        val origins = JSONObject()
        w.rawQuery("SELECT origin, COUNT(*) FROM records GROUP BY origin", null).use { c ->
            while (c.moveToNext()) origins.put(c.getString(0), c.getInt(1))
        }
        return JSONObject()
            .put("datasets", scalar("SELECT COUNT(*) FROM datasets"))
            .put("records", scalar("SELECT COUNT(*) FROM records"))
            .put("unassigned", scalar("SELECT COUNT(*) FROM records WHERE split='unassigned'"))
            .put("validated_datasets", scalar("SELECT COUNT(DISTINCT dataset_id) FROM validation_runs"))
            .put("last_errors", lastRuns.first).put("last_warnings", lastRuns.second)
            .put("storage_bytes", size).put("recent", recent).put("origins", origins)
    }

    // ---------- validation runs ----------
    fun saveValidation(ds: String, report: JSONObject) {
        w.execSQL(
            "INSERT OR REPLACE INTO validation_runs(id,dataset_id,created,errors,warnings,report) VALUES(?,?,?,?,?,?)",
            arrayOf(report.getString("validation_id"), ds, now(), report.getInt("errors"), report.getInt("warnings"), report.toString())
        )
    }

    fun lastValidation(ds: String): JSONObject? =
        w.rawQuery("SELECT report FROM validation_runs WHERE dataset_id=? ORDER BY created DESC LIMIT 1", arrayOf(ds))
            .use { c -> if (c.moveToFirst()) JSONObject(c.getString(0)) else null }

    // ---------- versions (content-addressed snapshots) ----------
    fun snapshot(ds: String, label: String): VersionInfo {
        val d = dataset(ds)!!
        w.beginTransaction()
        try {
            val number = w.rawQuery("SELECT COALESCE(MAX(number),0)+1 FROM versions WHERE dataset_id=?", arrayOf(ds)).use { c -> c.moveToFirst(); c.getInt(0) }
            val vid = uid()
            val insBlob = w.compileStatement("INSERT OR IGNORE INTO blobs(hash,data) VALUES(?,?)")
            val insVr = w.compileStatement("INSERT INTO version_records(version_id,record_id,hash,split,origin,source,license,label,grp) VALUES(?,?,?,?,?,?,?,?,?)")
            var n = 0
            w.rawQuery("SELECT id,data,split,origin,source,license,label,grp FROM records WHERE dataset_id=? ORDER BY seq", arrayOf(ds)).use { c ->
                while (c.moveToNext()) {
                    val data = c.getString(1)
                    val h = sha256Hex(data)
                    insBlob.bindString(1, h); insBlob.bindString(2, data); insBlob.executeInsert()
                    insVr.bindString(1, vid); insVr.bindString(2, c.getString(0)); insVr.bindString(3, h)
                    insVr.bindS(4, c.getString(2)); insVr.bindS(5, c.getString(3)); insVr.bindS(6, c.getString(4))
                    insVr.bindS(7, c.getString(5)); insVr.bindS(8, c.getString(6)); insVr.bindS(9, c.getString(7))
                    insVr.executeInsert()
                    n++
                }
            }
            val schema = JSONObject().put("kind", d.kind).put("required", JSONArray(d.required)).put("language", d.language)
            w.execSQL(
                "INSERT INTO versions(id,dataset_id,number,label,created,record_count,schema_json) VALUES(?,?,?,?,?,?,?)",
                arrayOf(vid, ds, number, label, now(), n, schema.toString())
            )
            logDs(ds, "snapshot", "v$number: $label ($n records)")
            w.setTransactionSuccessful()
            return VersionInfo(vid, number, label, now(), n)
        } finally {
            w.endTransaction()
        }
    }

    fun versions(ds: String): List<VersionInfo> {
        val out = ArrayList<VersionInfo>()
        w.rawQuery("SELECT id,number,label,created,record_count FROM versions WHERE dataset_id=? ORDER BY number DESC", arrayOf(ds)).use { c ->
            while (c.moveToNext()) out.add(VersionInfo(c.getString(0), c.getInt(1), c.getString(2) ?: "", c.getLong(3), c.getInt(4)))
        }
        return out
    }

    fun diff(a: String, b: String): JSONObject {
        fun ids(sql: String): JSONArray {
            val arr = JSONArray()
            w.rawQuery("$sql LIMIT 50", arrayOf(b, a)).use { c -> while (c.moveToNext()) arr.put(c.getString(0)) }
            return arr
        }
        fun cnt(sql: String) = w.rawQuery(sql, arrayOf(b, a)).use { c -> c.moveToFirst(); c.getInt(0) }
        val addedSql = "FROM version_records WHERE version_id=? AND record_id NOT IN (SELECT record_id FROM version_records WHERE version_id=?)"
        val removedSql = "FROM version_records WHERE version_id=? AND record_id NOT IN (SELECT record_id FROM version_records WHERE version_id=?)"
        val modSql = "FROM version_records x JOIN version_records y ON x.record_id=y.record_id WHERE x.version_id=? AND y.version_id=? AND x.hash<>y.hash"
        val movedSql = "FROM version_records x JOIN version_records y ON x.record_id=y.record_id WHERE x.version_id=? AND y.version_id=? AND COALESCE(x.split,'')<>COALESCE(y.split,'')"
        // removed = in A but not in B: swap arguments.
        val removedIds = JSONArray()
        w.rawQuery("SELECT record_id $removedSql LIMIT 50", arrayOf(a, b)).use { c -> while (c.moveToNext()) removedIds.put(c.getString(0)) }
        val removedCount = w.rawQuery("SELECT COUNT(*) $removedSql", arrayOf(a, b)).use { c -> c.moveToFirst(); c.getInt(0) }
        return JSONObject()
            .put("added", cnt("SELECT COUNT(*) $addedSql")).put("added_ids", ids("SELECT record_id $addedSql"))
            .put("removed", removedCount).put("removed_ids", removedIds)
            .put("modified", cnt("SELECT COUNT(*) $modSql")).put("modified_ids", ids("SELECT x.record_id $modSql"))
            .put("split_changed", cnt("SELECT COUNT(*) $movedSql"))
    }

    /** Restores a snapshot in place. A safety snapshot of the current state is taken first. */
    fun restore(ds: String, versionId: String) {
        snapshot(ds, "auto: before restore")
        w.beginTransaction()
        try {
            w.execSQL("DELETE FROM records_fts WHERE docid IN (SELECT seq FROM records WHERE dataset_id=?)", arrayOf(ds))
            w.execSQL("DELETE FROM records WHERE dataset_id=?", arrayOf(ds))
            val batch = ArrayList<Array<String?>>()
            fun flush() {
                if (batch.isEmpty()) return
                val datas = batch.map { JSONObject(it[1]!!) }
                val nm = norms(datas)
                batch.forEachIndexed { i, r ->
                    val cv = ContentValues()
                    cv.put("id", r[0]); cv.put("dataset_id", ds); cv.put("data", r[1]); cv.put("split", r[2]); cv.put("origin", r[3])
                    cv.put("source", r[4]); cv.put("license", r[5]); cv.put("label", r[6]); cv.put("grp", r[7])
                    cv.put("created", now()); cv.put("updated", now())
                    val seq = w.insertOrThrow("records", null, cv)
                    w.execSQL("INSERT INTO records_fts(docid, body) VALUES(?,?)", arrayOf(seq, nm[i]))
                }
                batch.clear()
            }
            w.rawQuery(
                "SELECT vr.record_id, b.data, vr.split, vr.origin, vr.source, vr.license, vr.label, vr.grp FROM version_records vr JOIN blobs b ON b.hash=vr.hash WHERE vr.version_id=?",
                arrayOf(versionId)
            ).use { c ->
                while (c.moveToNext()) {
                    batch.add(Array(8) { c.getString(it) })
                    if (batch.size >= 500) flush()
                }
            }
            flush()
            logDs(ds, "restore", "restored version $versionId")
            w.setTransactionSuccessful()
        } finally {
            w.endTransaction()
        }
    }

    // ---------- backup / restore ----------
    /** Writes a consistent copy of the whole workspace and verifies it before reporting success. */
    fun backupTo(out: OutputStream, cacheDir: File): JSONObject {
        val tmp = File(cacheDir, "backup-${now()}.db")
        try {
            w.execSQL("VACUUM INTO '${tmp.absolutePath.replace("'", "''")}'")
            val check = verifyBackup(tmp)
            tmp.inputStream().use { it.copyTo(out) }
            out.flush()
            return check.put("bytes", tmp.length())
        } finally {
            tmp.delete()
        }
    }

    fun verifyBackup(f: File): JSONObject {
        val db = SQLiteDatabase.openDatabase(f.absolutePath, null, SQLiteDatabase.OPEN_READONLY)
        try {
            val integrity = db.rawQuery("PRAGMA integrity_check", null).use { c -> c.moveToFirst(); c.getString(0) }
            if (integrity != "ok") throw IllegalStateException("Backup failed integrity check: $integrity")
            val ver = db.rawQuery("PRAGMA user_version", null).use { c -> c.moveToFirst(); c.getInt(0) }
            if (ver > DB_VERSION) throw IllegalStateException("Backup was made by a newer app version ($ver)")
            fun cnt(t: String) = db.rawQuery("SELECT COUNT(*) FROM $t", null).use { c -> c.moveToFirst(); c.getInt(0) }
            return JSONObject().put("integrity", integrity).put("datasets", cnt("datasets")).put("records", cnt("records")).put("versions", cnt("versions"))
        } finally {
            db.close()
        }
    }

    /** Validates the file first; only then replaces the live database (previous one is kept as .pre-restore). */
    fun restoreFrom(input: InputStream, cacheDir: File): JSONObject {
        val tmp = File(cacheDir, "restore-${now()}.db")
        input.use { i -> tmp.outputStream().use { o -> i.copyTo(o) } }
        val info = try { verifyBackup(tmp) } catch (e: Exception) { tmp.delete(); throw e }
        val live = helper.dbFile()
        helper.close()
        val safety = File(live.path + ".pre-restore")
        live.copyTo(safety, overwrite = true)
        File(live.path + "-wal").delete(); File(live.path + "-shm").delete()
        tmp.copyTo(live, overwrite = true)
        tmp.delete()
        return info
    }
}
