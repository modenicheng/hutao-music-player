fn main() {
    // vendor/lineedit-interface.slint 使用 `interface` 实验特性。Slint 1.19 起
    // SLINT_ENABLE_EXPERIMENTAL_FEATURES 不再接受「1 = 全部打开」，必须点名特性
    // （可选值见编译器报错列表）；这里只开实际用到的那一个。
    unsafe {
        std::env::set_var("SLINT_ENABLE_EXPERIMENTAL_FEATURES", "interfaces");
    }
    slint_build::compile("ui/app.slint").expect("compile slint ui");
}
