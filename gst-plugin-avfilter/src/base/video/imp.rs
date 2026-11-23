use super::{FilterBehavior, Hooks};
use crate::base::convert;
use avfilter::{Graph, Metadata, Pulled, Rational};
use gst::glib;
use gst::prelude::*;
use gst::subclass::prelude::*;
use gst_base::prelude::*;
use gst_base::subclass::base_transform::GenerateOutputSuccess;
use gst_base::subclass::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};

static CAT: LazyLock<gst::DebugCategory> = LazyLock::new(|| {
    gst::DebugCategory::new("avvideofilter", gst::DebugColorFlags::empty(), Some("libavfilter video base class"))
});

const DEFAULT_N_THREADS: u32 = 0; // 0 = let FFmpeg decide

#[derive(Debug, Clone, Copy)]
struct Settings {
    n_threads: u32,
}
impl Default for Settings {
    fn default() -> Self {
        Settings { n_threads: DEFAULT_N_THREADS }
    }
}

/// Runtime state; exists between set_caps and stop.
struct State {
    in_info: gst_video::VideoInfo,
    out_info: gst_video::VideoInfo,
    /// None → (re)build lazily before the next input buffer.
    graph: Option<Graph>,
    /// Time base of the graph's output (filters like minterpolate change it).
    out_tb: Rational,
    /// Frames were pushed since the graph was built (so a drain has something to do).
    has_input: bool,
    /// Reference to the last input frame, for EOS tail padding.
    last_input: Option<avfilter::Frame>,
    /// End of the last input frame (pts + duration) in ns.
    last_end: Option<i64>,
    /// Set DISCONT on the next output buffer.
    discont: bool,
}

#[derive(Default)]
pub struct AvVideoFilter {
    settings: Mutex<Settings>,
    state: Mutex<Option<State>>,
    rebuild: AtomicBool,
}

#[glib::object_subclass]
impl ObjectSubclass for AvVideoFilter {
    const NAME: &'static str = "GstAvVideoFilter";
    const ABSTRACT: bool = true;
    type Type = super::AvVideoFilter;
    type ParentType = gst_base::BaseTransform;
    type Class = super::Class;

    /// Defaults for the base class itself; subclasses overwrite them in IsSubclassable::class_init.
    fn class_init(klass: &mut Self::Class) {
        klass.filter = "";
        klass.behavior = FilterBehavior::DEFAULT;
        klass.hooks = Hooks {
            options: |_| avfilter::FilterOptions::new(),
            map_caps: |_, _, caps| caps.clone(),
            latency: |_, _| gst::ClockTime::ZERO,
            wants_passthrough: |_| false,
            handle_metadata: |_, _, _| {},
        };
    }
}

impl ObjectImpl for AvVideoFilter {
    fn constructed(&self) {
        self.parent_constructed();
        // BaseTransform's "qos" property defaults to false; stateless filters opt in.
        // (Set here, not in set_caps, so an application can still override it.)
        let (_, behavior, _) = self.klass();
        self.obj().set_qos(behavior.qos_safe);
    }

    fn properties() -> &'static [glib::ParamSpec] {
        static PROPERTIES: LazyLock<Vec<glib::ParamSpec>> = LazyLock::new(|| {
            vec![
                glib::ParamSpecUInt::builder("n-threads")
                    .nick("Threads")
                    .blurb("Number of FFmpeg filter threads (0 = automatic)")
                    .default_value(DEFAULT_N_THREADS)
                    .mutable_ready()
                    .build(),
            ]
        });
        PROPERTIES.as_ref()
    }

    fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
        match pspec.name() {
            "n-threads" => {
                self.settings.lock().unwrap().n_threads = value.get().expect("type checked by GObject");
                self.request_rebuild();
            }
            _ => unimplemented!(),
        }
    }

    fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
        match pspec.name() {
            "n-threads" => self.settings.lock().unwrap().n_threads.to_value(),
            _ => unimplemented!(),
        }
    }
}

impl GstObjectImpl for AvVideoFilter {}
impl ElementImpl for AvVideoFilter {} // abstract: no metadata, no templates

