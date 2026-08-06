# hmp-qqmusic-api

非官方 QQ 音乐（QQ Music）API 的 Rust 客户端。

- 行为以 [L-1124/QQMusicApi](https://github.com/L-1124/QQMusicApi)（Python, GPL-3.0-or-later）
  为参考实现与差分测试 Oracle；
- 协议与业务逻辑全部在 Rust 中实现，无 Python 运行时依赖；
- 面向 Linux 桌面（Wayland）的 HMP 播放器协议层，也可独立使用。

## 快速开始

```rust,no_run
use hmp_qqmusic_api::QqMusicClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = QqMusicClient::new();
    // 免登录搜索（阶段 A 示例；完整接口随移植推进逐步开放）
    let result = client.search_track("周杰伦", 5).await?;
    println!("{result:?}");
    Ok(())
}
```

## 许可证

GPL-3.0-or-later。本 crate 是上游 GPL 参考实现的移植（结构性复制），
许可证与上游一致；不含用户 Cookie、官方客户端二进制或受版权保护的音乐内容。
