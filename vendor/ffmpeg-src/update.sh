#!/usr/bin/env bash
# Regenerates ffmpeg/, include/ and gen/ from pinned sources: FFmpeg's
# release, nv-codec-headers, and libva's headers from the development
# shell's nixpkgs. It configures and builds FFmpeg once for each target, the
# way build.rs will, and keeps only the files that build read, the files
# configure generated, and the list of symbols to rename; then it applies
# patches/ to the kept files. Windows is built with clang-cl against the
# MSVC runtime and Windows SDK cargo-xwin downloads, which a Windows build
# (scripts/build-windows) fetches the first time.
# Usage: vendor/ffmpeg-src/update.sh (needs the network and Nix).
set -euo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=$(cd "$here/../.." && pwd)

FFMPEG_VERSION=9.0.2
FFMPEG_SHA256=8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e
# 12.0 needs NVIDIA's driver 520 or newer; newer headers raise that.
NVH_VERSION=12.0.16.1
NVH_SHA256=bfb3306f22c169fedcfc5489b344f414089eec796ffd528412677b0c872bc6e2
# Hardware encoders, and the HEVC decoder for watching H265 streams, which
# libwebrtc's FFmpeg lacks: the libraries hold nothing else.
LINUX_ENCODERS=h264_nvenc,hevc_nvenc,av1_nvenc,h264_vaapi,hevc_vaapi,av1_vaapi
WINDOWS_ENCODERS=h264_nvenc,hevc_nvenc,av1_nvenc
COMMON=(--disable-everything --disable-programs --disable-doc --disable-network
  --disable-autodetect --disable-avformat --disable-avdevice --disable-swscale
  --disable-swresample --disable-avfilter --disable-iconv --disable-asm
  --disable-debug --enable-pic --enable-static --disable-shared)

if [[ -z "${YAPFAST_FFMPEG_UPDATE:-}" ]]; then
  export YAPFAST_FFMPEG_UPDATE=1
  exec nix shell --inputs-from "path:$root/nix" nixpkgs#gnumake nixpkgs#clang \
    nixpkgs#pkg-config nixpkgs#binutils nixpkgs#curl nixpkgs#gnutar nixpkgs#xz nixpkgs#patch \
    --command "$0" "$@"
fi
xwin=${XWIN_CACHE_DIR:-$HOME/.cache/cargo-xwin}/xwin
if [[ ! -d "$xwin/sdk" ]]; then
  echo "update.sh: no Windows SDK in $xwin; run scripts/build-windows once" >&2
  exit 1
fi
# The Windows tools, by path, so the Linux build keeps the wrapped clang.
llvm=$(nix build --no-link --print-out-paths --inputs-from "path:$root/nix" \
  nixpkgs#llvmPackages_21.clang-unwrapped nixpkgs#llvmPackages_21.lld nixpkgs#llvmPackages_21.llvm |
  tr '\n' ' ')

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
fetch() {  # url file sha256
  curl -sSfL -o "$work/$2" "$1"
  echo "$3  $work/$2" | sha256sum -c --quiet
}
fetch "https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz" ffmpeg.tar.xz "$FFMPEG_SHA256"
fetch "https://github.com/FFmpeg/nv-codec-headers/releases/download/n$NVH_VERSION/nv-codec-headers-$NVH_VERSION.tar.gz" \
  nvh.tar.gz "$NVH_SHA256"