impl AvVideoFilter {
    /// Copy the subclass's class data (cheap: a &str, two ints, five fn pointers).
    fn klass(&self) -> (&'static str, FilterBehavior, Hooks) {
        let obj = self.obj();
        let c: &super::Class = obj.class().as_ref();
        (c.filter, c.behavior, c.hooks)
    }

    pub(super) fn request_rebuild(&self) {
        self.rebuild.store(true, Ordering::SeqCst);
    }

    fn build_graph(&self, info: &gst_video::VideoInfo) -> Result<(Graph, Rational), String> {
        let (filter, _, hooks) = self.klass();
        let params = convert::video_params(info).ok_or("unsupported video format")?;
        let opts = (hooks.options)(&self.obj());
        let threads = self.settings.lock().unwrap().n_threads;
        gst::debug!(CAT, imp = self, "building {filter} graph with {opts:?}");
        let graph = Graph::video(&params, filter, &opts, threads).map_err(|e| e.to_string())?;
        let tb = graph.output_video().time_base;
        Ok((graph, tb))
    }

    /// 6b: push everything still inside the graph downstream. Leaves `graph = None`.
    fn drain(&self) -> Result<gst::FlowSuccess, gst::FlowError> {
        let (_, behavior, hooks) = self.klass();
        let outputs: Vec<(gst::Buffer, Metadata)> = {
            let mut guard = self.state.lock().unwrap();
            let Some(state) = guard.as_mut() else { return Ok(gst::FlowSuccess::Ok) };
            if !state.has_input {
                return Ok(gst::FlowSuccess::Ok); // nothing pushed since build: keep the graph
            }
            let Some(mut graph) = state.graph.take() else { return Ok(gst::FlowSuccess::Ok) };
            state.has_input = false;
            let end = state.last_end;

            // Filters that don't flush at EOF (minterpolate): feed copies of the last frame
            // further in time so it emits the tail, then clip at the real end below.
            if let Some(last) = state.last_input.take() {
                let dur = last.duration().max(1);
                for i in 1..=behavior.eos_tail_frames as i64 {
                    if let Ok(mut f) = last.try_clone() {
                        f.set_pts(last.pts().map(|p| p + dur * i));
                        let _ = graph.push(f);
                    }
                }
            }
            if let Err(e) = graph.close(end) {
                gst::warning!(CAT, imp = self, "close failed: {e}");
            }

            let mut out = Vec::new();
            loop {
                match graph.pull() {
                    Ok(Pulled::Frame(f)) => {
                        let buf = convert::frame_to_buffer(&f, &state.out_info, state.out_tb)?;
                        let past_end = behavior.eos_tail_frames > 0
                            && matches!((buf.pts(), end), (Some(pts), Some(end)) if pts.nseconds() as i64 >= end);
                        if !past_end {
                            out.push((buf, f.metadata()));
                        }
                    }
                    Ok(Pulled::Again | Pulled::Eof) => break,
                    Err(e) => {
                        gst::warning!(CAT, imp = self, "drain pull failed: {e}");
                        break;
                    }
                }
            }
            state.discont = true; // a fresh graph will start a new sequence
            out
        }; // lock released — now talk to the rest of the pipeline

        gst::debug!(CAT, imp = self, "drained {} frames", outputs.len());
        for (buf, meta) in outputs {
            if !meta.is_empty() {
                (hooks.handle_metadata)(&self.obj(), &meta, buf.pts());
            }
            self.obj().src_pad().push(buf)?;
        }
        Ok(gst::FlowSuccess::Ok)
    }

    /// 6d: FLUSH_STOP throws everything away; the graph is rebuilt lazily.
    fn reset(&self) {
        if let Some(state) = self.state.lock().unwrap().as_mut() {
            state.graph = None;
            state.has_input = false;
            state.last_input = None;
            state.last_end = None;
            state.discont = true;
        }
    }

    /// 6f: our own latency for the current input format.
    fn current_latency(&self) -> gst::ClockTime {
        let info = self.state.lock().unwrap().as_ref().map(|s| s.in_info.clone());
        let (_, _, hooks) = self.klass();
        info.map_or(gst::ClockTime::ZERO, |i| (hooks.latency)(&self.obj(), &i))
    }
}

