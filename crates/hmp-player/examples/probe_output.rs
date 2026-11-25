//! 音频输出探针（QA 用，不进 CI）：列出 cpal host 的输出设备，并按
//! `open_default_output` 的生产候选策略实际开流，打印各阶段错误。
//! `cargo run -p hmp-player --example probe_output`
//!
//! 平台口径：Unix 只认 server-routed PCM（default/pipewire/pulse），
//! Windows WASAPI 共享模式全接纳（默认端点优先）。诊断"无声/拉起失败"
//! 先跑这个，别猜。
//!
//! 每个设备同时打印 **PCM 标识**（`alsa:pipewire` 形态，生产白名单的唯一
//! 依据）与显示名（cpal 0.18 起是 ALSA hint 描述，只供人读）——两者不是
//! 一回事，2026-11-25 就是拿显示名喂白名单导致候选恒空、daemon 静默。

use rodio::cpal::traits::{DeviceTrait, HostTrait};

/// PCM 标识 + 显示名。
fn label(device: &rodio::cpal::Device) -> String {
    match device.id() {
        Ok(id) => format!("{id}  [display: {device}]"),
        Err(error) => format!("<no id: {error}>  [display: {device}]"),
    }
}

fn main() {
    // 生产候选循环的 warn（"audio device open failed, trying next" 等）只在
    // 有 subscriber 时才落 stderr——探针必须自己装，否则「打印各阶段错误」
    // 是空话。
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_target(false)
        .init();

    let host = rodio::cpal::default_host();
    println!("host id: {:?}", host.id());
    println!("—— all output devices ——");
    if let Ok(devices) = host.output_devices() {
        for device in devices {
            let configs = device
                .supported_output_configs()
                .map(|configs| configs.count())
                .unwrap_or(0);
            println!(
                "  {}  (supported_output_configs: {configs})",
                label(&device)
            );
        }
    }
    println!("—— default_output_device ——");
    match host.default_output_device() {
        Some(device) => println!("  {}", label(&device)),
        None => println!("none"),
    }
    println!("—— production open_default_output() ——");
    match hmp_player::open_default_output() {
        Ok(_stream) => println!("open OK (real device stream created)"),
        Err(error) => println!("open ERR: {error}  (daemon 会回退静默 sink)"),
    }
}
