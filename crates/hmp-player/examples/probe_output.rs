//! 音频输出探针（QA 用，不进 CI）：列出 cpal host 的输出设备，并按
//! `open_default_output` 的生产候选策略实际开流，打印各阶段错误。
//! `cargo run -p hmp-player --example probe_output`
//!
//! 平台口径：Unix 只认 server-routed PCM（default/pipewire/pulse），
//! Windows WASAPI 共享模式全接纳（默认端点优先）。诊断"无声/拉起失败"
//! 先跑这个，别猜。

use rodio::cpal::traits::{DeviceTrait, HostTrait};

fn main() {
    let host = rodio::cpal::default_host();
    println!("host id: {:?}", host.id());
    println!("—— all output devices ——");
    if let Ok(devices) = host.output_devices() {
        for device in devices {
            let name = device.name().unwrap_or_else(|e| format!("<name err: {e}>"));
            let configs = device
                .supported_output_configs()
                .map(|configs| configs.count())
                .unwrap_or(0);
            println!("  {name}  (supported_output_configs: {configs})");
        }
    }
    println!("—— default_output_device ——");
    match host.default_output_device() {
        Some(device) => println!("name: {:?}", device.name().ok()),
        None => println!("none"),
    }
    println!("—— production open_default_output() ——");
    match hmp_player::open_default_output() {
        Ok(_stream) => println!("open OK (real device stream created)"),
        Err(error) => println!("open ERR: {error}  (daemon 会回退静默 sink)"),
    }
}
