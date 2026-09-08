//! 一次性音频输出探针（QA 用，不进 CI）：列出 cpal ALSA host 的输出设备并
//! 按 `open_default_output` 的候选策略逐个尝试，打印各阶段错误。
//! `cargo run -p hmp-player --example probe_output`

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
    println!("—— try open candidates in policy order ——");
    for name in ["default", "pipewire", "pulse"] {
        let Some(device) = host.output_devices().ok().and_then(|mut devices| {
            devices.find(|device| device.name().ok().as_deref() == Some(name))
        }) else {
            println!("{name}: not enumerated");
            continue;
        };
        match rodio::OutputStreamBuilder::from_device(device) {
            Ok(builder) => match builder.open_stream() {
                Ok(_) => println!("{name}: open OK"),
                Err(error) => println!("{name}: open_stream ERR: {error}"),
            },
            Err(error) => println!("{name}: from_device ERR: {error}"),
        }
    }
}
