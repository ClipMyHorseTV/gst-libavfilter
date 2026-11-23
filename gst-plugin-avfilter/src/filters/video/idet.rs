use crate::base::convert;
use crate::base::video::{AvVideoFilterImpl, av_video_filter_element};
use avfilter::{FilterOptions, Metadata};
use gst::glib;
use gst::prelude::*;
use gst::subclass::prelude::*;
use gst_base::subclass::prelude::*;
use std::sync::{LazyLock, Mutex};

static CAT: LazyLock<gst::DebugCategory> =
    LazyLock::new(|| gst::DebugCategory::new("avfilter_idet", gst::DebugColorFlags::empty(), Some("idet element")));

#[derive(Debug, Clone, Copy)]
struct Settings {
    intl_thres: f32,
    prog_thres: f32,
    rep_thres: f32,
    half_life: f32,
    analyze_interlaced_flag: u32,
    post_messages: bool,
    set_field_flags: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            intl_thres: 1.04,
            prog_thres: 1.5,
            rep_thres: 3.0,
            half_life: 0.0,
            analyze_interlaced_flag: 0,
            post_messages: true,
            set_field_flags: false,
        }
    }
}

/// FFmpeg option names this element sets; all must exist in the linked FFmpeg.
pub const OPTIONS: &[&str] = &["intl_thres", "prog_thres", "rep_thres", "half_life", "analyze_interlaced_flag"];

#[derive(Default)]
pub struct Idet {
    settings: Mutex<Settings>,
}

impl ObjectImpl for Idet {
    fn properties() -> &'static [glib::ParamSpec] {
        static PROPERTIES: LazyLock<Vec<glib::ParamSpec>> = LazyLock::new(|| {
            let d = Settings::default();
            let float = |name: &str, nick: &str, blurb: &str, default: f32| {
                glib::ParamSpecFloat::builder(name)
                    .nick(nick)
                    .blurb(blurb)
                    .minimum(-1.0)
                    .maximum(f32::MAX)
                    .default_value(default)
                    .mutable_ready()
                    .build()
            };
            vec![
                float("intl-thres", "Interlacing threshold", "Interlacing threshold", d.intl_thres),
                float("prog-thres", "Progressive threshold", "Progressive threshold", d.prog_thres),
                float("rep-thres", "Repeat threshold", "Repeated field threshold", d.rep_thres),
                float(
                    "half-life",
                    "Half life",
                    "Frames after which a frame's contribution to the statistics is halved (0 = never)",
                    d.half_life,
                ),
                glib::ParamSpecUInt::builder("analyze-interlaced-flag")
                    .nick("Analyze interlaced flag")
                    .blurb(
                        "Only check frames flagged interlaced, for this many frames, then pass through \
                         (0 = off, must be ≥ 2)",
                    )
                    .default_value(d.analyze_interlaced_flag)
                    .mutable_ready()
                    .build(),
                glib::ParamSpecBoolean::builder("post-messages")
                    .nick("Post messages")
                    .blurb("Post an 'avfilter-idet' element message per frame")
                    .default_value(d.post_messages)
                    .mutable_playing()
                    .build(),
                glib::ParamSpecBoolean::builder("set-field-flags")
                    .nick("Set field flags")
                    .blurb("Output interlace-mode=mixed caps and mark each buffer with the detected field order")
                    .default_value(d.set_field_flags)
                    .mutable_ready()
                    .build(),
            ]
        });
        PROPERTIES.as_ref()
    }

    fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
        let mut s = self.settings.lock().unwrap();
        match pspec.name() {
            "intl-thres" => s.intl_thres = value.get().unwrap(),
            "prog-thres" => s.prog_thres = value.get().unwrap(),
            "rep-thres" => s.rep_thres = value.get().unwrap(),
            "half-life" => s.half_life = value.get().unwrap(),
            "analyze-interlaced-flag" => {
                let mut n: u32 = value.get().unwrap();
                if n == 1 {
                    // FFmpeg's completion check is "== 1 after decrement", so 1 never finishes.
                    gst::warning!(CAT, imp = self, "analyze-interlaced-flag=1 never completes in FFmpeg, using 2");
                    n = 2;
                }
                s.analyze_interlaced_flag = n;
            }
            "post-messages" => s.post_messages = value.get().unwrap(),
            "set-field-flags" => s.set_field_flags = value.get().unwrap(),
            _ => unimplemented!(),
        }
    }

    fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
        let s = self.settings.lock().unwrap();
        match pspec.name() {
            "intl-thres" => s.intl_thres.to_value(),
            "prog-thres" => s.prog_thres.to_value(),
            "rep-thres" => s.rep_thres.to_value(),
            "half-life" => s.half_life.to_value(),
            "analyze-interlaced-flag" => s.analyze_interlaced_flag.to_value(),
            "post-messages" => s.post_messages.to_value(),
            "set-field-flags" => s.set_field_flags.to_value(),
            _ => unimplemented!(),
        }
    }
}

