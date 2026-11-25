# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- STR-541 - `avfilter_frameinsert` brings fractional NTSC rates up to the whole-number rate above them (59.94 → 60, 29.97 → 30, 23.976 → 24) in real time. Every input frame passes through unchanged and only the one frame missing every 1001 is motion-interpolated with FFmpeg's `minterpolate`. That frame repeats the frame before it instead on a scene cut or on interlaced input.

## [v0.1.0] - 2026-11-25

### Added

- STR-1687 - First release of the `avfilter` GStreamer plugin, which exposes FFmpeg libavfilter filters as GStreamer elements: `avfilter_hflip`, `avfilter_minterpolate` (motion-interpolated frame rate conversion) and `avfilter_idet` (interlace detection).

[Unreleased]: https://github.com/ClipMyHorseTV/gst-libavfilter/compare/v0.1.0...HEAD
[v0.1.0]: https://github.com/ClipMyHorseTV/gst-libavfilter/releases/tag/v0.1.0
