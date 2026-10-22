use crate::{FilterOptions, Graph, VideoParams, rational, sys};

/// Silences av_log while alive and restores the previous level on drop (RAII guard).
struct QuietLog(i32);

impl QuietLog {
    fn new() -> Self {
        let old = unsafe { sys::av_log_get_level() };
        unsafe { sys::av_log_set_level(sys::AV_LOG_QUIET) };
        QuietLog(old)
    }
}

impl Drop for QuietLog {
    fn drop(&mut self) {
        unsafe { sys::av_log_set_level(self.0) };
    }
}

pub fn probe_video_formats(
    filter: &str,
    opts: &FilterOptions,
    candidates: &[sys::AVPixelFormat],
) -> Vec<sys::AVPixelFormat> {
    let _quiet = QuietLog::new();
    candidates
        .iter()
        .copied()
        .filter(|&format| {
            let params = VideoParams {
                format,
                width: 64,
                height: 64,
                time_base: rational(1, 25),
                frame_rate: rational(25, 1),
                sample_aspect_ratio: rational(1, 1),
            };
            Graph::video(&params, filter, opts, 1).is_ok()
        })
        .collect()
}
