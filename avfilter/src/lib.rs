pub use avfilter_sys as sys;

mod error;
pub mod formats;
mod frame;
mod options;

pub use error::{AvError, Error};
pub use frame::{Frame, Metadata};
pub use options::FilterOptions;

pub type Rational = sys::AVRational;

pub fn rational(num: i32, den: i32) -> Rational {
    Rational { num, den }
}

/// Rescale a timestamp between time bases with FFmpeg's rounding (nearest).
pub fn rescale(ts: i64, from: Rational, to: Rational) -> i64 {
    unsafe { sys::av_rescale_q(ts, from, to) }
}