/// 6e: after configuring, the graph's output must match what we negotiated.
fn check_output(graph: &Graph, out: &gst_video::VideoInfo) -> Result<(), String> {
    let got = graph.output_video();
    let want_fmt = convert::to_av_format(out.format());
    if Some(got.format) != want_fmt || got.width != out.width() as i32 || got.height != out.height() as i32 {
        return Err(format!(
            "filter outputs {}x{} {} but src caps say {}x{} {:?} — the element's map_caps() is wrong",
            got.width,
            got.height,
            avfilter::formats::name(got.format),
            out.width(),
            out.height(),
            out.format()
        ));
    }
    Ok(())
}

impl BaseTransformImpl for AvVideoFilter {
    const MODE: gst_base::subclass::BaseTransformMode = gst_base::subclass::BaseTransformMode::NeverInPlace;
    const PASSTHROUGH_ON_SAME_CAPS: bool = false;
    const TRANSFORM_IP_ON_PASSTHROUGH: bool = false;

    fn stop(&self) -> Result<(), gst::ErrorMessage> {
        *self.state.lock().unwrap() = None; // drops the graph
        Ok(())
    }

    fn transform_caps(
        &self,
        direction: gst::PadDirection,
        caps: &gst::Caps,
        filter: Option<&gst::Caps>,
    ) -> Option<gst::Caps> {
        let (_, _, hooks) = self.klass();
        let other = (hooks.map_caps)(&self.obj(), direction, caps);
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
        let (_, _, hooks) = self.klass();

        let passthrough = incaps == outcaps && (hooks.wants_passthrough)(&self.obj());
        self.obj().set_passthrough(passthrough);

        let (graph, out_tb) = if passthrough {
            (None, convert::NS_TIME_BASE)
        } else {
            let (graph, tb) = self.build_graph(&in_info).map_err(|e| gst::loggable_error!(CAT, "{e}"))?;
            check_output(&graph, &out_info).map_err(|e| gst::loggable_error!(CAT, "{e}"))?;
            (Some(graph), tb)
        };
        // The graph we just built already reflects the current settings.
        self.rebuild.store(false, Ordering::SeqCst);

        *self.state.lock().unwrap() = Some(State {
            in_info,
            out_info,
            graph,
            out_tb,
            has_input: false,
            last_input: None,
            last_end: None,
            discont: false,
        });

        // Latency may depend on caps (e.g. frame duration) → tell the pipeline to re-query.
        let _ = self.obj().post_message(gst::message::Latency::builder().src(&*self.obj()).build());
        Ok(())
    }

    fn sink_event(&self, event: gst::Event) -> bool {
        match event.view() {
            // Drain BEFORE forwarding: frames inside the graph must go out before EOS / new caps.
            gst::EventView::Eos(_) => {
                if let Err(err) = self.drain() {
                    gst::debug!(CAT, imp = self, "drain on EOS: {err:?}");
                }
            }
            gst::EventView::Caps(c) => {
                let changed = match (self.state.lock().unwrap().as_ref(), gst_video::VideoInfo::from_caps(c.caps())) {
                    (Some(state), Ok(new)) => state.in_info != new,
                    _ => false,
                };
                if changed {
                    let _ = self.drain();
                }
            }
            gst::EventView::FlushStop(_) => self.reset(),
            _ => {}
        }
        self.parent_sink_event(event) // chain up: BaseTransform forwards it / calls set_caps
    }

    fn query(&self, direction: gst::PadDirection, query: &mut gst::QueryRef) -> bool {
        if direction == gst::PadDirection::Src
            && let gst::QueryViewMut::Latency(q) = query.view_mut()
        {
            let mut upstream = gst::query::Latency::new();
            if !self.obj().sink_pad().peer_query(&mut upstream) {
                return false;
            }
            let (live, min, max) = upstream.result();
            let ours = self.current_latency();
            gst::debug!(CAT, imp = self, "upstream latency {min}, adding {ours}");
            q.set(live, min + ours, max.map(|m| m + ours));
            return true;
        }
        BaseTransformImplExt::parent_query(self, direction, query)
    }

