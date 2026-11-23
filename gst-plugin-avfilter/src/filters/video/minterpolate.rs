use crate::base::convert;
use crate::base::enum_nick;
use crate::base::video::{AvVideoFilterImpl, FilterBehavior, av_video_filter_element};
use avfilter::FilterOptions;
use gst::glib;
use gst::prelude::*;
use gst::subclass::prelude::*;
use gst_base::subclass::prelude::*;
use std::sync::{LazyLock, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, glib::Enum, Default)]
#[repr(i32)]
#[enum_type(name = "GstAvFilterMinterpolateMiMode")]
pub enum MiMode {
    #[enum_value(name = "Duplicate frames", nick = "dup")]
    Dup = 0,
    #[enum_value(name = "Blend frames", nick = "blend")]
    Blend = 1,
    #[default]
    #[enum_value(name = "Motion compensated interpolation", nick = "mci")]
    Mci = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, glib::Enum, Default)]
#[repr(i32)]
#[enum_type(name = "GstAvFilterMinterpolateMcMode")]
pub enum McMode {
    #[default]
    #[enum_value(name = "Overlapped block motion compensation", nick = "obmc")]
    Obmc = 0,
    #[enum_value(name = "Adaptive OBMC", nick = "aobmc")]
    Aobmc = 1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, glib::Enum, Default)]
#[repr(i32)]
#[enum_type(name = "GstAvFilterMinterpolateMeMode")]
pub enum MeMode {
    #[enum_value(name = "Bidirectional", nick = "bidir")]
    Bidir = 0,
    #[default]
    #[enum_value(name = "Bilateral", nick = "bilat")]
    Bilat = 1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, glib::Enum, Default)]
#[repr(i32)]
#[enum_type(name = "GstAvFilterMinterpolateMe")]
pub enum Me {
    #[enum_value(name = "Exhaustive search", nick = "esa")]
    Esa = 1,
    #[enum_value(name = "Three step search", nick = "tss")]
    Tss = 2,
    #[enum_value(name = "Two dimensional logarithmic search", nick = "tdls")]
    Tdls = 3,
    #[enum_value(name = "New three step search", nick = "ntss")]
    Ntss = 4,
    #[enum_value(name = "Four step search", nick = "fss")]
    Fss = 5,
    #[enum_value(name = "Diamond search", nick = "ds")]
    Ds = 6,
    #[enum_value(name = "Hexagon-based search", nick = "hexbs")]
    Hexbs = 7,
    #[default]
    #[enum_value(name = "Enhanced predictive zonal search", nick = "epzs")]
    Epzs = 8,
    #[enum_value(name = "Uneven multi-hexagon search", nick = "umh")]
    Umh = 9,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, glib::Enum, Default)]
#[repr(i32)]
#[enum_type(name = "GstAvFilterMinterpolateScd")]
pub enum Scd {
    #[enum_value(name = "Disabled", nick = "none")]
    None = 0,
    #[default]
    #[enum_value(name = "Frame difference", nick = "fdiff")]
    Fdiff = 1,
}

const MB_SIZE_BLURB: &str = "Macroblock size (frames must be at least 2×mb-size in mci mode)";

#[derive(Debug, Clone, Copy)]
struct Settings {
    fps: gst::Fraction,
    mi_mode: MiMode,
    mc_mode: McMode,
    me_mode: MeMode,
    me: Me,
    mb_size: u32,
    search_param: u32,
    vsbmc: bool,
    scd: Scd,
    scd_threshold: f64,
}

impl Default for Settings {
    fn default() -> Self {
        // Defaults = FFmpeg's defaults.
        Settings {
            fps: gst::Fraction::new(60, 1),
            mi_mode: MiMode::Mci,
            mc_mode: McMode::Obmc,
            me_mode: MeMode::Bilat,
            me: Me::Epzs,
            mb_size: 16,
            search_param: 32,
            vsbmc: false,
            scd: Scd::Fdiff,
            scd_threshold: 10.0,
        }
    }
}

/// FFmpeg options this element sets (checked against the linked FFmpeg at registration).
pub const OPTIONS: &[&str] =
    &["fps", "mi_mode", "mc_mode", "me_mode", "me", "mb_size", "search_param", "vsbmc", "scd", "scd_threshold"];

#[derive(Default)]
pub struct Minterpolate {
    settings: Mutex<Settings>,
}

