//! Raw FFI bindings to FFmpeg's libavfilter and libavutil. Everything here is `unsafe`.
#![allow(non_upper_case_globals, non_camel_case_types, non_snake_case, clippy::all)]
// bindgen derives PartialEq on structs holding C callbacks; we never compare those.
#![allow(unpredictable_function_pointer_comparisons)]

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

const fn mktag(a: u8, b: u8, c: u8, d: u8) -> i32 {
    (a as i32) | ((b as i32) << 8) | ((c as i32) << 16) | ((d as i32) << 24)
}

/// `AVERROR_EOF` = FFERRTAG('E','O','F',' ')
pub const AVERROR_EOF: i32 = -mktag(b'E', b'O', b'F', b' ');
/// `AVERROR(EAGAIN)`
pub const AVERROR_EAGAIN: i32 = -libc::EAGAIN;
/// `AVERROR(EINVAL)`
pub const AVERROR_EINVAL: i32 = -libc::EINVAL;
/// `AV_NOPTS_VALUE` = (int64_t)0x8000000000000000
pub const AV_NOPTS_VALUE: i64 = i64::MIN;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_match_runtime_library() {
        let lavfi = unsafe { avfilter_version() };
        let lavu = unsafe { avutil_version() };
        assert_eq!(lavfi >> 16, LIBAVFILTER_VERSION_MAJOR, "libavfilter headers ≠ runtime library");
        assert_eq!(lavu >> 16, LIBAVUTIL_VERSION_MAJOR, "libavutil headers ≠ runtime library");
    }
}
