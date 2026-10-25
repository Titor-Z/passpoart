//! 视图条目模型：解锁后由 vault-core 解密快照填充（取代小样阶段的写死数据）。
//! UI 层只认这里的字段；vault-core 的明文 `Item` 不直接进渲染代码。

use vault_core::model::{Item as CoreItem, ItemKind as CoreKind};
use vault_core::storage::StoredItem;

/// 条目类型：分类色的语义来源（玫瑰=登录 / 薄荷=笔记 / 蜜桃=银行卡 / 薰衣草=身份）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Login,
    Note,
    Card,
    Identity,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Login => "登录",
            Kind::Note => "笔记",
            Kind::Card => "银行卡",
            Kind::Identity => "身份",
        }
    }

    /// 核心枚举 → 视图枚举（一一对应）。
    pub fn from_core(kind: CoreKind) -> Self {
        match kind {
            CoreKind::Login => Kind::Login,
            CoreKind::Note => Kind::Note,
            CoreKind::Card => Kind::Card,
            CoreKind::Identity => Kind::Identity,
        }
    }

    /// 视图枚举 → 核心枚举（编辑表单保存路径用）。
    pub fn to_core(self) -> CoreKind {
        match self {
            Kind::Login => CoreKind::Login,
            Kind::Note => CoreKind::Note,
            Kind::Card => CoreKind::Card,
            Kind::Identity => CoreKind::Identity,
        }
    }
}

/// 解密后的条目快照（视图形态，全部持有所有权）。
pub struct Item {
    /// 条目 id（收藏切换 / 删除 / 打开编辑都要用）。
    pub id: String,
    pub kind: Kind,
    pub title: String,
    /// 次行展示：登录=用户名，其余=类型词或用户自填别名。
    pub sub: String,
    pub url: String,
    /// 密钥正文（复制到剪贴板用；锁定即随快照清空）。
    pub secret: String,
    /// TOTP 共享密钥（base32 种子；动态码在渲染时现算，不存码本身）。
    pub totp_seed: Option<String>,
    pub starred: bool,
    pub note: String,
    /// 密码强度分段（0..=4，本地启发式，不外传）。
    pub strength: u8,
    pub version: u32,
    pub updated_at: i64,
}

impl Item {
    /// 解密后的核心条目 + 落盘元数据 → 视图模型。
    pub fn from_parts(core: &CoreItem, stored: &StoredItem) -> Self {
        Self {
            id: stored.id.clone(),
            kind: Kind::from_core(core.kind),
            title: core.title.clone(),
            sub: if core.subtitle.is_empty() {
                core.kind.label().to_string()
            } else {
                core.subtitle.clone()
            },
            url: core.url.clone(),
            secret: core.secret.clone(),
            totp_seed: core.totp_secret.clone(),
            starred: core.favorite,
            note: core.note.clone(),
            strength: strength_of(&core.secret),
            version: stored.version,
            updated_at: stored.updated_at,
        }
    }
}

/// 密码强度启发式：长度定基线，字符类别加分。纯本地、零依赖、不外传。
pub fn strength_of(secret: &str) -> u8 {
    if secret.is_empty() {
        return 0;
    }
    let mut score = match secret.len() {
        0..8 => 1,
        8..12 => 2,
        12..16 => 3,
        _ => 4,
    };
    let classes = [
        secret.chars().any(|c| c.is_ascii_lowercase()),
        secret.chars().any(|c| c.is_ascii_uppercase()),
        secret.chars().any(|c| c.is_ascii_digit()),
        secret.chars().any(|c| !c.is_ascii_alphanumeric()),
    ]
    .into_iter()
    .filter(|b| *b)
    .count();
    if classes >= 3 {
        score = (score + 1).min(4);
    }
    score
}

/// Unix 秒 → 相对时间文案（列表与详情的更新时间展示）。
pub fn relative_time(ts: i64) -> String {
    let delta = vault_core::storage::now_secs() - ts;
    match delta {
        0..60 => "刚刚".into(),
        60..3600 => format!("{} 分钟前", delta / 60),
        3600..86400 => format!("{} 小时前", delta / 3600),
        86400..172_800 => "昨天".into(),
        172_800..2_592_000 => format!("{} 天前", delta / 86_400),
        _ => format!("{} 个月前", delta / 2_592_000),
    }
}

/// 条目名首字（favicon 占位 glyph；真实站点图标随同步任务接入）。
pub fn first_char(s: &str) -> &str {
    s.chars().next().map(|c| &s[..c.len_utf8()]).unwrap_or("？")
}

