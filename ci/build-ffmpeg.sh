#!/usr/bin/env bash
# Build a minimal FFmpeg (libavutil + libavfilter, only the filters we wrap) into a prefix.
# Usage: ci/build-ffmpeg.sh 7.1.5 [prefix]   (default prefix: ~/.local/ffmpeg-<version>)
set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "usage: $0 <ffmpeg-version> [prefix]" >&2
  exit 2
fi
VER="$1"
PREFIX="${2:-$HOME/.local/ffmpeg-$VER}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

cd "$WORK"
curl -fsSLO "https://ffmpeg.org/releases/ffmpeg-$VER.tar.xz"
tar xf "ffmpeg-$VER.tar.xz"
cd "ffmpeg-$VER"
./configure --prefix="$PREFIX" \
  --enable-shared --disable-static --enable-pic \
  --disable-everything --disable-programs --disable-doc \
  --disable-autodetect --disable-network \
  --disable-avcodec --disable-avformat --disable-avdevice \
  --disable-swscale --disable-swresample \
  --enable-avfilter \
  --enable-filter=buffer,buffersink,abuffer,abuffersink,hflip,idet,minterpolate
make -j"$(nproc)"
make install
echo "Installed FFmpeg $VER to $PREFIX"