tar xf "$work/ffmpeg.tar.xz" -C "$work"
tar xf "$work/nvh.tar.gz" -C "$work"
src=$work/ffmpeg-$FFMPEG_VERSION
make -s -C "$work/nv-codec-headers-$NVH_VERSION" PREFIX="$work/nvh" install
libva=$(nix build --no-link --print-out-paths --inputs-from "path:$root/nix" nixpkgs#libva.dev)
export PKG_CONFIG_PATH=$work/nvh/lib/pkgconfig:$libva/lib/pkgconfig

# One target: configure, build, and record what it compiled and generated.
build() {  # target, nm, then configure's arguments
  local target=$1 nm=$2 out=$work/build-$1
  shift 2
  mkdir -p "$out"
  (cd "$out" && bash "$src/configure" "${COMMON[@]}" "$@" >/dev/null)
  make -s -C "$out" -j"$(nproc)" >/dev/null
  local gen=$work/gen/$target
  mkdir -p "$gen/libavutil" "$gen/libavcodec"
  cp "$out/config.h" "$out/config_components.h" "$gen/"
  cp "$out/libavutil/avconfig.h" "$out/libavutil/ffversion.h" "$gen/libavutil/"
  cp "$out"/libavcodec/{codec,parser,bsf}_list.c "$gen/libavcodec/"
  # intmath.c only gives av_log2 and av_log2_16bit out-of-line bodies for
  # FFmpeg's shared libraries, under #undef, which would drop their prefix;
  # FFmpeg's own code reaches them through intmath.h's macros.
  (cd "$out" && find libavcodec libavutil -name '*.o' | sed 's/\.o$/.c/' | sort) |
    grep -vx libavutil/intmath.c > "$gen/sources.txt"
  # Every file the compiler read, as the .d files name them, and the files
  # it compiled, which clang-cl's leave out.
  find "$out/libavcodec" "$out/libavutil" -name '*.d' -exec cat {} + | tr ' \\' '\n\n' |
    grep -v ':$' | grep . >> "$work/read.txt"
  sed 's|^|src/|' "$gen/sources.txt" >> "$work/read.txt"
  # What build.rs adds to compile and link as configure would: its defines,
  # its include folders inside FFmpeg, and the system libraries it links.
  sed -n 's/^CPPFLAGS=//p' "$out/ffbuild/config.mak" | tr ' ' '\n' | sed -n 's/^-D//p' > "$gen/defines.txt"
  sed -n 's/^CPPFLAGS=//p' "$out/ffbuild/config.mak" | tr ' ' '\n' |
    sed -n 's|^-I\$(SRC_PATH)/||p' > "$gen/includes.txt"
  # Not libva, which src/va.c loads when it's used, nor libatomic, which
  # x86-64 doesn't need and which would be one more library to install.
  sed -n 's/^EXTRALIBS\(-avcodec\|-avutil\)\{0,1\}=//p' "$out/ffbuild/config.mak" | tr ' ' '\n' |
    sed -n -e 's/^-l//p' -e 's/\.lib$//p' -e 's/^-pthread$/pthread/p' |
    grep -vxE 'va|va-drm|atomic' | sort -u > "$gen/libs.txt"
  # Archives as on Linux, libraries as on Windows.
  local libraries=() library
  for library in libavcodec/libavcodec.a libavutil/libavutil.a libavcodec/avcodec.lib libavutil/avutil.lib; do
    if [[ -f "$out/$library" ]]; then libraries+=("$out/$library"); fi
  done
  "$nm" --defined-only -g "${libraries[@]}" | awk 'NF == 3 {print $3}' >> "$work/defined.txt"
  "$nm" -u "${libraries[@]}" | awk 'NF == 2 {print $2}' >> "$work/used.txt"
}
build linux-x86_64 nm --cc=clang \
  --enable-ffnvcodec --enable-nvenc --enable-vaapi --enable-encoder="$LINUX_ENCODERS" \
  --enable-decoder=hevc
# configure takes `lib` for Microsoft's when its usage says so, and finds
# the MSVC runtime, the SDK and nv-codec-headers through INCLUDE and LIB,
# which clang-cl and lld-link read, so no path of this machine lands in
# config.h.
mkdir -p "$work/bin"
for tool in clang-cl lld-link llvm-lib llvm-nm; do
  ln -s "$(find $llvm -path "*/bin/$tool" -print -quit)" "$work/bin/$tool"
done
printf '%s\n' '#!/usr/bin/env bash' \
  'if [ $# -eq 0 ]; then echo "Microsoft (R) Library Manager, as llvm-lib"; exit 0; fi' \
  'exec llvm-lib "$@"' > "$work/bin/lib"
chmod +x "$work/bin/lib"
(
  export PATH=$work/bin:$PATH
  export INCLUDE="$work/nvh/include;$xwin/crt/include;$xwin/sdk/include/ucrt;$xwin/sdk/include/um;$xwin/sdk/include/shared"
  export LIB="$xwin/crt/lib/x86_64;$xwin/sdk/lib/um/x86_64;$xwin/sdk/lib/ucrt/x86_64"
  windows="clang-cl --target=x86_64-pc-windows-msvc"
  build windows-x86_64 llvm-nm --toolchain=msvc --cc="$windows" --cxx="$windows" --ld=lld-link \
    --ar=lib --nm=llvm-nm --target-os=win64 --arch=x86_64 --enable-cross-compile \
    --enable-ffnvcodec --enable-nvenc --enable-encoder="$WINDOWS_ENCODERS" --enable-decoder=hevc
)

if grep -En '/(tmp|nix|home)/' "$work"/gen/*/config.h; then
  echo "update.sh: config.h names a path on this machine" >&2
  exit 1
fi

# The trimmed source: FFmpeg's files the builds read, and its licenses.
rm -rf "$here/ffmpeg" "$here/include" "$here/gen"
sed -n 's|^src/||p' "$work/read.txt" | sort -u | while read -r file; do
  mkdir -p "$here/ffmpeg/$(dirname "$file")"
  cp "$src/$file" "$here/ffmpeg/$file"
done
cp "$src"/{COPYING.LGPLv2.1,LICENSE.md,CREDITS,VERSION} "$here/ffmpeg/"
for change in "$here"/patches/*.patch; do
  patch -s -d "$here/ffmpeg" -p1 < "$change"
done
# Headers from outside FFmpeg: NVIDIA's loader and libva's API.
mkdir -p "$here/include"
cp -r "$work/nvh/include/ffnvcodec" "$here/include/"
cp -r "$libva/include/va" "$here/include/"
chmod -R u+w "$here/include"
# Both are MIT, with the notice at the top of each header; packaging ships
# files named NOTICE.
{
  echo "include/ffnvcodec: nv-codec-headers $NVH_VERSION, https://github.com/FFmpeg/nv-codec-headers"
  sed -n '1,/\*\//p' "$here/include/ffnvcodec/nvEncodeAPI.h"
  echo
  echo "include/va: libva $(sed -n 's/^#define VA_VERSION_S *"\(.*\)"/\1/p' "$here/include/va/va_version.h"), https://github.com/intel/libva"
  sed -n '1,/\*\//p' "$here/include/va/va.h"
} > "$here/include/NOTICE"
cp -r "$work/gen" "$here/gen"

# FFmpeg's own symbols get a prefix, and so do the libva functions it calls,
# which src/va.c forwards to libva once it's loaded.
# Not FFmpeg's: clang-cl's constants for Windows (string literals, vector
# and floating point constants), which the linker merges, and the UCRT's
# stdio functions, which its headers define inline.
sort -u "$work/defined.txt" | grep -vxE 'av_log2(_16bit)?' |
  grep -vE '^(\?\?_C@|__(xmm|ymm|real)@|__local_stdio_)|^_?v?[sf]?n?(printf|scanf)(_s)?$' > "$work/defined.sorted"
mv "$work/defined.sorted" "$work/defined.txt"
grep -Ev '^(av|ff|avpriv|avcodec|avutil|avsubtitle)_' "$work/defined.txt" && {
  echo "update.sh: a symbol without FFmpeg's prefixes" >&2
  exit 1
}
sort -u "$work/used.txt" | grep '^va' > "$work/va.txt" || true
{
  echo "/* Generated by update.sh: renames FFmpeg's symbols, so they can't meet"
  echo " * the FFmpeg inside libwebrtc, and the libva functions src/va.c forwards. */"
  cat "$work/defined.txt" "$work/va.txt" | awk '{print "#define " $1 " yf_" $1}'
} > "$here/gen/rename.h"
missing=$(comm -23 "$work/va.txt" <(grep -oE 'FORWARD\([^,]+, *va[A-Za-z0-9]+' "$here/src/va.c" |
  sed -E 's/.*, *//' | sort -u))
if [[ -n "$missing" ]]; then
  echo "update.sh: add these to src/va.c: $missing" >&2
  exit 1
fi
echo "FFmpeg $FFMPEG_VERSION: $(find "$here/ffmpeg" -type f | wc -l) files," \
  "$(wc -l < "$here/gen/rename.h") renames"
