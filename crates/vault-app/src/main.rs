//! `passpoart` 桌面壳：420×640 马卡龙玻璃紧凑小窗。
//!
//! 设计稿的可执行版：解锁 → 列表 → 详情 的推入流、亮暗双主题、
//! 系统模糊背板 + 自绘标题栏（gpui-ce 的开窗方式，与 ~/rili、~/glowlink 同款）。
//! 解锁走 vault-core 真实链路（Argon2id 派生 + 校验器比对 + 信封解密）；
//! 库文件默认 ~/.passpoart/vault.nodb，可用 PASSPOART_DB 指定。

mod assets;
mod data;
mod platform;
mod theme;
mod view;

fn main() {
    if std::env::var_os("RUST_LOG").is_some() {
        env_logger::init();
    }

    gpui_ce_platform::application()
        // 图标必须注册资源源，否则 `svg()` 静默不画（不报错、不警告）
        .with_assets(assets::Assets)
        .run(|cx: &mut gpui::App| {
            view::open(cx);
            cx.activate(true);
        });
}
