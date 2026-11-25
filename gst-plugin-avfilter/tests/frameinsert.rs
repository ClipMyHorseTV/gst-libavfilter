mod common;

use gst::prelude::*;
use gst_video::prelude::*;

const W: u32 = 64;
const H: u32 = 64;

fn info(fps: gst::Fraction) -> gst_video::VideoInfo {
    gst_video::VideoInfo::builder(gst_video::VideoFormat::I420, W, H).fps(fps).build().unwrap()
}

fn interlaced_info(fps: gst::Fraction) -> gst_video::VideoInfo {
    gst_video::VideoInfo::builder(gst_video::VideoFormat::I420, W, H)
        .fps(fps)
        .interlace_mode(gst_video::VideoInterlaceMode::Interleaved)
        .build()
        .unwrap()
}

fn caps(fps: &str) -> String {
    format!("video/x-raw,format=I420,width={W},height={H},framerate={fps}")
}

/// Frame `n`: luma `background` with a bright 16×16 square that moves one pixel per frame.
fn frame(info: &gst_video::VideoInfo, n: u64, background: u8) -> gst::Buffer {
    let mut buf = common::video_buffer(info, n);
    {
        let b = buf.get_mut().unwrap();
        let mut vf = gst_video::VideoFrameRef::from_buffer_ref_writable(b, info).unwrap();
        let stride = vf.plane_stride()[0] as usize;
        let x0 = (n % 40) as usize;
        let y = vf.plane_data_mut(0).unwrap();
        for row in 0..H as usize {
            for col in 0..W as usize {
                let square = (24..40).contains(&row) && (x0..x0 + 16).contains(&col);
                y[row * stride + col] = if square { 200 } else { background };
            }
        }
        for plane in 1..3 {
            vf.plane_data_mut(plane).unwrap().fill(128);
        }
    }
    buf
}

fn data_ptr(buf: &gst::Buffer) -> usize {
    buf.map_readable().unwrap().as_slice().as_ptr() as usize
}

fn bytes(buf: &gst::Buffer) -> Vec<u8> {
    buf.map_readable().unwrap().to_vec()
}

/// Push `inputs`, send EOS and return (inputs, outputs).
fn run(h: &mut gst_check::Harness, inputs: Vec<gst::Buffer>) -> (Vec<gst::Buffer>, Vec<gst::Buffer>) {
    for b in &inputs {
        h.push(b.clone()).unwrap();
    }
    h.push_event(gst::event::Eos::new());
    let outputs = std::iter::from_fn(|| h.try_pull()).collect();
    (inputs, outputs)
}

/// Index into `inputs` for every output that is an input frame passed through, None for inserted ones.
fn origins(inputs: &[gst::Buffer], outputs: &[gst::Buffer]) -> Vec<Option<usize>> {
    let ptrs: Vec<usize> = inputs.iter().map(data_ptr).collect();
    outputs.iter().map(|o| ptrs.iter().position(|&p| p == data_ptr(o))).collect()
}

#[test]
fn ntsc_input_gets_one_interpolated_frame_per_1001() {
    common::init();
    let mut h = gst_check::Harness::new("avfilter_frameinsert");
    h.set_src_caps_str(&caps("60000/1001"));
    let info = info(gst::Fraction::new(60000, 1001));
    let (inputs, outputs) = run(&mut h, (0..1100).map(|n| frame(&info, n, 60)).collect());

    // Every input comes out exactly once and in order, plus exactly one new frame.
    let origin = origins(&inputs, &outputs);
    let passed: Vec<usize> = origin.iter().flatten().copied().collect();
    assert_eq!(passed, (0..inputs.len()).collect::<Vec<_>>());
    let inserted: Vec<usize> = (0..origin.len()).filter(|&i| origin[i].is_none()).collect();
    assert_eq!(inserted.len(), 1, "{inserted:?}");

    // On the 60 fps grid, starting at the first input frame.
    let frame = gst::ClockTime::SECOND.nseconds() / 60;
    for (i, pair) in outputs.windows(2).enumerate() {
        let step = (pair[1].pts().unwrap() - pair[0].pts().unwrap()).nseconds();
        assert!((frame..=frame + 1).contains(&step), "output {i}: step {step} ns");
    }
    assert_eq!(outputs[0].pts(), inputs[0].pts());

    // The inserted frame is interpolated: it is neither of its neighbours.
    let i = inserted[0];
    let new = bytes(&outputs[i]);
    assert_ne!(new, bytes(&outputs[i - 1]), "inserted frame repeats the frame before it");
    assert_ne!(new, bytes(&outputs[i + 1]), "inserted frame repeats the frame after it");
}

