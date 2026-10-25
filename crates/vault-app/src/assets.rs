//! 资源：图标 svg 的唯一来源（与 glowlink 同款机制）。
//!
//! `svg().path(..)` 只给路径，真正取字节靠 `AssetSource`。**不注册源，图标是静默不画的**
//! （不报错、不警告）。加一个图标要同时出现在两处：`assets/icons/` 文件 + 这张表。

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

const ICONS: &[(&str, &[u8])] = &[
    ("icons/back.svg", include_bytes!("../assets/icons/back.svg")),
    ("icons/check.svg", include_bytes!("../assets/icons/check.svg")),
    ("icons/clipboard.svg", include_bytes!("../assets/icons/clipboard.svg")),
    ("icons/clock.svg", include_bytes!("../assets/icons/clock.svg")),
    ("icons/chevron.svg", include_bytes!("../assets/icons/chevron.svg")),
    ("icons/close.svg", include_bytes!("../assets/icons/close.svg")),
    ("icons/database.svg", include_bytes!("../assets/icons/database.svg")),
    ("icons/copy.svg", include_bytes!("../assets/icons/copy.svg")),
    ("icons/dice.svg", include_bytes!("../assets/icons/dice.svg")),
    ("icons/eye.svg", include_bytes!("../assets/icons/eye.svg")),
    ("icons/hello.svg", include_bytes!("../assets/icons/hello.svg")),
    ("icons/link.svg", include_bytes!("../assets/icons/link.svg")),
    ("icons/lock.svg", include_bytes!("../assets/icons/lock.svg")),
    ("icons/minus.svg", include_bytes!("../assets/icons/minus.svg")),
    ("icons/moon.svg", include_bytes!("../assets/icons/moon.svg")),
    ("icons/pencil.svg", include_bytes!("../assets/icons/pencil.svg")),
    ("icons/plus.svg", include_bytes!("../assets/icons/plus.svg")),
    ("icons/search.svg", include_bytes!("../assets/icons/search.svg")),
    ("icons/settings.svg", include_bytes!("../assets/icons/settings.svg")),
    ("icons/shield.svg", include_bytes!("../assets/icons/shield.svg")),
    ("icons/square.svg", include_bytes!("../assets/icons/square.svg")),
    ("icons/star-off.svg", include_bytes!("../assets/icons/star-off.svg")),
    ("icons/star.svg", include_bytes!("../assets/icons/star.svg")),
    ("icons/sun.svg", include_bytes!("../assets/icons/sun.svg")),
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(ICONS
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| SharedString::from(*name))
            .collect())
    }
}
