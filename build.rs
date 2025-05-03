fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    slint_build::compile_with_config("ui/main.slint", config).unwrap();

    // Uncomment this and comment above to switch to Qt
    //slint_build::compile("ui/main.slint").unwrap();
}
