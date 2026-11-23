# gst-libavfilter

A GStreamer plugin that wraps the libavfilter library of ffmpeg

## Requirements

- Rust ≥ 1.92
- GStreamer ≥ 1.24 development files (`libgstreamer1.0-dev`, `libgstreamer-plugins-base1.0-dev`)
- FFmpeg ≥ 6.1 development files (`libavfilter-dev`), plus `clang`/`libclang-dev` for bindgen

## Build and test

```bash
cargo build -p gst-plugin-avfilter
GST_PLUGIN_PATH=$PWD/target/debug gst-inspect-1.0 avfilter
cargo test --workspace
```

### Testing against another FFmpeg version

`ci/build-ffmpeg.sh` builds a minimal FFmpeg (libavutil + libavfilter, only the wrapped
filters) into a prefix without touching the system:

```bash
ci/build-ffmpeg.sh 7.1.5 ~/.local/ffmpeg-7.1.5
export PKG_CONFIG_PATH=~/.local/ffmpeg-7.1.5/lib/pkgconfig
export LD_LIBRARY_PATH=~/.local/ffmpeg-7.1.5/lib
cargo clean -p avfilter-sys && cargo test --workspace
```

## Installing the plugin system-wide (optional)

```bash
cargo cinstall -p gst-plugin-avfilter --release --prefix=/usr/local --libdir=/usr/local/lib/x86_64-linux-gnu
gst-inspect-1.0 avfilter     # found without GST_PLUGIN_PATH
```

Or copy `target/release/libgstavfilter.so` into `~/.local/share/gstreamer-1.0/plugins/`.