    fn submit_input_buffer(&self, is_discont: bool, inbuf: gst::Buffer) -> Result<gst::FlowSuccess, gst::FlowError> {
        if self.obj().is_passthrough() {
            return self.parent_submit_input_buffer(is_discont, inbuf);
        }
        // Let GstController curves update properties for this timestamp.
        if let Some(pts) = inbuf.pts() {
            let _ = self.obj().sync_values(pts);
        }
        // Check the buffer flag, NOT `is_discont` (it stays true while we return NoOutput).
        let rebuild = self.rebuild.swap(false, Ordering::SeqCst);
        if rebuild || inbuf.flags().contains(gst::BufferFlags::DISCONT) {
            self.drain()?; // 6b: flush old frames out, graph becomes None
        }
        if rebuild {
            // drain() keeps an unused graph; a settings change must still replace it.
            if let Some(state) = self.state.lock().unwrap().as_mut() {
                state.graph = None;
            }
        }

        // Build the graph if needed — outside the state lock (calls into subclass code).
        let needs_graph = {
            let guard = self.state.lock().unwrap();
            let state = guard.as_ref().ok_or(gst::FlowError::NotNegotiated)?;
            state.graph.is_none().then(|| state.in_info.clone())
        };
        let new_graph = match needs_graph {
            Some(info) => Some(self.build_graph(&info).map_err(|e| {
                gst::element_imp_error!(self, gst::CoreError::Negotiation, ["{}", e]);
                gst::FlowError::NotNegotiated
            })?),
            None => None,
        };

        let (_, behavior, _) = self.klass();
        let mut guard = self.state.lock().unwrap();
        let state = guard.as_mut().ok_or(gst::FlowError::NotNegotiated)?;
        if let Some((graph, tb)) = new_graph {
            state.graph = Some(graph);
            state.out_tb = tb;
            state.discont = true;
        }

        let frame = convert::buffer_to_frame(&inbuf, &state.in_info)?;
        if behavior.eos_tail_frames > 0 {
            state.last_input = frame.try_clone().ok();
        }
        state.last_end = frame.pts().map(|p| p + frame.duration());
        state.has_input = true;

        let graph = state.graph.as_mut().expect("built above");
        if let Err(e) = graph.push(frame) {
            drop(guard);
            gst::element_imp_error!(self, gst::StreamError::Failed, ["push into graph failed: {}", e]);
            return Err(gst::FlowError::Error);
        }
        Ok(gst::FlowSuccess::Ok)
    }

    fn generate_output(&self) -> Result<GenerateOutputSuccess, gst::FlowError> {
        if self.obj().is_passthrough() {
            return self.parent_generate_output();
        }
        let (_, _, hooks) = self.klass();
        let (buffer, meta) = {
            let mut guard = self.state.lock().unwrap();
            let Some(state) = guard.as_mut() else { return Ok(GenerateOutputSuccess::NoOutput) };
            let Some(graph) = state.graph.as_mut() else { return Ok(GenerateOutputSuccess::NoOutput) };
            let frame = match graph.pull() {
                Ok(Pulled::Frame(f)) => f,
                Ok(Pulled::Again | Pulled::Eof) => return Ok(GenerateOutputSuccess::NoOutput),
                Err(e) => {
                    drop(guard);
                    gst::element_imp_error!(self, gst::StreamError::Failed, ["pull from graph failed: {}", e]);
                    return Err(gst::FlowError::Error);
                }
            };
            let mut buffer = convert::frame_to_buffer(&frame, &state.out_info, state.out_tb)?;
            if std::mem::take(&mut state.discont) {
                buffer.make_mut().set_flags(gst::BufferFlags::DISCONT);
            }
            (buffer, frame.metadata())
        }; // ← lock released here

        if !meta.is_empty() {
            (hooks.handle_metadata)(&self.obj(), &meta, buffer.pts());
        }
        Ok(GenerateOutputSuccess::Buffer(buffer))
    }
}
