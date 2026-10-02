//! daemon 构建指纹：对 `src/**.rs` + `Cargo.toml` 做稳定哈希，注入
//! `HMP_BUILD_CODE_HASH` 环境变量（lib.rs 以 `BUILD_CODE` 常量暴露）。
//!
//! 用途：桌面端 `connect_or_spawn` 经 IPC `DaemonState.backend_build` 与自身
//! 链接值比对，不一致 = 正在运行的 daemon 是旧代码（重新编译不会替换长驻
//! daemon，「陈旧常驻 daemon」是 §16 零号发现、§18 再次实锤的复发陷阱）→
//! 自动重启后端。
//!
//! 实现约束：
//! - 不发 `rerun-if-*` 指令 → cargo 默认语义（包内任一文件变化即重跑），
//!   指纹随 daemon 源码变化；hmp-desktop 依赖本 crate，daemon 重编必然连带
//!   桌面端重链，两侧取值恒同源。
//! - 自带 FNV-1a 而非 DefaultHasher：指纹必须跨 rustc 版本/机器稳定，否则
//!   同一份代码换工具链重编会误判代际。

use std::fs;
use std::path::{Path, PathBuf};

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv1a(hash: u64, data: &[u8]) -> u64 {
    let mut hash = hash;
    for &b in data {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// 递归收集 dir 下全部 `*.rs`（路径排序后参与哈希，目录序无关）。
fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn main() {
    let mut sources = Vec::new();
    collect_rs(Path::new("src"), &mut sources);
    sources.push(PathBuf::from("Cargo.toml"));
    sources.sort();
    let mut hash = FNV_OFFSET;
    for path in &sources {
        hash = fnv1a(hash, path.to_string_lossy().as_bytes());
        hash = fnv1a(hash, b"\0");
        match fs::read(path) {
            Ok(bytes) => hash = fnv1a(hash, &bytes),
            Err(e) => panic!("build fingerprint: cannot read {}: {e}", path.display()),
        }
        hash = fnv1a(hash, b"\0");
    }
    println!("cargo:rustc-env=HMP_BUILD_CODE_HASH={hash:016x}");
}
