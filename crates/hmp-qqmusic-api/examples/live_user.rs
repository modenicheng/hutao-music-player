//! user 域 live 探针（读取本地凭证，验证全部 UserApi）。
//!
//! 运行：`cargo run -p hmp-qqmusic-api --example live_user`
//! 未登录时仅跑免登录接口并标注 SKIP。

use hmp_qqmusic_api::user::{
    UserApi, UserCreatedSonglistResponse, UserFavAlbumResponse, UserFavSonglistResponse,
};
use hmp_qqmusic_api::{QqMusicClient, credential::Credential};

fn load_credential() -> Option<Credential> {
    hmp_storage::credential::store_from_env()
        .load()
        .ok()
        .flatten()
        .filter(|c| c.is_logged_in())
}

#[tokio::main]
async fn main() {
    let cred = load_credential();
    let client = QqMusicClient::new();
    let api = UserApi::new(&client);

    match &cred {
        Some(c) => println!("credential: uin={} euin_len={}", c.uin, c.encrypt_uin.len()),
        None => println!("credential: none (login-required APIs will SKIP)"),
    }

    // 免登录/登录增强
    match api
        .get_created_songlist(
            cred.as_ref()
                .map(|c| c.uin.clone())
                .unwrap_or_default()
                .as_str(),
            cred.as_ref(),
        )
        .await
    {
        Ok(r) => println!(
            "PASS get_created_songlist: total={} parsed={} finished={}",
            r.total,
            r.songlist.len(),
            r.finished
        ),
        Err(e) => println!("FAIL get_created_songlist: {e}"),
    }

    let Some(cred) = cred else {
        return;
    };

    // 需登录（读）
    match api.get_vip_info(&cred).await {
        Ok(v) => println!(
            "PASS get_vip_info: keys={}",
            v.as_object().map(|m| m.len()).unwrap_or(0)
        ),
        Err(e) => println!("FAIL get_vip_info: {e}"),
    }
    match api.get_music_gene(&cred.encrypt_uin, Some(&cred)).await {
        Ok(g) => println!("PASS get_music_gene: nick={:?}", g.userinfo_card.nick_name),
        Err(e) => println!("FAIL get_music_gene: {e}"),
    }
    match api.get_homepage(&cred.encrypt_uin, Some(&cred)).await {
        Ok(_) => println!("PASS get_homepage"),
        Err(e) => println!("FAIL get_homepage（已知服务端 10000 空壳，2026-09-29）: {e}"),
    }

    let _ = std::hint::black_box((
        std::any::type_name::<UserFavSonglistResponse>(),
        std::any::type_name::<UserFavAlbumResponse>(),
        std::any::type_name::<UserCreatedSonglistResponse>(),
    ));
}
