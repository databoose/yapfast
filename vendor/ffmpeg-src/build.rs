//! Builds FFmpeg's libavcodec and libavutil from ffmpeg/, as update.sh
//! configured them for the target: its hardware encoders and HEVC decoder
//! and what they use, without assembly. gen/<target>/ holds configure's
//! output: the files to compile, its defines, include folders and system
//! libraries. Every FFmpeg file is compiled with gen/rename.h included
//! first, which prefixes FFmpeg's symbols so they can't meet the FFmpeg
//! inside libwebrtc. On Linux, src/va.c loads libva when it's first used.

use std::{env, path::PathBuf};

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    for path in ["ffmpeg", "gen", "include", "src/va.c"] {
        println!("cargo:rerun-if-changed={path}");
    }
    let generated = manifest.join("gen").join(format!("{os}-{arch}"));
    let Ok(sources) = std::fs::read_to_string(generated.join("sources.txt")) else {
        // No encoders configured for this target: dependents build without.
        println!("cargo:encoders=0");
        return;
    };
    let lines = |name: &str| {
        std::fs::read_to_string(generated.join(name))
            .unwrap_or_else(|error| panic!("gen/{os}-{arch}/{name}: {error}"))
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let root = manifest.join("ffmpeg");
    let include = manifest.join("include");
    let rename = manifest.join("gen/rename.h");
    // configure's order: its output, then the source, then its own folders
    // inside it, such as Windows' compat/stdbit.
    let mut folders = vec![generated.clone(), root.clone(), include.clone()];
    folders.extend(lines("includes.txt").iter().map(|folder| root.join(folder)));
    let joined = env::join_paths(&folders).expect("include folders");
    println!("cargo:include={}", joined.to_string_lossy());
    println!("cargo:rename={}", rename.display());
    println!("cargo:encoders=1");

    let mut build = cc::Build::new();
    build
        .includes(&folders)
        .files(sources.lines().map(|file| root.join(file)))
        .define("HAVE_AV_CONFIG_H", None)
        // Encoders on a GPU leave the processor little to do.
        .opt_level(2)
        .warnings(false);
    for define in lines("defines.txt") {
        match define.split_once('=') {
            Some((name, value)) => build.define(name, value),
            None => build.define(&define, None),
        };
    }
    let rename = rename.to_str().expect("a UTF-8 path");
    // clang-cl, for Windows, spells the flags as MSVC does.
    if build.get_compiler().is_like_msvc() {
        build
            .flag("/std:c17")
            .flag("/utf-8")
            .flag(format!("/FI{rename}"));
    } else {
        build
            .flag("-std=c17")
            .flag("-fomit-frame-pointer")
            .flag("-fno-math-errno")
            .flag("-fno-signed-zeros")
            .flag("-pthread")
            .flag("-include")
            .flag(rename);
    }
    let mut objects = build.compile_intermediates();
    if os == "linux" {
        objects.extend(
            cc::Build::new()
                .file(manifest.join("src/va.c"))
                .include(&include)
                .opt_level(2)
                .warnings(true)
                .compile_intermediates(),
        );
        println!("cargo:rustc-link-lib=dl");
    }
    for library in lines("libs.txt") {
        println!("cargo:rustc-link-lib={library}");
    }
    cc::Build::new().objects(objects).compile("yapfast_ffmpeg");
}
