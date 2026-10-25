//! FFmpeg's hardware video encoders as a static library, for C code to call
//! through FFmpeg's headers: the build script compiles and links it, and
//! hands dependents' build scripts what they need to include, as
//! `DEP_FFMPEG_INCLUDE` (folders, joined like `PATH`) and `DEP_FFMPEG_RENAME`
//! (a header to force-include, which gives FFmpeg's names their prefix).
//! `DEP_FFMPEG_ENCODERS` is 0 where no encoders are built for the target.
//! Rust code reaches it only through those shims.
