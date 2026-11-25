mod common;

use gst::prelude::*;

fn i420(w: u32, h: u32, fps: i32) -> gst_video::VideoInfo {
    gst_video::VideoInfo::builder(gst_video::VideoFormat::I420, w, h).fps(gst::Fraction::new(fps, 1)).build().unwrap()
}

#[test]
fn doubles_frame_rate_and_keeps_the_tail() {
    common::init();
    let mut h = gst_check::Harness::new_parse("avfilter_minterpolate fps=50/1 mi-mode=blend");
    h.set_src_caps_str("video/x-raw,format=I420,width=64,height=64,framerate=25/1");
    let info = i420(64, 64, 25);

    for n in 0..25 {
        h.push(common::video_buffer(&info, n)).unwrap();
    }
    h.push_event(gst::event::Eos::new());

    let mut pts = Vec::new();
    while let Some(b) = h.try_pull() {
        pts.push(b.pts().unwrap());
    }
    // 1 s in → 1 s out at 50 fps, evenly spaced, starting at 0.
    assert_eq!(pts.len(), 50, "{pts:?}");
    assert_eq!(pts[0], gst::ClockTime::ZERO);
    assert!(pts.windows(2).all(|w| w[1] - w[0] == gst::ClockTime::from_mseconds(20)), "{pts:?}");
    // Tail padding must not leak frames past the end of the input (1 s).
    assert!(pts.iter().all(|&p| p < gst::ClockTime::SECOND), "{pts:?}");
}

#[test]
fn src_caps_carry_the_configured_frame_rate() {
    common::init();
    let mut h = gst_check::Harness::new_parse("avfilter_minterpolate fps=50/1 mi-mode=blend");
    h.set_src_caps_str("video/x-raw,format=I420,width=64,height=64,framerate=25/1");
    let info = i420(64, 64, 25);
    for n in 0..4 {
        h.push(common::video_buffer(&info, n)).unwrap();
    }
    let caps = h.sinkpad().unwrap().current_caps().expect("negotiated");
    let s = caps.structure(0).unwrap();
    assert_eq!(s.get::<gst::Fraction>("framerate").unwrap(), gst::Fraction::new(50, 1));
}

#[test]
fn reports_two_input_frames_of_latency() {
    common::init();
    let mut h = gst_check::Harness::new_parse("avfilter_minterpolate fps=50/1 mi-mode=blend");
    h.set_src_caps_str("video/x-raw,format=I420,width=64,height=64,framerate=25/1");
    let info = i420(64, 64, 25);
    h.push(common::video_buffer(&info, 0)).unwrap();

    let mut q = gst::query::Latency::new();
    assert!(h.sinkpad().unwrap().peer_query(&mut q));
    let (_, min, _) = q.result();
    assert_eq!(min, gst::ClockTime::from_mseconds(80)); // 2 × 40 ms
}

#[test]
fn rejects_tiny_frames_in_mci_mode() {
    common::init();
    let mut h = gst_check::Harness::new_parse("avfilter_minterpolate mi-mode=mci mb-size=16");
    h.set_src_caps_str("video/x-raw,format=I420,width=16,height=16,framerate=25/1");
    let info = i420(16, 16, 25);
    assert_eq!(h.push(common::video_buffer(&info, 0)), Err(gst::FlowError::NotNegotiated));
}
