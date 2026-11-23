//! Base-class conformance: every element listed in the registry and present in this FFmpeg
//! build must pass these, so new filters are covered automatically.
mod common;

use gst::glib;
use gst::prelude::*;

/// Elements registered in this FFmpeg build.
fn elements() -> Vec<&'static str> {
    common::init();
    gstavfilter::registry::ENTRIES
        .iter()
        .map(|e| e.element)
        .filter(|name| gst::ElementFactory::find(name).is_some())
        .collect()
}

/// Formats listed in the element's sink template.
fn template_formats(name: &str) -> Vec<String> {
    let factory = gst::ElementFactory::find(name).unwrap();
    let tmpl = factory.static_pad_templates().into_iter().find(|t| t.direction() == gst::PadDirection::Sink).unwrap();
    let caps = tmpl.caps();
    let s = caps.structure(0).unwrap();
    // "format" is a list when there are several formats, a plain string when there is one.
    match s.get::<gst::List>("format") {
        Ok(list) => list.iter().map(|v| v.get::<String>().unwrap()).collect(),
        Err(_) => vec![s.get::<String>("format").unwrap()],
    }
}

fn caps_and_info(format: &str, w: u32, h: u32) -> (String, gst_video::VideoInfo) {
    let caps_str = format!("video/x-raw,format={format},width={w},height={h},framerate=25/1");
    let info = gst_video::VideoInfo::from_caps(&caps_str.parse::<gst::Caps>().unwrap()).unwrap();
    (caps_str, info)
}

/// Pull everything currently queued on the harness sink.
fn pull_all(h: &mut gst_check::Harness) -> Vec<gst::Buffer> {
    std::iter::from_fn(|| h.try_pull()).collect()
}

#[test]
fn at_least_one_element_registered() {
    assert!(!elements().is_empty(), "no avfilter elements registered; is FFmpeg missing every filter?");
}

#[test]
fn state_changes() {
    for name in elements() {
        let e = gst::ElementFactory::make(name).build().unwrap();
        for s in [
            gst::State::Ready,
            gst::State::Paused,
            gst::State::Playing,
            gst::State::Paused,
            gst::State::Ready,
            gst::State::Null,
        ] {
            e.set_state(s).unwrap_or_else(|_| panic!("{name}: → {s:?} failed"));
        }
    }
}

#[test]
fn every_template_format_negotiates_and_drains() {
    for name in elements() {
        for f in template_formats(name) {
            let (caps_str, info) = caps_and_info(&f, 64, 64);
            let mut h = gst_check::Harness::new(name);
            h.set_src_caps_str(&caps_str);
            for n in 0..8 {
                h.push(common::video_buffer(&info, n)).unwrap_or_else(|e| panic!("{name}/{f}: {e:?}"));
            }
            h.push_event(gst::event::Eos::new());
            let mut out = 0;
            let mut last = None;
            while let Some(b) = h.try_pull() {
                let pts = b.pts().unwrap_or_else(|| panic!("{name}/{f}: output without PTS"));
                assert!(last.is_none_or(|l| pts > l), "{name}/{f}: timestamps not increasing");
                last = Some(pts);
                out += 1;
            }
            assert!(out > 0, "{name}/{f}: no output after EOS");
        }
    }
}

#[test]
fn property_defaults_round_trip() {
    for name in elements() {
        let e = gst::ElementFactory::make(name).build().unwrap();
        // GstObject's `name`/`parent` aren't ours, and a NULL name is replaced by a generated one.
        let ours = |p: &&glib::ParamSpec| {
            p.flags().contains(glib::ParamFlags::READWRITE) && p.owner_type() != gst::Object::static_type()
        };
        for p in e.list_properties().iter().filter(ours) {
            let def = p.default_value().clone();
            e.set_property_from_value(p.name(), &def);
            let got = e.property_value(p.name());
            assert_eq!(got.serialize().ok(), def.serialize().ok(), "{name}.{}", p.name());
        }
    }
}

#[test]
fn every_listed_filter_is_registered_when_ffmpeg_has_it() {
    common::init();
    for e in gstavfilter::registry::ENTRIES {
        if avfilter::FilterInfo::lookup(e.filter).is_some() {
            assert!(
                gst::ElementFactory::find(e.element).is_some(),
                "{}: FFmpeg has '{}' but the element wasn't registered — option drift? {:?}",
                e.element,
                e.filter,
                e.options
            );
        }
    }
}

