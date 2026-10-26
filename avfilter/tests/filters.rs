use avfilter::{FilterOptions, Frame, Graph, Pulled, VideoParams, rational, sys};

fn gray_params(w: i32, h: i32, fps: i32) -> VideoParams {
    VideoParams {
        format: sys::AV_PIX_FMT_GRAY8,
        width: w,
        height: h,
        time_base: rational(1, fps),
        frame_rate: rational(fps, 1),
        sample_aspect_ratio: rational(1, 1),
    }
}

fn gray_frame(w: i32, h: i32, pts: i64, fill: impl Fn(usize, usize) -> u8) -> Frame {
    let mut f = Frame::new_video(sys::AV_PIX_FMT_GRAY8, w, h).unwrap();
    let (w, h) = (w as usize, h as usize);
    let data: Vec<u8> = (0..h).flat_map(|y| (0..w).map(move |x| (y, x))).map(|(y, x)| fill(x, y)).collect();
    f.copy_plane_from(0, &data, w).unwrap();
    f.set_pts(Some(pts));
    f
}

fn drain(g: &mut Graph) -> Vec<Frame> {
    let mut out = Vec::new();
    while let Pulled::Frame(f) = g.pull().unwrap() {
        out.push(f);
    }
    out
}

#[test]
fn hflip_mirrors_rows() {
    let mut g = Graph::video(&gray_params(4, 2, 25), "hflip", &FilterOptions::new(), 1).unwrap();
    g.push(gray_frame(4, 2, 0, |x, y| (y * 4 + x) as u8)).unwrap(); // rows: 0 1 2 3 / 4 5 6 7
    g.close(Some(1)).unwrap();
    let out = drain(&mut g);
    assert_eq!(out.len(), 1);
    let mut px = vec![0u8; 8];
    out[0].copy_plane_to(0, &mut px, 4).unwrap();
    assert_eq!(px, [3, 2, 1, 0, 7, 6, 5, 4]);
}

#[test]
fn minterpolate_doubles_frame_rate() {
    let opts = FilterOptions::new().rational("fps", 50, 1).string("mi_mode", "blend");
    let mut g = Graph::video(&gray_params(64, 64, 25), "minterpolate", &opts, 1).unwrap();
    let mut out = Vec::new();
    for i in 0..10 {
        g.push(gray_frame(64, 64, i, |x, _| (x * 4) as u8)).unwrap();
        out.extend(drain(&mut g));
    }
    g.close(Some(10)).unwrap();
    out.extend(drain(&mut g));
    assert_eq!(g.output_video().time_base, rational(1, 50));
    // 10 inputs at 25 fps ≈ 20 outputs at 50 fps, minus the tail minterpolate doesn't drain.
    assert!((15..=20).contains(&out.len()), "got {} frames", out.len());
}

#[test]
fn idet_passes_every_frame_and_writes_metadata() {
    let mut g = Graph::video(&gray_params(64, 64, 25), "idet", &FilterOptions::new(), 1).unwrap();
    let mut out = Vec::new();
    for i in 0..5 {
        g.push(gray_frame(64, 64, i, |x, y| ((x + y) % 256) as u8)).unwrap();
        out.extend(drain(&mut g));
    }
    assert_eq!(out.len(), 4, "idet holds one frame back");
    g.close(Some(5)).unwrap();
    out.extend(drain(&mut g));
    assert_eq!(out.len(), 5, "idet flushes the held frame at EOF");
    assert!(out[0].metadata().get("lavfi.idet.single.current_frame").is_some());
}

#[test]
fn probing_finds_native_formats() {
    let cands = [sys::AV_PIX_FMT_GRAY8, sys::AV_PIX_FMT_GRAY16LE, sys::AV_PIX_FMT_YUV420P];
    let mi = avfilter::probe_video_formats("minterpolate", &FilterOptions::new(), &cands);
    assert_eq!(mi, [sys::AV_PIX_FMT_GRAY8, sys::AV_PIX_FMT_YUV420P]); // 8-bit only
    let idet = avfilter::probe_video_formats("idet", &FilterOptions::new(), &cands);
    assert_eq!(idet, cands);
}
