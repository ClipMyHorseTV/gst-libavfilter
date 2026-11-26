//! Frame-rate up-conversion without repeated frames: every input frame is passed through and placed
//! on the nearest slot of the output frame grid, and only the slots that no input frame lands on are
//! filled with a frame that minterpolate interpolates from the two neighbouring input frames.
//!
//! At 60000/1001 → 60/1 that is one interpolated frame every 1001 input frames, instead of running
//! motion estimation for every output frame like `avfilter_minterpolate` does.

use crate::base::convert;
use crate::base::enum_nick;
use crate::filters::video::minterpolate::{McMode, Me, MeMode, Scd};
use avfilter::{FilterOptions, Graph, Pulled};
use gst::glib;
use gst::prelude::*;
use gst::subclass::prelude::*;
use gst_base::subclass::base_transform::GenerateOutputSuccess;
use gst_base::subclass::prelude::*;
use gst_video::prelude::*;
use std::collections::VecDeque;
use std::sync::{LazyLock, Mutex};

static CAT: LazyLock<gst::DebugCategory> = LazyLock::new(|| {
    gst::DebugCategory::new(
        "avfilter_frameinsert",
        gst::DebugColorFlags::empty(),
        Some("Frame insertion with minterpolate"),
    )
});

/// The libavfilter filter that interpolates the inserted frames.
const FILTER: &str = "minterpolate";

/// minterpolate's interpolation step (ALPHA_MAX in vf_minterpolate.c): positions between two
/// frames are quantised to 1/1024.
const ALPHA_MAX: i64 = 1024;

const NS_PER_SECOND: u128 = 1_000_000_000;

/// FFmpeg options this element sets on minterpolate (checked against the linked FFmpeg at registration).
pub const OPTIONS: &[&str] = &["fps", "mi_mode", "mc_mode", "me_mode", "me", "mb_size", "search_param", "vsbmc", "scd"];