/// 搜索匹配：标题/次行/网址含关键词（大小写不敏感；空关键词全过）。
/// 搜索只发生在解锁后的内存快照上，无索引、无落盘。
pub fn search_matches(item: &Item, needle: &str) -> bool {
    let needle = needle.trim();
    if needle.is_empty() {
        return true;
    }
    let n = needle.to_lowercase();
    item.title.to_lowercase().contains(&n)
        || item.sub.to_lowercase().contains(&n)
        || item.url.to_lowercase().contains(&n)
}

/// 列表过滤 chips：全部 / 收藏 / 四种分类。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    Favorite,
    Kind(Kind),
}

impl Filter {
    pub fn matches(self, item: &Item) -> bool {
        match self {
            Filter::All => true,
            Filter::Favorite => item.starred,
            Filter::Kind(k) => item.kind == k,
        }
    }
}

/// 过滤 + 搜索后的条目下标列表。
pub fn indices(items: &[Item], filter: Filter, needle: &str) -> Vec<usize> {
    items
        .iter()
        .enumerate()
        .filter(|(_, it)| filter.matches(it) && search_matches(it, needle))
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 强度启发式：长度定基线、类别加分、空串为 0。
    #[test]
    fn strength_levels() {
        assert_eq!(strength_of(""), 0);
        assert_eq!(strength_of("abc"), 1);
        assert_eq!(strength_of("abcdefgh"), 2);
        assert_eq!(strength_of("abcdefghijkl!A9"), 4); // 长度 16 + 三类字符
    }

    /// 核心条目 → 视图模型：空 subtitle 回退类型词、密钥与 TOTP 种子随行。
    #[test]
    fn from_parts_mapping() {
        let mut core = CoreItem::new(CoreKind::Note, "开发令牌");
        core.secret = "abcdefghijkl!A9".into();
        core.totp_secret = Some("BASE32".into());
        core.favorite = true;
        let stored = StoredItem {
            id: "x".into(),
            kind: vault_core::model::ItemKind::Note,
            favorite: true,
            envelope: vault_core::crypto::Envelope {
                ciphertext: vec![],
                nonce: vec![],
                wrapped_dk: vec![],
                wrap_nonce: vec![],
            },
            version: 3,
            updated_at: 1_000_000,
            deleted_at: None,
        };
        let view = Item::from_parts(&core, &stored);
        assert_eq!(view.sub, "安全笔记"); // 空 subtitle 回退类型词
        assert_eq!(view.totp_seed.as_deref(), Some("BASE32"));
        assert_eq!(view.secret, "abcdefghijkl!A9");
        assert!(view.starred);
        assert_eq!(view.version, 3);
        assert_eq!(view.strength, 4);
    }

    /// 相对时间：刚更新与陈旧条目的分档。
    #[test]
    fn relative_time_buckets() {
        let now = vault_core::storage::now_secs();
        assert_eq!(relative_time(now), "刚刚");
        assert_eq!(relative_time(now - 29 * 86_400), "29 天前");
        assert_eq!(relative_time(now - 30 * 86_400), "1 个月前"); // 30 天整恰好跨分档
    }

    /// 过滤 + 搜索可叠加：收藏/分类互斥，关键词跨标题/用户名/网址命中。
    #[test]
    fn filter_and_search_indices() {
        let mk = |starred: bool, kind: Kind, title: &str, sub: &str, url: &str| Item {
            id: format!("id-{title}"),
            kind,
            title: title.into(),
            sub: sub.into(),
            url: url.into(),
            secret: "x".into(),
            totp_seed: None,
            starred,
            note: String::new(),
            strength: 1,
            version: 1,
            updated_at: 0,
        };
        let items = vec![
            mk(true, Kind::Login, "GitHub", "titor@dev.io", "github.com"),
            mk(false, Kind::Card, "招行信用卡", "尾号 4821", ""),
            mk(true, Kind::Card, "工资卡", "尾号 9900", ""),
        ];
        assert_eq!(indices(&items, Filter::All, ""), vec![0, 1, 2]);
        assert_eq!(indices(&items, Filter::Favorite, ""), vec![0, 2]);
        assert_eq!(indices(&items, Filter::Kind(Kind::Card), ""), vec![1, 2]);
        assert_eq!(indices(&items, Filter::Kind(Kind::Identity), ""), Vec::<usize>::new());
        // 搜索：标题/用户名/网址、大小写不敏感
        assert_eq!(indices(&items, Filter::All, "git"), vec![0]);
        assert_eq!(indices(&items, Filter::All, "TITOR"), vec![0]); // 大小写不敏感
        assert_eq!(indices(&items, Filter::All, "卡"), vec![1, 2]);
        // 过滤 × 搜索叠加
        assert_eq!(indices(&items, Filter::Kind(Kind::Card), "4821"), vec![1]);
        assert_eq!(indices(&items, Filter::All, "不存在"), Vec::<usize>::new());
    }
}