/// Every option an Entry declares must be found by `has_option` (a broken lookup either
/// segfaults or silently skips registration). The control lookups keep this test meaningful
/// even while no registered Entry declares options.
#[test]
fn entry_options_exist_in_ffmpeg() {
    common::init();
    for (filter, opt) in [("hflip", "no_such_option"), ("idet", "intl_thres"), ("minterpolate", "fps")] {
        if let Some(info) = avfilter::FilterInfo::lookup(filter) {
            assert_eq!(info.has_option(opt), opt != "no_such_option", "control {filter}.{opt}");
        }
    }
    for e in gstavfilter::registry::ENTRIES {
        let Some(info) = avfilter::FilterInfo::lookup(e.filter) else { continue };
        for opt in e.options {
            assert!(info.has_option(opt), "{}: FFmpeg '{}' has no option '{opt}'", e.element, e.filter);
        }
    }
}

/// FLUSH (seek): nothing pushed before the flush may come out after it.
#[test]
fn flush_discards_pending_frames() {
    for name in elements() {
        let (caps_str, info) = caps_and_info("I420", 64, 64);
        let mut h = gst_check::Harness::new(name);
        h.set_src_caps_str(&caps_str);
        for n in 0..5 {
            h.push(common::video_buffer(&info, n)).unwrap();
        }
        pull_all(&mut h);

        h.push_event(gst::event::FlushStart::new());
        h.push_event(gst::event::FlushStop::new(true));
        h.push_event(gst::event::Segment::new(&gst::FormattedSegment::<gst::ClockTime>::new()));

        // The post-flush stream restarts at 10 s; anything earlier leaked from before the flush.
        let restart = 250;
        for n in restart..restart + 5 {
            h.push(common::video_buffer(&info, n)).unwrap();
        }
        h.push_event(gst::event::Eos::new());
        let first_new = common::video_buffer(&info, restart).pts().unwrap();
        let out = pull_all(&mut h);
        assert!(!out.is_empty(), "{name}: no output after flush");
        for b in out {
            let pts = b.pts().unwrap();
            assert!(pts >= first_new, "{name}: pre-flush frame {pts} leaked after flush");
        }
    }
}

/// Mid-stream caps change: old-size frames drain out under the old caps, then the new size.
#[test]
fn renegotiation_switches_size_at_boundary() {
    for name in elements() {
        let (small_caps, small) = caps_and_info("I420", 64, 64);
        let (big_caps, big) = caps_and_info("I420", 128, 128);
        let mut h = gst_check::Harness::new(name);
        h.set_src_caps_str(&small_caps);
        for n in 0..6 {
            h.push(common::video_buffer(&small, n)).unwrap();
        }
        let mut sizes: Vec<usize> = pull_all(&mut h).iter().map(|b| b.size()).collect();

        h.set_src_caps_str(&big_caps);
        // The caps event drained the graph: every small frame is out before any big one.
        sizes.extend(pull_all(&mut h).iter().map(|b| b.size()));
        for n in 6..12 {
            h.push(common::video_buffer(&big, n)).unwrap();
        }
        h.push_event(gst::event::Eos::new());
        sizes.extend(pull_all(&mut h).iter().map(|b| b.size()));

        let first_big = sizes
            .iter()
            .position(|&s| s == big.size())
            .unwrap_or_else(|| panic!("{name}: no 128x128 output: {sizes:?}"));
        assert!(first_big > 0, "{name}: no 64x64 output");
        assert!(sizes[..first_big].iter().all(|&s| s == small.size()), "{name}: {sizes:?}");
        assert!(sizes[first_big..].iter().all(|&s| s == big.size()), "{name}: sizes interleaved {sizes:?}");
    }
}

/// A DISCONT input (e.g. after packet loss) drains the old graph and marks the next output DISCONT.
#[test]
fn discont_input_marks_output_discont() {
    for name in elements() {
        let (caps_str, info) = caps_and_info("I420", 64, 64);
        let mut h = gst_check::Harness::new(name);
        h.set_src_caps_str(&caps_str);
        for n in 0..6 {
            h.push(common::video_buffer(&info, n)).unwrap();
        }
        let before = pull_all(&mut h).len();

        let mut gap = common::video_buffer(&info, 20);
        gap.get_mut().unwrap().set_flags(gst::BufferFlags::DISCONT);
        h.push(gap).unwrap();
        for n in 21..26 {
            h.push(common::video_buffer(&info, n)).unwrap();
        }
        h.push_event(gst::event::Eos::new());
        let after = pull_all(&mut h);
        let gap_pts = common::video_buffer(&info, 20).pts().unwrap();
        let first_new = after
            .iter()
            .find(|b| b.pts().unwrap() >= gap_pts)
            .unwrap_or_else(|| panic!("{name}: no output after DISCONT ({before} buffers before)"));
        assert!(first_new.flags().contains(gst::BufferFlags::DISCONT), "{name}: first post-gap buffer not DISCONT");
    }
}
