//! daemon 构建指纹：对 daemon 及其全部 workspace 依赖 crate 的 `src/**.rs` +
//! 各自 `Cargo.toml` 做稳定哈希，注入 `HMP_BUILD_CODE_HASH` 环境变量（lib.rs 以
//! `BUILD_CODE` 常量暴露）。
//!
//! 用途：桌面端 `connect_or_spawn` 经 IPC `DaemonState.backend_build` 与自身
//! 链接值比对，不一致 = 正在运行的 daemon 是旧代码（重新编译不会替换长驻
//! daemon，「陈旧常驻 daemon」是 §16 零号发现、§18 再次实锤的复发陷阱）→
//! 自动重启后端。
//!
//! 作用域（S12，AUDIT §25②）：原实现只哈希 daemon 自身 `src/**`——行为变更
//! 落在依赖 crate（如 S9 的 hmp-storage persist_cover 派生封面）时指纹不变，
//! 陈旧检测对依赖级变更失明（E1 两臂同指纹实证）。现覆盖 daemon 链接的全部
//! workspace 成员。
//!
//! 实现约束：
//! - 不发 `rerun-if-*` 指令 → cargo 默认语义（包内任一文件变化即重跑）；
//!   跨 crate 路径以 `CARGO_MANIFEST_DIR/../<crate>/src` 相对寻址，宿主工作区
//!   结构固定（`crates/<name>/`），无运行时发现。依赖 crate 变更时 daemon 的
//!   build.rs 是否重跑由 cargo 对「依赖指纹输入变化 → 重编译依赖 → 重链
//!   hmp-daemon → 重跑其 build.rs」的传播保证（实测 §18/S11 语义成立）；
//!   hmp-desktop 依赖本 crate，daemon 重编必然连带桌面端重链，两侧取值恒
//!   同源。
//! - 自带 FNV-1a 而非 DefaultHasher：指纹必须跨 rustc 版本/机器稳定，否则
//!   同一份代码换工具链重编会误判代际。

use std::fs;
use std::path::{Path, PathBuf};

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// daemon 链接的全部 workspace 成员（Cargo.toml members 除去 daemon 自身；
/// hmp-qqmusic-api 虽独立发布但 daemon 运行时链接它，行为变更同样须换指纹）。
const WORKSPACE_CRATES: &[&str] = &[
    "hmp-cli",
    "hmp-core",
    "hmp-desktop",
    "hmp-media",
    "hmp-mpris",
    "hmp-player",
    "hmp-qqmusic-api",
    "hmp-smtc",
    "hmp-storage",
];

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

/// 一个 crate 的指纹输入：`src/**.rs` + `Cargo.toml`（排序由调用方统一做）。
fn collect_crate(manifest_dir: &Path, out: &mut Vec<PathBuf>) {
    collect_rs(&manifest_dir.join("src"), out);
    out.push(manifest_dir.join("Cargo.toml"));
}

fn main() {
    let manifest_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("daemon lives at crates/<name>/")
        .to_path_buf();

    let mut sources = Vec::new();
    collect_crate(&manifest_dir, &mut sources);
    for name in WORKSPACE_CRATES {
        collect_crate(&workspace_root.join("crates").join(name), &mut sources);
    }
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
