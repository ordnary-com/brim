//! The functions JavaScript calls. Strings cross as UTF-8 bytes in the
//! module's memory: the page asks for a buffer with `brim_alloc`, writes the
//! source and input into it and calls `brim_run`, which returns a buffer
//! holding a 4-byte little-endian length and the result as JSON. The page
//! frees every buffer with `brim_free`.

/// A buffer of `len` bytes for the page to fill.
#[unsafe(no_mangle)]
pub extern "C" fn brim_alloc(len: usize) -> *mut u8 {
    let mut buffer = vec![0u8; len.max(1)].into_boxed_slice();
    let ptr = buffer.as_mut_ptr();
    std::mem::forget(buffer);
    ptr
}

/// Frees a buffer from `brim_alloc` or `brim_run`.
///
/// # Safety
///
/// `ptr` and `len` must describe a buffer this module handed out and not
/// freed yet.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brim_free(ptr: *mut u8, len: usize) {
    // SAFETY: the caller passes a buffer from `brim_alloc` or `brim_run`,
    // which are boxed slices of exactly `len.max(1)` bytes.
    drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len.max(1))) });
}

/// Runs a program; see the module documentation for the result.
///
/// # Safety
///
/// The pointers and lengths must describe buffers from `brim_alloc` that
/// hold the source and the input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brim_run(
    source: *const u8,
    source_len: usize,
    input: *const u8,
    input_len: usize,
) -> *mut u8 {
    // SAFETY: the caller passes initialised buffers of these lengths.
    let (source, input) = unsafe {
        (
            std::slice::from_raw_parts(source, source_len),
            std::slice::from_raw_parts(input, input_len),
        )
    };
    let json = super::run_json(
        &String::from_utf8_lossy(source),
        &String::from_utf8_lossy(input),
        super::MAX_STEPS,
    );
    let mut out = (json.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(json.as_bytes());
    let mut buffer = out.into_boxed_slice();
    let ptr = buffer.as_mut_ptr();
    std::mem::forget(buffer);
    ptr
}
