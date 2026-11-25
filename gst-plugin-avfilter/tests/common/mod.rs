#![allow(dead_code)] // each test crate uses a different subset

use gst::prelude::*;

pub fn init() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        gst::init().unwrap();
        gstavfilter::plugin_register_static().expect("register avfilter plugin");
    });
}

/// A blank video buffer for `info`, with PTS/duration set for frame `n`.
pub fn video_buffer(info: &gst_video::VideoInfo, n: u64) -> gst::Buffer {
    let dur = gst::ClockTime::SECOND.mul_div_floor(info.fps().denom() as u64, info.fps().numer() as u64).unwrap();
    let mut buf = gst::Buffer::with_size(info.size()).unwrap();
    {
        let b = buf.get_mut().unwrap();
        b.set_pts(dur * n);
        b.set_duration(dur);
    }
    buf
}