impl ObjectImpl for Minterpolate {
    fn properties() -> &'static [glib::ParamSpec] {
        static PROPERTIES: LazyLock<Vec<glib::ParamSpec>> = LazyLock::new(|| {
            let d = Settings::default();
            vec![
                gst::ParamSpecFraction::builder("fps")
                    .nick("Frame rate")
                    .blurb("Output frame rate")
                    .minimum(gst::Fraction::new(1, 1))
                    .maximum(gst::Fraction::new(1000, 1))
                    .default_value(d.fps)
                    .mutable_ready()
                    .build(),
                glib::ParamSpecEnum::builder_with_default("mi-mode", d.mi_mode)
                    .nick("Interpolation mode")
                    .blurb("Motion interpolation mode")
                    .mutable_ready()
                    .build(),
                glib::ParamSpecEnum::builder_with_default("mc-mode", d.mc_mode)
                    .nick("Compensation mode")
                    .blurb("Motion compensation mode (mci only)")
                    .mutable_ready()
                    .build(),
                glib::ParamSpecEnum::builder_with_default("me-mode", d.me_mode)
                    .nick("Estimation mode")
                    .blurb("Motion estimation mode (mci only)")
                    .mutable_ready()
                    .build(),
                glib::ParamSpecEnum::builder_with_default("me", d.me)
                    .nick("Estimation algorithm")
                    .blurb("Motion estimation algorithm (mci only)")
                    .mutable_ready()
                    .build(),
                glib::ParamSpecUInt::builder("mb-size")
                    .nick("Block size")
                    .blurb(MB_SIZE_BLURB)
                    .minimum(4)
                    .maximum(16)
                    .default_value(d.mb_size)
                    .mutable_ready()
                    .build(),
                glib::ParamSpecUInt::builder("search-param")
                    .nick("Search parameter")
                    .blurb("Motion estimation search parameter")
                    .minimum(4)
                    .maximum(i32::MAX as u32)
                    .default_value(d.search_param)
                    .mutable_ready()
                    .build(),
                glib::ParamSpecBoolean::builder("vsbmc")
                    .nick("VSBMC")
                    .blurb("Variable-size block motion compensation")
                    .default_value(d.vsbmc)
                    .mutable_ready()
                    .build(),
                glib::ParamSpecEnum::builder_with_default("scd", d.scd)
                    .nick("Scene change detection")
                    .blurb("Scene change detection method")
                    .mutable_ready()
                    .build(),
                glib::ParamSpecDouble::builder("scd-threshold")
                    .nick("Scene change threshold")
                    .blurb("Scene change detection threshold")
                    .minimum(0.0)
                    .maximum(100.0)
                    .default_value(d.scd_threshold)
                    .mutable_ready()
                    .build(),
            ]
        });
        PROPERTIES.as_ref()
    }

    fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
        let mut s = self.settings.lock().unwrap();
        match pspec.name() {
            "fps" => s.fps = value.get().unwrap(),
            "mi-mode" => s.mi_mode = value.get().unwrap(),
            "mc-mode" => s.mc_mode = value.get().unwrap(),
            "me-mode" => s.me_mode = value.get().unwrap(),
            "me" => s.me = value.get().unwrap(),
            "mb-size" => s.mb_size = value.get().unwrap(),
            "search-param" => s.search_param = value.get().unwrap(),
            "vsbmc" => s.vsbmc = value.get().unwrap(),
            "scd" => s.scd = value.get().unwrap(),
            "scd-threshold" => s.scd_threshold = value.get().unwrap(),
            _ => unimplemented!(),
        }
    }

    fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
        let s = self.settings.lock().unwrap();
        match pspec.name() {
            "fps" => s.fps.to_value(),
            "mi-mode" => s.mi_mode.to_value(),
            "mc-mode" => s.mc_mode.to_value(),
            "me-mode" => s.me_mode.to_value(),
            "me" => s.me.to_value(),
            "mb-size" => s.mb_size.to_value(),
            "search-param" => s.search_param.to_value(),
            "vsbmc" => s.vsbmc.to_value(),
            "scd" => s.scd.to_value(),
            "scd-threshold" => s.scd_threshold.to_value(),
            _ => unimplemented!(),
        }
    }
}

impl AvVideoFilterImpl for Minterpolate {
    const FILTER: &'static str = "minterpolate";
    // Temporal filter: QoS must not drop inputs. It doesn't drain at EOF, so pad 2 frames.
    const BEHAVIOR: FilterBehavior = FilterBehavior { qos_safe: false, eos_tail_frames: 2 };

    fn options(&self) -> FilterOptions {
        let s = *self.settings.lock().unwrap();
        FilterOptions::new()
            .rational("fps", s.fps.numer(), s.fps.denom())
            .string("mi_mode", enum_nick(s.mi_mode))
            .string("mc_mode", enum_nick(s.mc_mode))
            .string("me_mode", enum_nick(s.me_mode))
            .string("me", enum_nick(s.me))
            .int("mb_size", s.mb_size as i64)
            .int("search_param", s.search_param as i64)
            .int("vsbmc", s.vsbmc as i64)
            .string("scd", enum_nick(s.scd))
            .double("scd_threshold", s.scd_threshold)
    }

    fn map_caps(&self, direction: gst::PadDirection, caps: &gst::Caps) -> gst::Caps {
        if caps.is_any() {
            return caps.clone();
        }
        let s = *self.settings.lock().unwrap();
        let mut out = caps.clone();
        for st in out.make_mut().iter_mut() {
            match direction {
                // Given our sink caps, our src caps have the configured output rate…
                gst::PadDirection::Sink => st.set("framerate", s.fps),
                // …and given src caps, we accept any input rate.
                _ => st.set(
                    "framerate",
                    gst::FractionRange::new(gst::Fraction::new(0, 1), gst::Fraction::new(i32::MAX, 1)),
                ),
            }
        }
        if s.mi_mode == MiMode::Mci {
            // FFmpeg rejects frames smaller than 2×mb_size in mci mode; refuse them at negotiation.
            let min = 2 * s.mb_size as i32;
            let limits = gst::Caps::builder("video/x-raw")
                .field("width", gst::IntRange::new(min, i32::MAX))
                .field("height", gst::IntRange::new(min, i32::MAX))
                .build();
            out = out.intersect(&limits);
        }
        out
    }

    fn latency(&self, info: &gst_video::VideoInfo) -> gst::ClockTime {
        // Output for the interval between inputs N and N+1 is produced when N+2 arrives.
        convert::frame_duration(info).map_or(gst::ClockTime::ZERO, |d| d * 2)
    }
}

av_video_filter_element!(
    Minterpolate,
    AvFilterMinterpolate,
    "avfilter_minterpolate",
    "GstAvFilterMinterpolate",
    "Filter/Effect/Video"
);
