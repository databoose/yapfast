//! Builds FFmpeg's libavcodec and libavutil from ffmpeg/, as update.sh
//! configured them for the target: its hardware encoders and HEVC decoder
//! and what they use, with the x86-64 assembly, which NASM assembles ($NASM,
//! or nasm on the PATH). gen/<target>/ holds configure's output: the files
//! to compile and assemble, its defines, include folders and system
//! libraries. Every FFmpeg C file is compiled with gen/rename.h included
//! first, which prefixes FFmpeg's symbols so they can't meet the FFmpeg
//! inside libwebrtc, and NASM gives the assembly's the same prefix. On
//! Linux, src/va.c loads libva when it's first used.

use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    for path in ["ffmpeg", "gen", "include", "src/va.c"] {
        println!("cargo:rerun-if-changed={path}");
    }
    println!("cargo:rerun-if-env-changed=NASM");
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
    objects.extend(assemble(
        &generated,
        &root,
        &lines("asm.txt"),
        &lines("asmflags.txt"),
    ));
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

/// Assembles FFmpeg's assembly as configure's makefiles would, with
/// config.asm read first, returning the objects. `--gprefix` puts
/// gen/rename.h's prefix on every symbol it defines or uses, all of them
/// FFmpeg's.
fn assemble(generated: &Path, root: &Path, files: &[String], flags: &[String]) -> Vec<PathBuf> {
    let nasm = env::var_os("NASM").unwrap_or_else(|| "nasm".into());
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let folder = |path: &Path| format!("-I{}/", path.display());
    files
        .iter()
        .map(|file| {
            let source = root.join(file);
            let object = out.join(file.trim_end_matches(".asm").replace('/', "-") + ".o");
            // nasm joins include paths to file names as they are, so each
            // ends in a slash.
            let status = Command::new(&nasm)
                .args(flags)
                .arg(folder(generated))
                .arg(folder(root))
                .arg(folder(source.parent().expect("a folder")))
                .arg("-P")
                .arg(generated.join("config.asm"))
                .args(["--gprefix", "yf_", "-o"])
                .arg(&object)
                .arg(&source)
                .status();
            match status {
                Ok(status) if status.success() => object,
                Ok(status) => panic!("nasm failed on {file}: {status}"),
                Err(error) => panic!(
                    "could not run nasm ({error}), which assembles FFmpeg's x86-64 code: \
                     install it, or set NASM to its path"
                ),
            }
        })
        .collect()
}
