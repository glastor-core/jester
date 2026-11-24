//! Official Zero-Node Vue SFC (`.vue`) WASM plugin for `zerho` (`zerho:plugin@0.1.0`).
//!
//! Compiles `<template>`, `<script>` / `<script setup>`, and `<style>` blocks into
//! a standalone ESM component module without Node.js or `@vue/compiler-sfc`.
//!
//! Build: `cargo build --release --target wasm32-unknown-unknown`

use std::alloc::{Layout, alloc};
use std::slice;

#[unsafe(no_mangle)]
pub extern "C" fn zerho_abi_version() -> i32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn zerho_alloc(len: i32) -> i32 {
    let Ok(size) = usize::try_from(len) else { return 0 };
    if size == 0 { return 0; }
    let Ok(layout) = Layout::from_size_align(size, 1) else { return 0 };
    // SAFETY: size > 0, alignment 1.
    unsafe { alloc(layout) as usize as i32 }
}

fn extract_block<'a>(src: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let start_tag = src.find(&open)?;
    let content_start = src[start_tag..].find('>')? + start_tag + 1;
    let end_tag = src[content_start..].find(&close)? + content_start;
    Some(src[content_start..end_tag].trim())
}

fn escape_js_template(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace("${", "\\${")
}

#[unsafe(no_mangle)]
pub extern "C" fn zerho_transform(ptr: i32, len: i32) -> i64 {
    let (Ok(addr), Ok(size)) = (usize::try_from(ptr), usize::try_from(len)) else {
        return 0;
    };
    if addr == 0 || size == 0 {
        return 0;
    }
    // SAFETY: host wrote `size` valid UTF-8 bytes allocated via `zerho_alloc`.
    let input = unsafe { slice::from_raw_parts(addr as *const u8, size) };
    let Ok(source) = std::str::from_utf8(input) else {
        return 0;
    };
    if !source.contains("<template") {
        return 0;
    }

    let template = extract_block(source, "template").unwrap_or("");
    let script = extract_block(source, "script").unwrap_or("export default {};");
    let style = extract_block(source, "style");

    let mut out = String::with_capacity(source.len() + 256);
    out.push_str("/* compiled by zerho-plugin-vue-sfc (WASM / Zero-Node) */\n");
    if let Some(css) = style {
        out.push_str("if (typeof document !== 'undefined') {\n  const __s = document.createElement('style');\n  __s.textContent = `");
        out.push_str(&escape_js_template(css));
        out.push_str("`;\n  document.head.appendChild(__s);\n}\n");
    }
    let normalized_script = script.replacen("export default", "const __sfc__ =", 1);
    if normalized_script == script {
        out.push_str(script);
        out.push_str("\nconst __sfc__ = {};\n");
    } else {
        out.push_str(&normalized_script);
        out.push('\n');
    }
    out.push_str("__sfc__.template = `");
    out.push_str(&escape_js_template(template));
    out.push_str("`;\nexport default __sfc__;\n");

    let bytes = out.into_bytes().into_boxed_slice();
    let out_len = bytes.len() as i64;
    let out_ptr = Box::into_raw(bytes).cast::<u8>() as usize as i64;
    (out_ptr << 32) | (out_len & 0xFFFF_FFFF)
}
