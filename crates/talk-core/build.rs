//! Builds the Go Live codec shims, the video file reader and the cameras
//! against the libvpx, OpenH264, FFmpeg, dav1d, libyuv and WebRTC headers in
//! the same libwebrtc archive webrtc-sys links, so they match its code.

fn main() {
    for file in [
        "vp8.c",
        "h264.c",
        "decode.c",
        "video.c",
        "picture.c",
        "camera.cc",
    ] {
        println!("cargo:rerun-if-changed=native/{file}");
    }
    let include = webrtc_sys_build::webrtc_dir().join("include");
    let third_party = include.join("third_party");
    cc::Build::new()
        .file("native/picture.c")
        .include(third_party.join("libyuv/include"))
        .warnings(true)
        .compile("talk_picture");
    // Linux captures through V4L2. Windows's DirectShow side of the same
    // module is in its libwebrtc archive too, but isn't built yet; see
    // docs/voice.md.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        let mut camera = cc::Build::new();
        camera
            .cpp(true)
            .file("native/camera.cc")
            .include(&include)
            .include(third_party.join("abseil-cpp"))
            .include(third_party.join("libyuv/include"))
            .flag("-std=c++20")
            .flag("-Wno-deprecated-declarations")
            .warnings(true);
        // The toolchain sets its own hardening mode.
        for (key, value) in webrtc_sys_build::webrtc_defines() {
            if key != "_LIBCPP_HARDENING_MODE" {
                camera.define(&key, value.as_deref());
            }
        }
        camera.compile("talk_camera");
    }
    if std::env::var_os("CARGO_FEATURE_DISCORD").is_none() {
        return;
    }
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
        .file("native/video.c")
        .include(third_party.join("libvpx/source/libvpx"))
        .include(third_party.join("openh264/src/codec/api"))
        .include(third_party.join("ffmpeg"))
        .include(third_party.join("dav1d/libdav1d/include"))
        .include(third_party.join("libyuv/include"))
        // FFmpeg's configuration for the Chrome build libwebrtc uses.
        .include(third_party.join(format!("ffmpeg/chromium/config/Chrome/{os}/{arch}")))
        .warnings(true)
        .compile("talk_codecs");
}
