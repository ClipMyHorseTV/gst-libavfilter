use crate::sys;
use std::ffi::{CStr, c_char, c_int};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AvError(pub c_int);

impl fmt::Display for AvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut buf = [0 as c_char; 128];
        let msg = unsafe {
            sys::av_strerror(self.0, buf.as_mut_ptr(), buf.len());
            CStr::from_ptr(buf.as_ptr())
        };
        write!(f, "{} ({})", msg.to_string_lossy(), self.0)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{op} failed: {code}")]
    Av { op: &'static str, code: AvError },
    #[error("filter '{0}' is not available in this FFmpeg build")]
    FilterNotFound(String),
    #[error("option '{name}' rejected: {code}")]
    Option { name: String, code: AvError },
    #[error("plane {0} does not exist for this pixel format")]
    BadPlane(usize),
    #[error("buffer too small for plane data")]
    BufferTooSmall,
    #[error("string contains a NUL byte")]
    Nul(#[from] std::ffi::NulError),
    #[error("out of memory")]
    OutOfMemory,
}

pub(crate) fn check(op: &'static str, ret: c_int) -> Result<c_int, Error> {
    if ret < 0 {
        Err(Error::Av {
            op,
            code: AvError(ret),
        })
    } else {
        Ok(ret)
    }
}
