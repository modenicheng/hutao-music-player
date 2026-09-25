//! 诊断探针：无 GUI 复现 backend::connect_or_spawn 的拉起链路。
//! 用法：cargo run -p hmp-desktop --example spawn_probe
//! 依次打印：初始连接错误 / 二进制解析结果 / spawn 结果 / 就绪重试轨迹。

fn main() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");

    let exe = hmp_desktop::backend::resolve_backend_binary();
    println!("[probe] resolve_backend_binary = {exe:?}");

    let outcome = rt.block_on(async { hmp_desktop::backend::connect_or_spawn().await });
    match &outcome {
        Ok(_) => println!("[probe] connect_or_spawn OK"),
        Err(e) => println!("[probe] connect_or_spawn ERR: {e}"),
    }

    // 失败时补一刀：直接看 connect() 对不存在管道返回的 io error 细节。
    if let Err(err) = outcome {
        println!("[probe] final error debug = {err:?}");
    }
    // 观察窗口：spawn 成功但就绪慢的场合，额外等 5s 再探测一次。
    rt.block_on(async {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    });
    let late =
        rt.block_on(async { hmp_desktop::backend::request(hmp_core::Request::Status).await });
    println!("[probe] late Status = {late:?}");
}
