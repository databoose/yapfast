# Talk Windows build patch

This is gpui-pre-windows 0.3.7 from crates.io, an Apache-2.0 snapshot of Zed
revision 1a28cff4b409169bac058bca40dfbfeb7621d19b.

Only build.rs differs. Release shader generation uses CARGO_CFG_TARGET_OS
instead of the build host's target_os. Linux cross-builds can then supply
GPUI_FXC_PATH to invoke the SDK shader compiler through Wine. SDK discovery
through the Windows registry stays compiled only for Windows hosts. Both HLSL
files and GPUI_FXC_PATH invalidate the generated shaders when changed.

The renderer and HLSL files retain their upstream source. Release builds embed
the compiled shader bytes and need no HLSL files on the user's machine.
