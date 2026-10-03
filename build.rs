fn main() {
    #[cfg(feature = "desktop")]
    slint_build::compile_with_config(
        "ui/talk.slint",
        slint_build::CompilerConfiguration::new().with_style("fluent".into()),
    )
    .expect("could not compile Talk's Slint UI");
}
