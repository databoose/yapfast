# FFmpeg for Yapfast

Go Live's hardware encoders: FFmpeg 9.0.2's NVENC encoders for NVIDIA, and on
Linux its VA-API ones for AMD and Intel, for H264, HEVC and AV1. Nothing else
of FFmpeg is built: no decoders, formats, filters or assembly.
`crates/yapfast-core/native/hwenc.c` drives them.

`update.sh` makes everything here except `src/` and `patches/`. It downloads
FFmpeg's release and nv-codec-headers 12.0.16.1 and checks their SHA-256,
takes libva's headers from the development shell's nixpkgs, configures and
builds FFmpeg for each target, and keeps:

- `ffmpeg/`: the FFmpeg files that build read, unchanged but for `patches/`,
  under FFmpeg's LGPL-2.1-or-later (`ffmpeg/COPYING.LGPLv2.1`).
- `include/`: nv-codec-headers' `ffnvcodec/`, which loads NVIDIA's driver
  at run time, and libva's `va/`, both MIT (`include/NOTICE`).
- `gen/<os>-<arch>/`: what configure generated, and the files to compile.
- `gen/rename.h`: a `#define` giving each of FFmpeg's symbols, and the libva
  functions it calls, a `yf_` prefix.

`build.rs` compiles those files with `gen/rename.h` included first. The
prefix matters: libwebrtc's archive holds Chromium's own FFmpeg, built to
decode, and three quarters of the names would otherwise meet. `src/va.c`
defines the `yf_va...` functions, which load libva on first use, so the
binary needs neither libva nor NVIDIA's libraries to start, and an encoder
that can't load them fails to open.

The minimum NVIDIA driver follows the nv-codec-headers version: 12.0 needs
520 or newer. Newer headers would raise it.

## Changes

- `nvenc-bitrate-without-idr.patch`: NVENC changes its bitrate as it runs,
  where FFmpeg resets the encoder, which costs an IDR frame each time. Go
  Live follows Discord's bandwidth estimate, which moves often.

## Updating

Change the versions and hashes at the top of `update.sh`, run it, and build:
it fails when FFmpeg starts calling a libva function `src/va.c` lacks, or
when a patch no longer applies.