impl AvVideoFilterImpl for Idet {
    const FILTER: &'static str = "idet";

    fn options(&self) -> FilterOptions {
        let s = *self.settings.lock().unwrap();
        FilterOptions::new()
            .double("intl_thres", s.intl_thres as f64)
            .double("prog_thres", s.prog_thres as f64)
            .double("rep_thres", s.rep_thres as f64)
            .double("half_life", s.half_life as f64)
            .int("analyze_interlaced_flag", s.analyze_interlaced_flag as i64)
    }

    fn map_caps(&self, direction: gst::PadDirection, caps: &gst::Caps) -> gst::Caps {
        if caps.is_any() || !self.settings.lock().unwrap().set_field_flags {
            return caps.clone();
        }
        let mut out = caps.clone();
        for st in out.make_mut().iter_mut() {
            st.remove_field("field-order");
            match direction {
                // Downstream gets per-buffer field info.
                gst::PadDirection::Sink => st.set("interlace-mode", "mixed"),
                // Upstream may be anything (progressive, interleaved, mixed).
                _ => st.remove_field("interlace-mode"),
            }
        }
        out
    }

    fn latency(&self, info: &gst_video::VideoInfo) -> gst::ClockTime {
        convert::frame_duration(info).unwrap_or(gst::ClockTime::ZERO) // one frame held back
    }

    fn handle_metadata(&self, meta: &Metadata, pts: Option<gst::ClockTime>) {
        if !self.settings.lock().unwrap().post_messages {
            return;
        }
        let key = |k: &str| format!("lavfi.idet.{k}");
        let text = |k: &str, default: &str| meta.get(&key(k)).unwrap_or(default).to_owned();
        let num = |k: &str| meta.get(&key(k)).and_then(|v| v.trim().parse::<f64>().ok()).unwrap_or(0.0);

        let s = gst::Structure::builder("avfilter-idet")
            .field_if_some("timestamp", pts)
            .field("single", text("single.current_frame", "undetermined"))
            .field("multiple", text("multiple.current_frame", "undetermined"))
            .field("repeated", text("repeated.current_frame", "neither"))
            .field("single-tff", num("single.tff"))
            .field("single-bff", num("single.bff"))
            .field("single-progressive", num("single.progressive"))
            .field("single-undetermined", num("single.undetermined"))
            .field("multiple-tff", num("multiple.tff"))
            .field("multiple-bff", num("multiple.bff"))
            .field("multiple-progressive", num("multiple.progressive"))
            .field("multiple-undetermined", num("multiple.undetermined"))
            .field("repeated-neither", num("repeated.neither"))
            .field("repeated-top", num("repeated.top"))
            .field("repeated-bottom", num("repeated.bottom"))
            .build();
        let _ = self.obj().post_message(gst::message::Element::builder(s).src(&*self.obj()).build());
    }
}

av_video_filter_element!(Idet, AvFilterIdet, "avfilter_idet", "GstAvFilterIdet", "Filter/Analyzer/Video");