#[test]
fn scene_cut_repeats_the_frame_before_it() {
    common::init();
    let mut h = gst_check::Harness::new("avfilter_frameinsert");
    h.set_src_caps_str(&caps("30000/1001"));
    let info = info(gst::Fraction::new(30000, 1001));
    // The drift reaches half a frame between inputs 500 and 501 (see `video_buffer`'s timestamps).
    let cut = 501;
    let inputs = (0..600).map(|n| frame(&info, n, if n < cut { 16 } else { 235 })).collect();
    let (inputs, outputs) = run(&mut h, inputs);

    // The repeated frame shares its memory with the frame before the cut, so it shows up twice.
    assert_eq!(outputs.len(), inputs.len() + 1);
    let origin = origins(&inputs, &outputs);
    let i = (1..origin.len()).find(|&i| origin[i] == origin[i - 1]).expect("a frame is repeated");
    assert_eq!(origin[i], Some(cut as usize - 1), "repeated the wrong frame: {:?}", &origin[i - 2..i + 2]);
    assert_eq!(bytes(&outputs[i]), bytes(&inputs[cut as usize - 1]));
}

#[test]
fn interlaced_input_repeats_instead_of_interpolating() {
    common::init();
    let mut h = gst_check::Harness::new("avfilter_frameinsert");
    h.set_src_caps_str(&format!("{},interlace-mode=interleaved", caps("30000/1001")));
    let info = interlaced_info(gst::Fraction::new(30000, 1001));
    let (inputs, outputs) = run(&mut h, (0..600).map(|n| frame(&info, n, 60)).collect());

    assert_eq!(outputs.len(), inputs.len() + 1);
    let origin = origins(&inputs, &outputs);
    assert!(origin.iter().all(Option::is_some), "a frame was interpolated from woven fields");
    let i = (1..origin.len()).find(|&i| origin[i] == origin[i - 1]).expect("a frame is repeated");
    assert!(outputs[i].pts() > outputs[i - 1].pts());
}

#[test]
fn integer_rate_input_is_untouched() {
    common::init();
    let mut h = gst_check::Harness::new("avfilter_frameinsert");
    h.set_src_caps_str(&caps("25/1"));
    let info = info(gst::Fraction::new(25, 1));
    let (inputs, outputs) = run(&mut h, (0..50).map(|n| frame(&info, n, 60)).collect());
    assert_eq!(origins(&inputs, &outputs), (0..50).map(Some).collect::<Vec<_>>());
    for (i, o) in inputs.iter().zip(&outputs) {
        assert_eq!(i.pts(), o.pts());
    }
}

#[test]
fn src_caps_carry_the_whole_number_rate() {
    common::init();
    for (pipeline, input, want) in [
        ("avfilter_frameinsert", (60000, 1001), 60),
        ("avfilter_frameinsert", (30000, 1001), 30),
        ("avfilter_frameinsert", (24000, 1001), 24),
        ("avfilter_frameinsert fps=60/1", (60000, 1001), 60),
    ] {
        let mut h = gst_check::Harness::new_parse(pipeline);
        let input = gst::Fraction::new(input.0, input.1);
        h.set_src_caps_str(&caps(&input.to_string()));
        h.push(frame(&info(input), 0, 60)).unwrap();
        let caps = h.sinkpad().unwrap().current_caps().expect("negotiated");
        assert_eq!(
            caps.structure(0).unwrap().get::<gst::Fraction>("framerate").unwrap(),
            gst::Fraction::new(want, 1),
            "{input}"
        );
    }
}
