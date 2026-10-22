use std::{env, path::PathBuf};

fn version_tuple(v: &str) -> (u32, u32) {
    let mut it = v.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    (it.next().unwrap_or(0), it.next().unwrap_or(0))
}

fn main() {
    let libs = system_deps::Config::new()
        .probe()
        .unwrap_or_else(|e| panic!("FFMPEG not found or too old: {e}"));

    let lavfi = version_tuple(&libs.get_by_name("libavfilter").unwrap().version);
    let mut enabled = Vec::new();
    for (cfg, min) in [
        ("ffmpeg_7_0", (10, 1)),
        ("ffmpeg_7_1", (10, 4)),
        ("ffmpeg_8_0", (11, 4)),
    ] {
        println!("cargo::rustc-check-cfg=cfg({cfg})");
        if lavfi >= min {
            println!("cargo::rustc-cfg={cfg}");
            enabled.push(cfg);
        }
    }
    println!("cargo::metadata=cfgs={}", enabled.join(","));

    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_args(
            libs.all_include_paths()
                .iter()
                .map(|p| format!("-I{}", p.display())),
        )
        .allowlist_function("avfilter_.*|av_buffersrc_.*|av_buffersink_.*")
        .allowlist_function("av_frame_.*|av_opt_.*|av_dict_.*|av_image_.*")
        .allowlist_function("av_pix_fmt_.*|av_get_pix_fmt_name|av_channel_layout_.*")
        .allowlist_function("av_strerror|av_rescale_q|av_cpu_max_align|av_free")
        .allowlist_function("av_log_.*|avutil_version")
        .allowlist_type("AV.*")
        .allowlist_var("AV_.*|AVFILTER_.*|LIBAVFILTER_VERSION_.*|LIBAVUTIL_VERSION_.*")
        .prepend_enum_name(false)
        .derive_default(true)
        .derive_partialeq(true)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("bindgen failed to parse FFmpeg headers");

    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("bindings.rs");
    bindings
        .write_to_file(out)
        .expect("could not write bindings")
}
