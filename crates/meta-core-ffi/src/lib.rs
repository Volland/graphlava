//! C FFI for meta-core. Builds as `libmeta_core_ffi` (cdylib and staticlib) for
//! desktop, iOS and Android hosts.

use std::ffi::{CStr, c_char};

static VERSION: &CStr = {
    // Keep in sync with the workspace version; checked by the test below.
    match CStr::from_bytes_with_nul(concat!(env!("CARGO_PKG_VERSION"), "\0").as_bytes()) {
        Ok(s) => s,
        Err(_) => panic!("version contains a NUL byte"),
    }
};

/// Returns the library version as a static, NUL-terminated string.
/// The caller must not free it.
#[unsafe(no_mangle)]
pub extern "C" fn meta_core_version() -> *const c_char {
    VERSION.as_ptr()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_matches_core() {
        // SAFETY: meta_core_version returns a pointer to a static C string.
        let v = unsafe { CStr::from_ptr(meta_core_version()) };
        assert_eq!(v.to_str().unwrap(), meta_core::VERSION);
    }
}
