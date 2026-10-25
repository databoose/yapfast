//! Builds FFmpeg's libavcodec and libavutil from ffmpeg/, as update.sh
//! configured them for the target: its hardware encoders and what they use,
//! without assembly. gen/<target>/ holds configure's output and the files
//! to compile; every FFmpeg file is compiled with gen/rename.h included
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
    let root = manifest.join("ffmpeg");
    let include = manifest.join("include");
    let rename = manifest.join("gen/rename.h");
    let folders = env::join_paths([&generated, &root, &include]).expect("include folders");
    println!("cargo:include={}", folders.to_string_lossy());
    println!("cargo:rename={}", rename.display());
    println!("cargo:encoders=1");

    let mut build = cc::Build::new();
    // configure's order: its output, then the source.
    build
        .include(&generated)
        .include(&root)
        .include(&include)
        .files(sources.lines().map(|file| root.join(file)))
        .define("HAVE_AV_CONFIG_H", None)
        .define("_ISOC11_SOURCE", None)
        .define("_FILE_OFFSET_BITS", "64")
        .define("_LARGEFILE_SOURCE", None)
        .define("_POSIX_C_SOURCE", "200112")
        .define("_XOPEN_SOURCE", "600")
        .define("PIC", None)
        // Encoders on a GPU leave the processor little to do.
        .opt_level(2)
        .warnings(false)
        .flag("-std=c17")
        .flag("-fomit-frame-pointer")
        .flag("-fno-math-errno")
        .flag("-fno-signed-zeros")
        .flag("-pthread")
        .flag("-include")
        .flag(rename.to_str().expect("a UTF-8 path"));
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
        println!("cargo:rustc-link-lib=pthread");
        println!("cargo:rustc-link-lib=m");
    }
    cc::Build::new().objects(objects).compile("yapfast_ffmpeg");
}
