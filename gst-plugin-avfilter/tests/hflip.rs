mod common;

use avfilter::{FilterOptions, Graph, Pulled};

#[test]
fn hflip_mirrors_gray8() {
    common::init();
    let mut h = gst_check::Harness::new("avfilter_hflip");
    h.set_src_caps_str("video/x-raw,format=GRAY8,width=4,height=2,framerate=25/1");
    let mut buf = gst::Buffer::from_slice(vec![0u8, 1, 2, 3, 4, 5, 6, 7]);
    buf.get_mut().unwrap().set_pts(gst::ClockTime::ZERO);
    let out = h.push_and_pull(buf).unwrap();
    assert_eq!(&*out.map_readable().unwrap(), &[3, 2, 1, 0, 7, 6, 5, 4]);
}

/// The element must produce exactly the bytes a bare FFmpeg graph produces:
/// the GStreamer layer adds no artifacts (strides, plane offsets, chroma).
#[test]
fn hflip_is_bit_exact_with_ffmpeg_i420() {
    common::init();
    let caps = "video/x-raw,format=I420,width=64,height=48,framerate=25/1";
    let info = gst_video::VideoInfo::from_caps(&caps.parse::<gst::Caps>().unwrap()).unwrap();
    let mut input = common::video_buffer(&info, 0);
    input.get_mut().unwrap().map_writable().unwrap().iter_mut().enumerate().for_each(|(i, p)| *p = (i * 7 % 253) as u8);

    let mut h = gst_check::Harness::new("avfilter_hflip");
    h.set_src_caps_str(caps);
    let via_gst = h.push_and_pull(input.clone()).unwrap();

    let params = avfilter::VideoParams {
        format: avfilter::sys::AV_PIX_FMT_YUV420P,
        width: info.width() as i32,
        height: info.height() as i32,
        time_base: avfilter::rational(1, 1_000_000_000),
        frame_rate: avfilter::rational(25, 1),
        sample_aspect_ratio: avfilter::rational(1, 1),
    };
    let mut g = Graph::video(&params, "hflip", &FilterOptions::new(), 1).unwrap();
    let mut frame = avfilter::Frame::new_video(params.format, params.width, params.height).unwrap();
    {
        let map = input.map_readable().unwrap();
        for plane in 0..info.n_planes() as usize {
            let off = info.offset()[plane];
            frame.copy_plane_from(plane, &map[off..], info.stride()[plane] as usize).unwrap();
        }
    }
    frame.set_pts(Some(0));
    g.push(frame).unwrap();
    g.close(None).unwrap();
    let Pulled::Frame(out) = g.pull().unwrap() else { panic!("hflip produced no frame") };
    let mut direct = vec![0u8; info.size()];
    for plane in 0..info.n_planes() as usize {
        let off = info.offset()[plane];
        out.copy_plane_to(plane, &mut direct[off..], info.stride()[plane] as usize).unwrap();
    }
    assert_eq!(&*via_gst.map_readable().unwrap(), &direct[..]);
}

#[test]
fn hflip_keeps_timestamps() {
    common::init();
    let caps = "video/x-raw,format=GRAY8,width=16,height=16,framerate=25/1";
    let info = gst_video::VideoInfo::from_caps(&caps.parse::<gst::Caps>().unwrap()).unwrap();
    let mut h = gst_check::Harness::new("avfilter_hflip");
    h.set_src_caps_str(caps);
    for n in 0..5 {
        let inbuf = common::video_buffer(&info, n);
        let out = h.push_and_pull(inbuf.clone()).unwrap();
        assert_eq!(out.pts(), inbuf.pts(), "frame {n}");
        assert_eq!(out.duration(), inbuf.duration(), "frame {n}");
    }
}
