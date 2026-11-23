mod common;

use gst::prelude::*;

fn info() -> gst_video::VideoInfo {
    gst_video::VideoInfo::builder(gst_video::VideoFormat::I420, 64, 64).fps(gst::Fraction::new(25, 1)).build().unwrap()
}

#[test]
fn posts_one_message_per_frame_and_drains() {
    common::init();
    let mut h = gst_check::Harness::new("avfilter_idet");
    let bus = gst::Bus::new();
    h.element().unwrap().set_bus(Some(&bus)); // harness elements have no bus by default
    h.set_src_caps_str("video/x-raw,format=I420,width=64,height=64,framerate=25/1");
    let info = info();

    for n in 0..10 {
        h.push(common::video_buffer(&info, n)).unwrap();
    }
    h.push_event(gst::event::Eos::new());

    let mut pts = Vec::new();
    while let Some(buf) = h.try_pull() {
        pts.push(buf.pts().unwrap());
    }
    assert_eq!(pts.len(), 10, "idet must output every frame, including the one held at EOS");
    assert!(pts.windows(2).all(|w| w[0] < w[1]), "timestamps must increase: {pts:?}");

    let msgs: Vec<_> = std::iter::from_fn(|| bus.pop_filtered(&[gst::MessageType::Element]))
        .filter(|m| m.structure().is_some_and(|s| s.name() == "avfilter-idet"))
        .collect();
    assert_eq!(msgs.len(), 10);
    let s = msgs[0].structure().unwrap();
    assert!(["tff", "bff", "progressive", "undetermined"].contains(&s.get::<&str>("single").unwrap()));
    assert!(s.get::<f64>("multiple-progressive").is_ok());
}

#[test]
fn post_messages_false_posts_nothing() {
    common::init();
    let mut h = gst_check::Harness::new("avfilter_idet");
    let bus = gst::Bus::new();
    let element = h.element().unwrap();
    element.set_bus(Some(&bus));
    element.set_property("post-messages", false);
    h.set_src_caps_str("video/x-raw,format=I420,width=64,height=64,framerate=25/1");
    let info = info();
    for n in 0..5 {
        h.push(common::video_buffer(&info, n)).unwrap();
    }
    h.push_event(gst::event::Eos::new());
    while h.try_pull().is_some() {}
    let n = std::iter::from_fn(|| bus.pop_filtered(&[gst::MessageType::Element])).count();
    assert_eq!(n, 0);
}

#[test]
fn set_field_flags_outputs_mixed_caps() {
    common::init();
    let mut h = gst_check::Harness::new("avfilter_idet");
    h.element().unwrap().set_property("set-field-flags", true);
    h.set_src_caps_str("video/x-raw,format=I420,width=64,height=64,framerate=25/1");
    let info = info();
    for n in 0..3 {
        h.push(common::video_buffer(&info, n)).unwrap();
    }
    h.push_event(gst::event::Eos::new());
    while h.try_pull().is_some() {}
    let caps = h.sinkpad().unwrap().current_caps().unwrap();
    assert_eq!(caps.structure(0).unwrap().get::<&str>("interlace-mode").unwrap(), "mixed");
}

#[test]
fn analyze_interlaced_flag_one_is_bumped_to_two() {
    common::init();
    let e = gst::ElementFactory::make("avfilter_idet").build().unwrap();
    e.set_property("analyze-interlaced-flag", 1u32);
    assert_eq!(e.property::<u32>("analyze-interlaced-flag"), 2);
}

#[test]
fn reports_latency_of_one_frame() {
    common::init();
    let mut h = gst_check::Harness::new("avfilter_idet");
    h.set_src_caps_str("video/x-raw,format=I420,width=64,height=64,framerate=25/1");
    h.push(common::video_buffer(&info(), 0)).unwrap();
    let mut q = gst::query::Latency::new();
    assert!(h.sinkpad().unwrap().peer_query(&mut q));
    let (_, min, _) = q.result();
    assert!(min >= gst::ClockTime::from_mseconds(40), "latency {min}");
}
