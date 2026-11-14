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
}
