//! CLI 统一 JSON 输出（`--json`；面向 agent 程序化消费）。
//!
//! 约定：
//! - 数据一律写 stdout；`--json` 时为 pretty JSON（UTF-8，无 ANSI 色）；
//! - 错误不进 stdout：非 JSON 模式照旧 `error: …` 到 stderr；JSON 模式下
//!   main 以 `{"error": "…"}` 单对象输出到 stdout 并以非零码退出；
//! - 结构化页类型（TopDetailPage/CommentPage/SearchPage 等，自带
//!   total/has_more 分页元数据）直接序列化；本地 sqlite 列表命令在
//!   调用点以 `json!({"total", "offset", "items"})` 信封包装。

use std::io::Write;

use serde::Serialize;

/// JSON 模式下输出值（pretty；失败错误上抛）。
pub fn print<T: Serialize>(value: &T) -> Result<(), Box<dyn std::error::Error>> {
    let text = serde_json::to_string_pretty(value)?;
    let mut out = std::io::stdout().lock();
    writeln!(out, "{text}")?;
    out.flush()?;
    Ok(())
}
