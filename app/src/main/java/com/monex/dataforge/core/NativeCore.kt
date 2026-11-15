package com.monex.dataforge.core

import org.json.JSONObject

class CoreException(message: String) : Exception(message)

/** Bridge to the Rust engine (libdataforge_jni.so). One JSON-in / JSON-out entry point. */
object NativeCore {
    init {
        System.loadLibrary("dataforge_jni")
    }

    @JvmStatic
    external fun call(op: String, input: String): String

    fun run(op: String, input: JSONObject): JSONObject {
        val out = JSONObject(call(op, input.toString()))
        if (!out.optBoolean("ok", false)) throw CoreException(out.optString("error", "unknown native error"))
        return out.getJSONObject("result")
    }

    /** Indented JSON produced by the engine: keeps key order and Unicode, never escapes "/". */
    fun pretty(json: String, indent: Int = 2): String =
        run("pretty", JSONObject().put("text", json).put("indent", indent)).getString("text")

    /** One-line list preview of a record (conversation -> first user message). */
    fun preview(data: String, max: Int = 140): String =
        run("preview", JSONObject().put("datas", org.json.JSONArray().put(data)).put("max", max)).getJSONArray("previews").getString(0)
}
