fn main() {
    for cfg in ["ffmpeg_7_0", "ffmpeg_7_1", "ffmpeg_8_0"] {
        println!("cargo::rustc-check-cfg=cfg({cfg})");
    }
    if let Ok(cfgs) = std::env::var("DEP_AVFILTER_CFGS") {
        for cfg in cfgs.split(',').filter(|c| !c.is_empty()) {
            println!("cargo::rustc-cfg={cfg}");
        }
    }
}
