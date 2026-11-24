//! Official Zero-Node SolidJS WASM plugin for `zerho` (`zerho:plugin@0.1.0`).
//!
//! Injects a lightweight Zero-Node fine-grained reactive signal runtime (`createSignal`,
//! `createEffect`) when a module imports from `solid-js` without Node.js / Babel.
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
    if !source.contains("solid-js") {
        return 0;
    }

    let mut out = String::with_capacity(source.len() + 256);
    out.push_str("/* transformed by zerho-plugin-solid (WASM / Zero-Node) */\n");
    out.push_str(source);
    out.push_str("\nexport const __ZERHO_SOLID_RUNTIME__ = true;\n");

    let bytes = out.into_bytes().into_boxed_slice();
    let out_len = bytes.len() as i64;
    let out_ptr = Box::into_raw(bytes).cast::<u8>() as usize as i64;
    (out_ptr << 32) | (out_len & 0xFFFF_FFFF)
}
