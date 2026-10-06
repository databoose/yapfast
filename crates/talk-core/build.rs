//! Builds the Go Live codec shims against the libvpx, OpenH264 and FFmpeg
//! headers in the same libwebrtc archive webrtc-sys links, so they match its
//! code.

fn main() {
    println!("cargo:rerun-if-changed=native/vp8.c");
    println!("cargo:rerun-if-changed=native/h264.c");
    println!("cargo:rerun-if-changed=native/decode.c");
    if std::env::var_os("CARGO_FEATURE_DISCORD").is_none() {
        return;
    }
    let third_party = webrtc_sys_build::webrtc_dir().join("include/third_party");
    let os = match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("windows") => "win",
        Ok("macos") => "mac",
        _ => "linux",
    };
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        _ => "x64",
    };
    cc::Build::new()
        .file("native/vp8.c")
        .file("native/h264.c")
        .file("native/decode.c")
        .include(third_party.join("libvpx/source/libvpx"))
        .include(third_party.join("openh264/src/codec/api"))
        .include(third_party.join("ffmpeg"))
        // FFmpeg's configuration for the Chrome build libwebrtc uses.
        .include(third_party.join(format!("ffmpeg/chromium/config/Chrome/{os}/{arch}")))
        .warnings(true)
        .compile("talk_codecs");
}
