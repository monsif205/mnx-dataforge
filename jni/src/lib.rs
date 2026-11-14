//! JNI bridge: com.monex.dataforge.core.NativeCore.call(op, input) -> String.
//! Panics are caught and returned as {"ok":false,...}; they never cross the FFI boundary.

use jni::objects::{JClass, JString};
use jni::sys::jstring;
use jni::JNIEnv;
use std::panic::{catch_unwind, AssertUnwindSafe};

fn error_json(msg: &str) -> String {
    let mut s = String::from("{\"ok\":false,\"error\":");
    dataforge_core::json::escape_into(msg, &mut s);
    s.push('}');
    s
}

#[no_mangle]
pub extern "system" fn Java_com_monex_dataforge_core_NativeCore_call<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    op: JString<'l>,
    input: JString<'l>,
) -> jstring {
    let op_s: Result<String, _> = env.get_string(&op).map(|s| s.into());
    let in_s: Result<String, _> = env.get_string(&input).map(|s| s.into());
    let out = match (op_s, in_s) {
        (Ok(o), Ok(i)) => catch_unwind(AssertUnwindSafe(|| dataforge_core::api::call(&o, &i)))
            .unwrap_or_else(|_| error_json("internal error (panic) in native core")),
        _ => error_json("could not read arguments"),
    };
    match env.new_string(out) {
        Ok(s) => s.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}