#[derive(Debug, Clone, Copy)]
struct Settings {
    fps: gst::Fraction,
    max_insert: u32,
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
        // Motion settings and scene change detection default to minterpolate's own defaults.
        Settings {
            fps: gst::Fraction::new(0, 1),
            max_insert: 1,
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

impl Settings {
    /// The output rate for an input rate: the `fps` property, or the input rate rounded up to a whole number.
    fn output_rate(&self, input: Option<gst::Fraction>) -> Option<gst::Fraction> {
        if self.fps.numer() > 0 {
            return Some(self.fps);
        }
        let input = input.filter(|f| f.numer() > 0 && f.denom() > 0)?;
        let (n, d) = (input.numer() as i64, input.denom() as i64);
        i32::try_from((n + d - 1) / d).ok().map(|whole| gst::Fraction::new(whole, 1))
    }

    fn minterpolate_options(&self) -> FilterOptions {
        // One output frame per second: with the frames placed by `interpolate`, the only output
        // that needs motion compensation falls on second 1.
        FilterOptions::new()
            .rational("fps", 1, 1)
            .string("mi_mode", "mci")
            .string("mc_mode", enum_nick(self.mc_mode))
            .string("me_mode", enum_nick(self.me_mode))
            .string("me", enum_nick(self.me))
            .int("mb_size", self.mb_size as i64)
            .int("search_param", self.search_param as i64)
            .int("vsbmc", self.vsbmc as i64)
            // Scene changes are detected here (see `is_scene_change`) so a cut repeats the frame before it.
            .string("scd", "none")
    }
}

/// Where an input frame goes on the output grid.
#[derive(Debug, PartialEq, Eq)]
enum Placement {
    /// Earlier than the next free slot by more than a frame: the input is faster than the output.
    Drop,
    /// No timestamp and no grid yet: pass through as is.
    Untimed,
    /// Output the frame at `slot`, after interpolating the `missing` slots before it.
    Slot { slot: u64, missing: std::ops::Range<u64> },
}

/// Output frame grid anchored at the first frame after a (re)start: slot `n` is at `base + n / fps`.
#[derive(Debug, Clone, Copy)]
struct Grid {
    fps: gst::Fraction,
    base: Option<u64>,
    last: Option<u64>,
}

impl Grid {
    fn new(fps: gst::Fraction) -> Self {
        Grid { fps, base: None, last: None }
    }

    fn reset(&mut self) {
        self.base = None;
        self.last = None;
    }

    /// Nanoseconds per `n` output frames.
    fn span(&self, n: u64) -> u64 {
        let (num, den) = (self.fps.numer() as u128, self.fps.denom() as u128);
        ((n as u128 * NS_PER_SECOND * den + num / 2) / num) as u64
    }

    fn slot_pts(&self, slot: u64) -> u64 {
        self.base.unwrap_or(0) + self.span(slot)
    }

    /// Place a frame with timestamp `pts`. `restart` re-anchors the grid on this frame.
    fn place(&mut self, pts: Option<u64>, restart: bool, max_insert: u32) -> Placement {
        // Monotonic input never lands more than a slot behind the last one, so that is a timestamp jump.
        let behind = self.last.map_or(0, |l| l.saturating_sub(1));
        let backwards = matches!((self.base, pts), (Some(base), Some(p)) if p < base || p < self.slot_pts(behind));
        if restart || backwards || self.base.is_none() {
            let Some(p) = pts else {
                self.reset();
                return Placement::Untimed;
            };
            self.base = Some(p);
            self.last = Some(0);
            return Placement::Slot { slot: 0, missing: 0..0 };
        }

        let expected = self.last.map_or(0, |l| l + 1);
        let (Some(base), Some(p)) = (self.base, pts) else {
            self.last = Some(expected);
            return Placement::Slot { slot: expected, missing: expected..expected };
        };

        // Exact position in output frames is elapsed · num / (den · 1 s).
        let elapsed = (p - base) as u128;
        let num = self.fps.numer() as u128;
        let unit = self.fps.denom() as u128 * NS_PER_SECOND;
        let nearest = ((2 * elapsed * num + unit) / (2 * unit)) as u64;

        let (slot, missing) = if nearest >= expected {
            let gap = nearest - expected;
            // A larger gap (lost input frames) stays a gap instead of stalling on interpolation.
            let missing = if gap <= max_insert as u64 { expected..nearest } else { nearest..nearest };
            (nearest, missing)
        } else if expected > 0 && elapsed * num < (expected as u128 - 1) * unit {
            return Placement::Drop;
        } else {
            // Up to one frame early (timestamp jitter): take the next slot rather than drop.
            (expected, expected..expected)
        };
        self.last = Some(slot);
        Placement::Slot { slot, missing }
    }
}

/// Runtime state; exists between set_caps and stop.
struct State {
    in_info: gst_video::VideoInfo,
    out_info: gst_video::VideoInfo,
    grid: Grid,
    /// The last two input frames, newest first: the neighbours of an inserted frame and the pair
    /// before them, whose difference is the baseline for scene change detection.
    prev: Option<gst::Buffer>,
    prev2: Option<gst::Buffer>,
    /// Set DISCONT on the next output buffer.
    discont: bool,
    pending: VecDeque<gst::Buffer>,
}

impl State {
    fn forget_frames(&mut self) {
        self.prev = None;
        self.prev2 = None;
    }
}

/// What to output for one input frame, worked out under the state lock and executed outside it.
struct Job {
    input: gst::Buffer,
    slot: u64,
    missing: std::ops::Range<u64>,
    grid: Grid,
    discont: bool,
    prev: Option<gst::Buffer>,
    prev2: Option<gst::Buffer>,
    in_info: gst_video::VideoInfo,
    out_info: gst_video::VideoInfo,
}

#[derive(Default)]
pub struct FrameInsert {
    settings: Mutex<Settings>,
    state: Mutex<Option<State>>,
}

#[glib::object_subclass]
impl ObjectSubclass for FrameInsert {
    const NAME: &'static str = "GstAvFilterFrameInsert";
    type Type = AvFilterFrameInsert;
    type ParentType = gst_base::BaseTransform;
}

glib::wrapper! {
    pub struct AvFilterFrameInsert(ObjectSubclass<FrameInsert>)
        @extends gst_base::BaseTransform, gst::Element, gst::Object;
}

pub fn register(plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
    gst::Element::register(Some(plugin), "avfilter_frameinsert", gst::Rank::NONE, AvFilterFrameInsert::static_type())
}

impl ObjectImpl for FrameInsert {
    fn properties() -> &'static [glib::ParamSpec] {
        static PROPERTIES: LazyLock<Vec<glib::ParamSpec>> = LazyLock::new(|| {
            let d = Settings::default();
            vec![
                gst::ParamSpecFraction::builder("fps")
                    .nick("Frame rate")
                    .blurb("Output frame rate (0/1 = the input rate rounded up to a whole number)")
                    .minimum(gst::Fraction::new(0, 1))
                    .maximum(gst::Fraction::new(1000, 1))
                    .default_value(d.fps)
                    .mutable_ready()
                    .build(),
                glib::ParamSpecUInt::builder("max-insert")
                    .nick("Maximum insertions")
                    .blurb("Most consecutive missing frames to interpolate; a longer gap is left as a gap")
                    .maximum(16)
                    .default_value(d.max_insert)
                    .mutable_ready()
                    .build(),
                glib::ParamSpecEnum::builder_with_default("mc-mode", d.mc_mode)
                    .nick("Compensation mode")
                    .blurb("Motion compensation mode")
                    .mutable_ready()
                    .build(),
                glib::ParamSpecEnum::builder_with_default("me-mode", d.me_mode)
                    .nick("Estimation mode")
                    .blurb("Motion estimation mode")
                    .mutable_ready()
                    .build(),
                glib::ParamSpecEnum::builder_with_default("me", d.me)
                    .nick("Estimation algorithm")
                    .blurb("Motion estimation algorithm")
                    .mutable_ready()
                    .build(),
                glib::ParamSpecUInt::builder("mb-size")
                    .nick("Block size")
                    .blurb("Macroblock size (frames must be at least 2×mb-size)")
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
                    .blurb("Scene change detection method; at a cut the frame before it is repeated")
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
            "max-insert" => s.max_insert = value.get().unwrap(),
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
            "max-insert" => s.max_insert.to_value(),
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

impl GstObjectImpl for FrameInsert {}

impl ElementImpl for FrameInsert {
    fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
        static META: LazyLock<gst::subclass::ElementMetadata> = LazyLock::new(|| {
            gst::subclass::ElementMetadata::new(
                "FFmpeg minterpolate frame insertion",
                "Filter/Effect/Video",
                "Raises the frame rate by passing every frame through and motion-interpolating only the missing ones",
                "Simon Koester <s.koester@clipmyhorse.tv>",
            )
        });
        Some(&*META)
    }

    fn pad_templates() -> &'static [gst::PadTemplate] {
        static TEMPLATES: LazyLock<Vec<gst::PadTemplate>> = LazyLock::new(|| {
            let formats: Vec<gst_video::VideoFormat> =
                avfilter::probe_video_formats(FILTER, &FilterOptions::new(), &convert::all_av_formats())
                    .into_iter()
                    .filter_map(convert::to_gst_format)
                    .collect();
            let caps = gst_video::VideoCapsBuilder::new().format_list(formats).build();
            vec![
                gst::PadTemplate::new("sink", gst::PadDirection::Sink, gst::PadPresence::Always, &caps).unwrap(),
                gst::PadTemplate::new("src", gst::PadDirection::Src, gst::PadPresence::Always, &caps).unwrap(),
            ]
        });
        TEMPLATES.as_ref()
    }
}

impl FrameInsert {
    /// Decide where `input` goes and remember it as the newest neighbour.
    fn plan(&self, input: gst::Buffer, max_insert: u32) -> Result<Option<Job>, gst::FlowError> {
        let mut guard = self.state.lock().unwrap();
        let state = guard.as_mut().ok_or(gst::FlowError::NotNegotiated)?;
        let restart = input.flags().contains(gst::BufferFlags::DISCONT);
        let placement = state.grid.place(input.pts().map(gst::ClockTime::nseconds), restart, max_insert);
        let (slot, missing) = match placement {
            Placement::Drop => {
                gst::debug!(CAT, imp = self, "dropping {:?}: input is ahead of the output rate", input.pts());
                return Ok(None);
            }
            Placement::Untimed => {
                state.pending.push_back(input);
                return Ok(None);
            }
            Placement::Slot { slot, missing } => (slot, missing),
        };
        if slot == 0 {
            state.discont = true;
            state.prev2 = None;
        }
        let prev = state.prev.replace(input.clone());
        let prev2 = std::mem::replace(&mut state.prev2, prev.clone());
        Ok(Some(Job {
            input,
            slot,
            missing,
            grid: state.grid,
            discont: std::mem::take(&mut state.discont),
            prev,
            prev2,
            in_info: state.in_info.clone(),
            out_info: state.out_info.clone(),
        }))
    }

    /// The output buffers for one job, in order: the inserted frames, then the input frame.
    fn execute(&self, settings: &Settings, job: Job) -> Vec<gst::Buffer> {
        let mut out = Vec::with_capacity(job.missing.clone().count() + 1);
        let mut discont = job.discont;
        let mut restamp = |mut buf: gst::Buffer, slot: u64| {
            let b = buf.make_mut();
            let pts = job.grid.slot_pts(slot);
            b.set_pts(gst::ClockTime::from_nseconds(pts));
            b.set_duration(gst::ClockTime::from_nseconds(job.grid.slot_pts(slot + 1) - pts));
            if std::mem::take(&mut discont) {
                b.set_flags(gst::BufferFlags::DISCONT);
            } else {
                b.unset_flags(gst::BufferFlags::DISCONT);
            }
            buf
        };

        if let (false, Some(prev)) = (job.missing.is_empty(), job.prev.as_ref()) {
            // Motion estimation across woven fields tears anything that moves.
            let interlaced = is_interlaced(&job.in_info, prev) || is_interlaced(&job.in_info, &job.input);
            let cut = !interlaced
                && settings.scd == Scd::Fdiff
                && is_scene_change(job.prev2.as_ref(), prev, &job.input, &job.in_info, settings.scd_threshold);
            for slot in job.missing.clone() {
                let inserted = if interlaced {
                    gst::debug!(CAT, imp = self, "interlaced input at slot {slot}: repeating the frame before it");
                    Some(prev.copy())
                } else if cut {
                    gst::debug!(CAT, imp = self, "scene change at slot {slot}: repeating the frame before the cut");
                    Some(prev.copy())
                } else {
                    let alpha = alpha_at(job.grid.slot_pts(slot), prev, &job.input);
                    interpolate(settings, &job.in_info, &job.out_info, prev, &job.input, alpha)
                        .inspect_err(|e| {
                            gst::warning!(CAT, imp = self, "interpolating slot {slot} failed, leaving a gap: {e}");
                        })
                        .ok()
                };
                if let Some(buf) = inserted {
                    gst::trace!(CAT, imp = self, "inserted slot {slot}");
                    out.push(restamp(buf, slot));
                }
            }
        }
        out.push(restamp(job.input, job.slot));
        out
    }
}

fn is_interlaced(info: &gst_video::VideoInfo, buf: &gst::Buffer) -> bool {
    match info.interlace_mode() {
        gst_video::VideoInterlaceMode::Progressive => false,
        gst_video::VideoInterlaceMode::Mixed => {
            buf.flags().bits() & gst_video::VideoBufferFlags::INTERLACED.bits() != 0
        }
        _ => true,
    }
}

/// Position of `pts` between the timestamps of `prev` and `cur`, in 1/ALPHA_MAX steps, strictly inside.
fn alpha_at(pts: u64, prev: &gst::Buffer, cur: &gst::Buffer) -> i64 {
    let (Some(a), Some(b)) = (prev.pts(), cur.pts()) else { return ALPHA_MAX / 2 };
    let (a, b) = (a.nseconds() as i128, b.nseconds() as i128);
    if b <= a {
        return ALPHA_MAX / 2;
    }
    let alpha = ((pts as i128 - a) * ALPHA_MAX as i128 * 2 + (b - a)) / (2 * (b - a));
    (alpha as i64).clamp(1, ALPHA_MAX - 1)
}

/// minterpolate's `fdiff` detector (vf_minterpolate.c `detect_scene_change`) for the pair prev → cur.
fn is_scene_change(
    prev2: Option<&gst::Buffer>,
    prev: &gst::Buffer,
    cur: &gst::Buffer,
    info: &gst_video::VideoInfo,
    threshold: f64,
) -> bool {
    let Some(mafd) = mean_abs_frame_diff(prev, cur, info) else { return false };
    let prev_mafd = prev2.and_then(|p2| mean_abs_frame_diff(p2, prev, info)).unwrap_or(0.0);
    let diff = (mafd - prev_mafd).abs();
    mafd.min(diff).clamp(0.0, 100.0) >= threshold
}

/// Mean absolute luma difference in percent of full scale. Every minterpolate format is 8-bit with
/// luma (or gray) in plane 0.
fn mean_abs_frame_diff(a: &gst::Buffer, b: &gst::Buffer, info: &gst_video::VideoInfo) -> Option<f64> {
    let fa = gst_video::VideoFrameRef::from_buffer_ref_readable(a.as_ref(), info).ok()?;
    let fb = gst_video::VideoFrameRef::from_buffer_ref_readable(b.as_ref(), info).ok()?;
    let (w, h) = (info.width() as usize, info.height() as usize);
    let (sa, sb) = (fa.plane_stride()[0] as usize, fb.plane_stride()[0] as usize);
    let (da, db) = (fa.plane_data(0).ok()?, fb.plane_data(0).ok()?);
    let mut sad = 0u64;
    for y in 0..h {
        let (ra, rb) = (da.get(y * sa..y * sa + w)?, db.get(y * sb..y * sb + w)?);
        sad += ra.iter().zip(rb).map(|(x, y)| x.abs_diff(*y) as u64).sum::<u64>();
    }
    Some(sad as f64 * 100.0 / (w * h) as f64 / 256.0)
}

/// Interpolate the frame at `alpha`/ALPHA_MAX between `prev` and `cur` with a throwaway minterpolate graph.
///
/// The frames are placed one second apart, shifted so the 1 fps output grid hits that position at
/// second 1. minterpolate emits everything up to the second frame once a third one arrives, so `cur`
/// is pushed twice; at most one other output (a plain copy at second 0) comes out and is ignored.
fn interpolate(
    settings: &Settings,
    in_info: &gst_video::VideoInfo,
    out_info: &gst_video::VideoInfo,
    prev: &gst::Buffer,
    cur: &gst::Buffer,
    alpha: i64,
) -> Result<gst::Buffer, String> {
    let mut params = convert::video_params(in_info).ok_or("unsupported video format")?;
    params.time_base = avfilter::rational(1, ALPHA_MAX as i32);
    params.frame_rate = avfilter::rational(1, 1);
    let mut graph = Graph::video(&params, FILTER, &settings.minterpolate_options(), 1).map_err(|e| e.to_string())?;

    let frame = |buf: &gst::Buffer, pts: i64| {
        let mut f = convert::buffer_to_frame(buf, in_info).map_err(|e| format!("{e:?}"))?;
        f.set_pts(Some(pts));
        f.set_duration(ALPHA_MAX);
        Ok::<_, String>(f)
    };
    let offset = ALPHA_MAX - alpha;
    for f in [frame(prev, offset)?, frame(cur, ALPHA_MAX + offset)?, frame(cur, 2 * ALPHA_MAX + offset)?] {
        graph.push(f).map_err(|e| e.to_string())?;
    }

    let tb = graph.output_video().time_base;
    let wanted = avfilter::rescale(1, avfilter::rational(1, 1), tb);
    loop {
        match graph.pull().map_err(|e| e.to_string())? {
            Pulled::Frame(f) if f.pts() == Some(wanted) => {
                return convert::frame_to_buffer(&f, out_info, tb).map_err(|e| format!("{e:?}"));
            }
            Pulled::Frame(_) => {}
            Pulled::Again | Pulled::Eof => return Err(format!("minterpolate gave no frame at alpha {alpha}")),
        }
    }
}

impl BaseTransformImpl for FrameInsert {
    const MODE: gst_base::subclass::BaseTransformMode = gst_base::subclass::BaseTransformMode::NeverInPlace;
    const PASSTHROUGH_ON_SAME_CAPS: bool = false;
    const TRANSFORM_IP_ON_PASSTHROUGH: bool = false;

    fn stop(&self) -> Result<(), gst::ErrorMessage> {
        *self.state.lock().unwrap() = None;
        Ok(())
    }

    fn transform_caps(
        &self,
        direction: gst::PadDirection,
        caps: &gst::Caps,
        filter: Option<&gst::Caps>,
    ) -> Option<gst::Caps> {
        let s = *self.settings.lock().unwrap();
        let mut other = caps.clone();
        if !caps.is_any() {
            for st in other.make_mut().iter_mut() {
                let rate = match direction {
                    gst::PadDirection::Sink => s
                        .output_rate(st.get::<gst::Fraction>("framerate").ok())
                        .map(|f| f.to_send_value())
                        .unwrap_or_else(|| {
                            gst::FractionRange::new(gst::Fraction::new(1, 1), gst::Fraction::new(i32::MAX, 1))
                                .to_send_value()
                        }),
                    _ => gst::FractionRange::new(gst::Fraction::new(0, 1), gst::Fraction::new(i32::MAX, 1))
                        .to_send_value(),
                };
                st.set_value("framerate", rate);
            }
            // minterpolate rejects frames smaller than 2×mb_size; refuse them at negotiation.
            let min = 2 * s.mb_size as i32;
            let limits = gst::Caps::builder("video/x-raw")
                .field("width", gst::IntRange::new(min, i32::MAX))
                .field("height", gst::IntRange::new(min, i32::MAX))
                .build();
            other = other.intersect(&limits);
        }
        gst::trace!(CAT, imp = self, "transform_caps {direction:?}: {caps} → {other}");
        Some(match filter {
            Some(f) => f.intersect_with_mode(&other, gst::CapsIntersectMode::First),
            None => other,
        })
    }

    fn set_caps(&self, incaps: &gst::Caps, outcaps: &gst::Caps) -> Result<(), gst::LoggableError> {
        let in_info = gst_video::VideoInfo::from_caps(incaps)
            .map_err(|_| gst::loggable_error!(CAT, "invalid input caps {incaps}"))?;
        let out_info = gst_video::VideoInfo::from_caps(outcaps)
            .map_err(|_| gst::loggable_error!(CAT, "invalid output caps {outcaps}"))?;
        let settings = *self.settings.lock().unwrap();
        let fps = out_info.fps();
        if fps.numer() <= 0 || settings.output_rate(Some(in_info.fps())) != Some(fps) {
            return Err(gst::loggable_error!(
                CAT,
                "output frame rate {fps} does not match fps={} for input {}",
                settings.fps,
                in_info.fps()
            ));
        }

        let mut guard = self.state.lock().unwrap();
        match guard.as_mut() {
            // Same output grid: keep the timeline, but frames of the old format can't be interpolated with.
            Some(state) if state.grid.fps == fps => {
                state.forget_frames();
                state.in_info = in_info;
                state.out_info = out_info;
            }
            _ => {
                *guard = Some(State {
                    in_info,
                    out_info,
                    grid: Grid::new(fps),
                    prev: None,
                    prev2: None,
                    discont: false,
                    pending: VecDeque::new(),
                })
            }
        }
        Ok(())
    }

    fn sink_event(&self, event: gst::Event) -> bool {
        if let gst::EventView::FlushStop(_) = event.view()
            && let Some(state) = self.state.lock().unwrap().as_mut()
        {
            state.grid.reset();
            state.forget_frames();
            state.pending.clear();
        }
        self.parent_sink_event(event)
    }

    fn submit_input_buffer(&self, _is_discont: bool, inbuf: gst::Buffer) -> Result<gst::FlowSuccess, gst::FlowError> {
        let settings = *self.settings.lock().unwrap();
        let Some(job) = self.plan(inbuf, settings.max_insert)? else { return Ok(gst::FlowSuccess::Ok) };
        // Interpolation takes a while on large frames: don't hold the state lock for it.
        let out = self.execute(&settings, job);
        if let Some(state) = self.state.lock().unwrap().as_mut() {
            state.pending.extend(out);
        }
        Ok(gst::FlowSuccess::Ok)
    }

    fn generate_output(&self) -> Result<GenerateOutputSuccess, gst::FlowError> {
        let next = self.state.lock().unwrap().as_mut().and_then(|s| s.pending.pop_front());
        Ok(next.map_or(GenerateOutputSuccess::NoOutput, GenerateOutputSuccess::Buffer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME_5994: u64 = 16_683_333; // 1001/60000 s, truncated like GStreamer's video_buffer

    fn grid(fps: (i32, i32)) -> Grid {
        Grid::new(gst::Fraction::new(fps.0, fps.1))
    }

    #[test]
    fn ntsc_frames_fill_every_slot_and_insert_once_per_1001() {
        let mut g = grid((60, 1));
        let mut inserted = Vec::new();
        let mut last_slot = None;
        for n in 0..3003u64 {
            let pts = n * 1_001_000_000_000 / 60_000;
            match g.place(Some(pts), false, 1) {
                Placement::Slot { slot, missing } => {
                    assert_eq!(missing.start, last_slot.map_or(0, |l| l + 1), "frame {n}: slot skipped");
                    inserted.extend(missing);
                    // Placed within half an output frame of its own timestamp.
                    let err = g.slot_pts(slot) as i64 - pts as i64;
                    assert!(err.abs() <= g.span(1) as i64 / 2 + 1, "frame {n}: {err} ns off");
                    last_slot = Some(slot);
                }
                other => panic!("frame {n}: {other:?}"),
            }
        }
        assert_eq!(inserted.len(), 3, "{inserted:?}");
        // One insertion per 1001 input frames (1002 output slots), give or take a rounding tie.
        assert!(inserted.windows(2).all(|w| (1000..=1003).contains(&(w[1] - w[0]))), "{inserted:?}");
    }

    #[test]
    fn integer_rate_input_passes_straight_through() {
        let mut g = grid((25, 1));
        for n in 0..250u64 {
            let p = g.place(Some(n * 40_000_000), false, 1);
            assert_eq!(p, Placement::Slot { slot: n, missing: n..n });
        }
    }

    #[test]
    fn faster_input_drops_instead_of_stacking_up() {
        let mut g = grid((25, 1));
        let placements: Vec<_> = (0..30u64).map(|n| g.place(Some(n * 1_000_000_000 / 30), false, 1)).collect();
        let kept = placements.iter().filter(|p| matches!(p, Placement::Slot { .. })).count();
        assert!((25..=26).contains(&kept), "{placements:?}");
    }

    #[test]
    fn long_gaps_are_not_interpolated() {
        let mut g = grid((60, 1));
        g.place(Some(0), false, 1);
        let p = g.place(Some(FRAME_5994 * 4), false, 1);
        assert_eq!(p, Placement::Slot { slot: 4, missing: 4..4 });
    }

    #[test]
    fn discont_reanchors_the_grid() {
        let mut g = grid((60, 1));
        g.place(Some(0), false, 1);
        g.place(Some(FRAME_5994), false, 1);
        assert_eq!(g.place(Some(5_000_000_123), true, 1), Placement::Slot { slot: 0, missing: 0..0 });
        assert_eq!(g.slot_pts(0), 5_000_000_123);
    }

    #[test]
    fn backwards_jump_without_discont_reanchors_the_grid() {
        let mut g = grid((60, 1));
        for n in 0..100u64 {
            g.place(Some(n * FRAME_5994), false, 1);
        }
        assert_eq!(g.place(Some(FRAME_5994 * 10), false, 1), Placement::Slot { slot: 0, missing: 0..0 });
        assert_eq!(g.slot_pts(0), FRAME_5994 * 10);
    }

    #[test]
    fn alpha_is_strictly_between_the_frames() {
        let buf = |ns: u64| {
            let mut b = gst::Buffer::new();
            b.get_mut().unwrap().set_pts(gst::ClockTime::from_nseconds(ns));
            b
        };
        gst::init().unwrap();
        let (a, b) = (buf(1000), buf(2000));
        assert_eq!(alpha_at(1500, &a, &b), ALPHA_MAX / 2);
        assert_eq!(alpha_at(1000, &a, &b), 1);
        assert_eq!(alpha_at(2000, &a, &b), ALPHA_MAX - 1);
    }
}
