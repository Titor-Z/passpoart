//! 条目模型：密码库的核心数据结构（明文形态）。
//!
//! 红线：明文 `Item` 只允许出现在**解锁后的内存**里；落盘与上传的永远是
//! `storage` 模块的密文形态（ciphertext + nonce + key_wrap_dk）。

/// 条目类型：与设计系统的分类色一一对应（登录=玫瑰 / 笔记=薄荷 / 银行卡=蜜桃 / 身份=薰衣草）。
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum ItemKind {
    /// 站点登录：用户名 + 密码 + 网址 + 可选 TOTP。
    Login,
    /// 安全笔记：自由文本。
    Note,
    /// 银行卡：卡号 / 有效期 / 安全码。
    Card,
    /// 身份信息：证件 / 地址等个人资料。
    Identity,
}

impl ItemKind {
    /// 全部类型，按 UI 展示顺序排列（过滤 chips 的顺序以此为准）。
    pub fn all() -> [ItemKind; 4] {
        [ItemKind::Login, ItemKind::Note, ItemKind::Card, ItemKind::Identity]
    }

    /// 中文标签：**仅展示用**；存储与同步一律存枚举名（`serde` 别名后续任务接入）。
    pub fn label(&self) -> &'static str {
        match self {
            ItemKind::Login => "登录",
            ItemKind::Note => "安全笔记",
            ItemKind::Card => "银行卡",
            ItemKind::Identity => "身份信息",
        }
    }
}

/// 条目：解密后的明文形态。字段按初期四类条目的并集裁剪，新类型到齐后再拆子模型。
/// Serialize/Deserialize 只用于「加解密前的 JSON 化」，产物立即进信封，不落盘。
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Item {
    /// 条目唯一标识（UUID v4，生成于创建时，永不复用）。
    pub id: String,
    /// 条目类型。
    pub kind: ItemKind,
    /// 名称（用户可读标题，如站点名）。
    pub title: String,
    /// 登录用户名 / 卡片别名等次行展示字段。
    pub subtitle: String,
    /// 关联网址（仅 Login 类条目使用，可为空）。
    pub url: String,
    /// 密钥正文（密码 / 笔记内容 / 卡号信息，统一装这个字段，展示层按 kind 解读）。
    pub secret: String,
    /// 一次性验证码密钥（TOTP 共享密钥的 base32，未配置为 None）。
    pub totp_secret: Option<String>,
    /// 是否收藏（星标）。
    pub favorite: bool,
    /// 用户备注。
    pub note: String,
    /// 逻辑版本号：每次修改 +1，同步冲突判定依据。
    pub version: u32,
}

impl Item {
    /// 构造新条目的便捷方法：id 由调用方（存储层）生成，version 从 1 起。
    pub fn new(kind: ItemKind, title: impl Into<String>) -> Self {
        Self {
            id: String::new(),
            kind,
            title: title.into(),
            subtitle: String::new(),
            url: String::new(),
            secret: String::new(),
            totp_secret: None,
            favorite: false,
            note: String::new(),
            version: 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 类型清单完整且顺序稳定（UI 过滤 chips 依赖此顺序）。
    #[test]
    fn kind_all_order_stable() {
        let all = ItemKind::all();
        assert_eq!(all.len(), 4);
        assert_eq!(all[0], ItemKind::Login);
        assert_eq!(all[3], ItemKind::Identity);
    }

    /// 中文标签仅作展示，四类互不相同。
    #[test]
    fn kind_labels_distinct() {
        let labels: Vec<&str> = ItemKind::all().iter().map(|k| k.label()).collect();
        let mut sorted = labels.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(labels.len(), sorted.len());
    }

    /// 新条目默认值：未收藏、无 TOTP、版本 1。
    #[test]
    fn item_new_defaults() {
        let item = Item::new(ItemKind::Login, "GitHub");
        assert_eq!(item.title, "GitHub");
        assert!(!item.favorite);
        assert!(item.totp_secret.is_none());
        assert_eq!(item.version, 1);
    }
}
