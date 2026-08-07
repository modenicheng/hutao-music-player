fn main() {
    // vendor/lineedit-base.slint 使用 interface 实验特性
    unsafe {
        std::env::set_var("SLINT_ENABLE_EXPERIMENTAL_FEATURES", "1");
    }
    slint_build::compile("ui/app.slint").expect("compile slint ui");
}
