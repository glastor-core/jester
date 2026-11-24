//! Example Rust -> WebAssembly plugin for `zerho` (`zerho:plugin@0.1.0` linear-memory ABI).
//!
//! Build with:
//!   rustup target add wasm32-unknown-unknown
//!   cargo build --release --target wasm32-unknown-unknown
//!
//! Then reference in `zerho.toml`:
//!   [plugins]
//!   wasm = ["target/wasm32-unknown-unknown/release/zerho_example_wasm_plugin.wasm"]
//!   fuel_limit = 1000000

use std::alloc::{Layout, alloc};
use std::slice;

/// Returns the supported zerho WASM plugin ABI version (`1`).
#[unsafe(no_mangle)]
pub extern "C" fn zerho_abi_version() -> i32 {
    1
}

/// Allocates `len` bytes in guest linear memory so the `zerho` host can pass UTF-8 source strings.
#[unsafe(no_mangle)]
pub extern "C" fn zerho_alloc(len: i32) -> i32 {
    let Ok(size) = usize::try_from(len) else {
        return 0;
    };
    if size == 0 {
        return 0;
    }
    let Ok(layout) = Layout::from_size_align(size, 1) else {
        return 0;
    };
    // SAFETY: `size > 0` and alignment is 1.
    let ptr = unsafe { alloc(layout) };
    ptr as usize as i32
}

/// Transforms a UTF-8 module source buffer and returns a packed `i64`: `(out_ptr << 32) | out_len`.
/// Returning `0` signals "no modification".
#[unsafe(no_mangle)]
pub extern "C" fn zerho_transform(ptr: i32, len: i32) -> i64 {
    let (Ok(addr), Ok(size)) = (usize::try_from(ptr), usize::try_from(len)) else {
        return 0;
    };
    if addr == 0 || size == 0 {
        return 0;
    }
    // SAFETY: host wrote `size` valid UTF-8 bytes at `addr` allocated via `zerho_alloc`.
    let input = unsafe { slice::from_raw_parts(addr as *const u8, size) };
    let Ok(source) = std::str::from_utf8(input) else {
        return 0;
    };

    let mut output = String::with_capacity(source.len() + 40);
    output.push_str("/* transformed by zerho wasm plugin */\n");
    output.push_str(source);

    let bytes = output.into_bytes().into_boxed_slice();
    let out_len = bytes.len() as i64;
    let out_ptr = Box::into_raw(bytes).cast::<u8>() as usize as i64;
    (out_ptr << 32) | (out_len & 0xFFFF_FFFF)
}
