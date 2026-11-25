pub use avfilter_sys as sys;

mod error;
pub mod formats;
mod frame;
mod graph;
mod info;
mod options;
mod probe;

pub use error::{AvError, Error};
pub use frame::{Frame, Metadata};
pub use graph::{Graph, Pulled, VideoParams};
pub use info::{FilterInfo, library_is_gpl};
pub use options::FilterOptions;
pub use probe::probe_video_formats;

pub type Rational = sys::AVRational;

pub fn rational(num: i32, den: i32) -> Rational {
    Rational { num, den }
}

/// Rescale a timestamp between time bases with FFmpeg's rounding (nearest).
pub fn rescale(ts: i64, from: Rational, to: Rational) -> i64 {
    unsafe { sys::av_rescale_q(ts, from, to) }
}
