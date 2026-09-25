//! HoverGroup 黏滞形变（HoverGroup.vue triggerStretch 的 Slint 对应物）回归测试。
//!
//! 数值契约（与 Vue 原型逐项对齐）：
//! - 峰值 = 1 + min(dist/110, 0.6) * viscosity（行距 88px、viscosity 1 → 1.6）
//! - 双段 easeOutQuint：0→72ms 拉到峰、72→240ms 回弹；飞行结束倍率恒归 1
//! - 原点钉在运动前缘：向下运动前缘=下缘，向上=上缘
//! - 首次进入只定位不形变（Vue placed 语义）；离开所有行取消飞行（hideBlock）
//!
//! 注意：testing backend 的 platform 是进程级单例，本文件只放一个 #[test]。

use slint::{ComponentHandle, Global};

fn move_to(ui: &hmp_desktop::AppWindow, x: f32, y: f32) {
    ui.window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(x, y),
        });
}

fn advance(ms: u64) {
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(ms));
}

#[test]
fn hover_stretch_transient_deformation() {
    i_slint_backend_testing::init_no_event_loop();

    let ui = hmp_desktop::AppWindow::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    hmp_desktop::bridge::load_data(&ui);

    // 步进推进 mock 时间让初始布局/动画收敛（同 hover_slider.rs）
    for _ in 0..40 {
        advance(16);
    }

    let bus = hmp_desktop::HoverBus::get(&ui);

    // 首次进入主导航第一行（顶栏下移后窗口 y=92..132）：只定位跳位淡入，不触发形变
    move_to(&ui, 120.0, 112.0);
    advance(16);
    assert!(bus.get_active(), "首次进入应激活总线");
    assert!(
        !bus.get_flying(),
        "首次进入只定位不做形变（Vue placed 语义）"
    );
    assert!(
        (bus.get_fly_parallel() - 1.0).abs() < 1e-6,
        "静止倍率恒为 1"
    );

    // 行 1 → 行 3（y=180..220，dy=+88 向下）：瞬态拉伸，前缘=下缘
    move_to(&ui, 120.0, 200.0);
    assert!(
        (bus.get_fly_peak() - 1.6).abs() < 0.01,
        "峰值应为 1+min(88/110, 0.6)=1.6，实际 {}",
        bus.get_fly_peak()
    );
    assert!(!bus.get_fly_horizontal(), "纵向运动主轴应为垂直");
    assert_eq!(bus.get_fly_edge_y(), 1, "向下运动前缘应钉在下缘");
    assert_eq!(bus.get_fly_edge_x(), 0, "无水平分量时水平前缘居中");
    // 关键帧 offset 0：触发瞬间倍率仍是 1
    assert!(
        (bus.get_fly_parallel() - 1.0).abs() < 1e-6,
        "触发瞬间倍率应为 1"
    );

    // 飞行 240ms：逐帧采样应看到先冲峰后回弹，结束倍率归 1
    let mut max_parallel = 1.0f32;
    for _ in 0..24 {
        advance(16);
        max_parallel = max_parallel.max(bus.get_fly_parallel());
    }
    assert!(
        max_parallel > 1.3,
        "飞行中应有可见瞬态拉伸（理论峰值≈1.6），实测 {max_parallel}"
    );
    assert!(!bus.get_flying(), "240ms 后飞行应结束");
    assert!(
        (bus.get_fly_parallel() - 1.0).abs() < 1e-6,
        "飞行结束后倍率应归 1"
    );

    // 离开所有行（播放条无 HoverItem）：取消进行中的形变（对齐 hideBlock cancel）
    move_to(&ui, 640.0, 770.0);
    advance(16);
    assert!(!bus.get_active(), "离开所有行后总线应收起");
    assert!(!bus.get_flying(), "收起时应取消形变");

    // 冻结几何重入（上次停在行 3 y=180，重入行 1 y=92，dy=-88 向上）：前缘=上缘
    move_to(&ui, 120.0, 112.0);
    assert!(bus.get_active());
    assert!(
        (bus.get_fly_peak() - 1.6).abs() < 0.01,
        "跨间隙重入也按位置增量形变（对齐 Vue lastX/lastY）"
    );
    assert_eq!(bus.get_fly_edge_y(), -1, "向上运动前缘应钉在上缘");
}
