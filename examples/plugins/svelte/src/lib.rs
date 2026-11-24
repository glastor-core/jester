//! Official Zero-Node Svelte (`.svelte`) WASM plugin for `zerho` (`zerho:plugin@0.1.0`).
//!
//! Compiles `<script>`, markup template, and `<style>` into a mountable ESM component
//! without Node.js or the JS Svelte compiler.
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

fn strip_tag_block(src: &str, tag: &str) -> (String, Option<String>) {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let Some(start) = src.find(&open) else {
        return (src.to_owned(), None);
    };
    let Some(gt) = src[start..].find('>') else {
        return (src.to_owned(), None);
    };
    let content_start = start + gt + 1;
    let Some(end_rel) = src[content_start..].find(&close) else {
        return (src.to_owned(), None);
    };
    let end = content_start + end_rel;
    let extracted = src[content_start..end].trim().to_owned();
    let mut rest = String::with_capacity(src.len());
    rest.push_str(&src[..start]);
    rest.push_str(&src[end + close.len()..]);
    (rest, Some(extracted))
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
    if !source.contains('<') {
        return 0;
    }

    let (without_script, script) = strip_tag_block(source, "script");
    let (markup_raw, style) = strip_tag_block(&without_script, "style");
    let markup = markup_raw.trim();

    let mut out = String::with_capacity(source.len() + 256);
    out.push_str("/* compiled by zerho-plugin-svelte (WASM / Zero-Node) */\n");
    if let Some(css) = style {
        out.push_str("if (typeof document !== 'undefined') {\n  const __s = document.createElement('style');\n  __s.textContent = `");
        out.push_str(&escape_js_template(&css));
        out.push_str("`;\n  document.head.appendChild(__s);\n}\n");
    }
    out.push_str("export default function SvelteComponent(target) {\n");
    if let Some(js) = script {
        out.push_str(&js);
        out.push('\n');
    }
    out.push_str("  if (target) target.innerHTML = `");
    out.push_str(&escape_js_template(markup));
    out.push_str("`;\n  return { destroy() { if (target) target.innerHTML = ''; } };\n}\n");

    let bytes = out.into_bytes().into_boxed_slice();
    let out_len = bytes.len() as i64;
    let out_ptr = Box::into_raw(bytes).cast::<u8>() as usize as i64;
    (out_ptr << 32) | (out_len & 0xFFFF_FFFF)
}
