//! 本地存储：nodblite（单文件 .nodb）上的全密文条目库。
//!
//! 存储选型（TL20261025T191400 后续讨论定稿）：自研 nodblite 而非 SQLite——
//! 纯 Rust 可审计、文档模型天然匹配条目形态、单文件拷贝即备份。
//!
//! 数据布局（三个集合）：
//! - `items`：每条一条文档。`_id`=条目 UUID；`kind`/`favorite`/`version`/
//!   `updated_at`/`deleted_at` 为**明文元数据**（计划书第 8 节：列表过滤与同步
//!   判定需在解密前可用）；正文（标题/用户名/密码/备注…）整体在 `envelope` 密文内。
//! - `meta`：单文档（`_id`="vault"），KDF salt 与参数——换机解密必需。
//! - `audit_log`：审计流水，`item_id` 只存 blake3 哈希前 16 位（绝不记明文）。
//!
//! 安全红线：任何明文正文（Item JSON）只进信封不进库；
//! 红队用例对 .nodb 文件做字节级扫描验证。

use std::path::Path;

use nodblite::query::{Filter, Update};
use nodblite::{Database, Document};
use serde_json::json;

use crate::crypto::Envelope;
use crate::keymgr::{KdfParams, UnlockedVault};
use crate::model::{Item, ItemKind};

/// items 集合名。
const COLLECTION_ITEMS: &str = "items";
/// meta 集合名（单文档）。
const COLLECTION_META: &str = "meta";
/// 审计集合名。
const COLLECTION_AUDIT: &str = "audit_log";
/// meta 单文档固定主键。
const META_DOC_ID: &str = "vault";

/// 条目的落盘形态：明文元数据 + 信封密文（与 items 文档字段一一对应）。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct StoredItem {
    /// 条目 UUID（文档主键 `_id`）。
    #[serde(rename = "_id")]
    pub id: String,
    /// 条目类型（明文元数据：解密前的过滤占位）。
    pub kind: ItemKind,
    /// 是否收藏（明文元数据：星标过滤）。
    pub favorite: bool,
    /// 信封密文（正文全部在此）。
    pub envelope: Envelope,
    /// 逻辑版本号：每次修改 +1，同步冲突判定依据。
    pub version: u32,
    /// 更新时间戳（Unix 秒）。
    pub updated_at: i64,
    /// 软删除时间戳；None = 存活。
    pub deleted_at: Option<i64>,
}

/// 本地条目库契约：测试可实现为内存 Vec，生产实现为 NodbStore。
pub trait ItemStore {
    /// 错误类型由实现方定义。
    type Error: std::fmt::Debug;

    /// 写入或按 id 覆盖一条密文条目（版本递增由调用方保证）。
    fn put(&mut self, item: &StoredItem) -> Result<(), Self::Error>;

    /// 按 id 取一条（含墓碑，供同步判定）。
    fn get(&self, id: &str) -> Result<Option<StoredItem>, Self::Error>;

    /// 列出全部存活条目（解锁后批量解密渲染用）。
    fn list_alive(&self) -> Result<Vec<StoredItem>, Self::Error>;

    /// 软删除：打墓碑，不物理删。
    fn tombstone(&mut self, id: &str, at: i64) -> Result<(), Self::Error>;
}

/// 存储错误：数据库层 / 序列化层 / 加密层三源归一。
#[derive(Debug)]
pub enum StoreError {
    /// nodblite 底层错误。
    Db(String),
    /// JSON 序列化/反序列化错误。
    Serde(String),
    /// 信封解密失败（错误密钥或数据被篡改）。
    Crypto(crate::crypto::CryptoError),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Db(e) => write!(f, "存储错误：{e}"),
            StoreError::Serde(e) => write!(f, "序列化错误：{e}"),
            StoreError::Crypto(e) => write!(f, "解密失败：{e}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<nodblite::DatabaseError> for StoreError {
    fn from(e: nodblite::DatabaseError) -> Self {
        StoreError::Db(e.to_string())
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(e: serde_json::Error) -> Self {
        StoreError::Serde(e.to_string())
    }
}

// nodblite 三类构造错误（Document/Filter/Update）归一到 Db：
// 它们只在「数据形状损坏」时出现，语义上属于存储层错误。
impl From<nodblite::DocumentError> for StoreError {
    fn from(e: nodblite::DocumentError) -> Self {
        StoreError::Db(e.to_string())
    }
}

impl From<nodblite::query::FilterError> for StoreError {
    fn from(e: nodblite::query::FilterError) -> Self {
        StoreError::Db(e.to_string())
    }
}

impl From<nodblite::query::UpdateError> for StoreError {
    fn from(e: nodblite::query::UpdateError) -> Self {
        StoreError::Db(e.to_string())
    }
}

/// 库元数据：解锁所需的最小信息（salt + KDF 参数）。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct VaultMeta {
    /// Argon2id salt（16 字节）。
    pub kdf_salt: Vec<u8>,
    /// Argon2id 参数（换机必须原样还原）。
    pub kdf_params: KdfParams,
    /// 本地口令校验器（keymgr::derive_local_verifier 的 hex；不存口令本身）。
    pub verifier: String,
    /// 创建时间（Unix 秒）。
    pub created_at: i64,
    /// 最后更新时间（Unix 秒）。
    pub updated_at: i64,
}

/// 解锁失败：口令错误 / KDF 异常。
#[derive(Debug)]
pub enum UnlockError {
    /// 校验器不匹配：口令错误。
    BadPassword,
    /// 派生过程出错（参数被篡改等）。
    Kdf(crate::keymgr::KdfError),
}

impl std::fmt::Display for UnlockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UnlockError::BadPassword => write!(f, "主密码错误"),
            UnlockError::Kdf(e) => write!(f, "解锁失败：{e}"),
        }
    }
}

impl std::error::Error for UnlockError {}

impl VaultMeta {
    /// 口令解锁：派生 MK → 校验器比对（不碰任何条目密文）→ 开库会话。
    pub fn unlock(&self, password: &str) -> Result<UnlockedVault, UnlockError> {
        let mk = crate::keymgr::derive_master_key(password, &self.kdf_salt, self.kdf_params)
            .map_err(UnlockError::Kdf)?;
        if crate::keymgr::derive_local_verifier(&mk) != self.verifier {
            return Err(UnlockError::BadPassword);
        }
        Ok(UnlockedVault::from_master_key(mk))
    }
}

/// 审计条目：只记元数据，绝不记明文。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AuditEntry {
    /// 动作名（unlock / lock / put / tombstone …）。
    pub action: String,
    /// 条目 id 的 blake3 哈希前 16 位十六进制；无条目上下文时为空串。
    pub item_id_hash: String,
    /// 时间戳（Unix 秒）。
    pub ts: i64,
}

/// 审计排序键：单调递增（ts*1e6 + 进程内计数；时钟回拨由计数位兜底近似）。
fn audit_seq() -> i64 {
    static COUNTER: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    now_secs() * 1_000_000 + n
}

/// nodblite 实现的条目库：进程内单文件，redb ACID 底座。
pub struct NodbStore {
    /// nodblite 数据库句柄。
    db: Database,
}

impl NodbStore {
    /// 打开或创建 .nodb 库文件（create 要求文件不存在，open 校验魔数与版本）。
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let db = if path.exists() {
            Database::open(path)?
        } else {
            // 新建库前补齐父目录：redb 不会自动建目录（~/.passpoart 首次 init 的真实场景）
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| StoreError::Db(format!("创建父目录失败: {e}")))?;
                }
            }
            Database::create(path)?
        };
        Ok(Self { db })
    }

    /// 写入（或重写）库元数据：幂等，以固定主键单文档存放。
    pub fn init_vault_meta(
        &self,
        salt: &[u8],
        params: KdfParams,
        verifier: &str,
    ) -> Result<(), StoreError> {
        let now = now_secs();
        let meta = VaultMeta {
            kdf_salt: salt.to_vec(),
            kdf_params: params,
            verifier: verifier.to_string(),
            created_at: now,
            updated_at: now,
        };
        let mut doc = Document::from_value(serde_json::to_value(&meta)?)?;
        doc.insert("_id", json!(META_DOC_ID));
        // 单文档 upsert：先删后插（写频极低，崩溃窗口由 redb 事务兜底每次提交原子）
        self.db.delete_by_id(COLLECTION_META, META_DOC_ID)?;
        self.db.insert(COLLECTION_META, &mut doc)?;
        Ok(())
    }

    /// 读取库元数据；未初始化的库返回 None。
    pub fn read_vault_meta(&self) -> Result<Option<VaultMeta>, StoreError> {
        match self.db.get_by_id(COLLECTION_META, META_DOC_ID)? {
            Some(map) => Ok(Some(serde_json::from_value(serde_json::Value::Object(map))?)),
            None => Ok(None),
        }
    }

    /// 追加一条审计流水（item_id 自动转哈希，绝不落明文 id）。
    pub fn log_action(&self, action: &str, item_id: Option<&str>) -> Result<(), StoreError> {
        let entry = AuditEntry {
            action: action.to_string(),
            item_id_hash: match item_id {
                Some(id) => crate::audit_hash(id),
                None => String::new(),
            },
            ts: now_secs(),
        };
        let mut doc = Document::from_value(serde_json::to_value(&entry)?)?;
        doc.insert("seq", json!(audit_seq()));
        self.db.insert(COLLECTION_AUDIT, &mut doc)?;
        Ok(())
    }

    /// 最近 N 条审计流水（按插入时序倒序近似：ObjectId 字典序 = 时间序）。
    pub fn recent_audit(&self, limit: usize) -> Result<Vec<AuditEntry>, StoreError> {
        // 同秒内 ObjectId 的随机尾部不保序，排序键用单调 seq（log_action 写入）。
        #[derive(serde::Deserialize)]
        struct Row {
            #[serde(flatten)]
            entry: AuditEntry,
            seq: i64,
        }
        let filter = Filter::from_value(&json!({}))?;
        let mut rows: Vec<Row> = self
            .db
            .find(COLLECTION_AUDIT, &filter)?
            .into_iter()
            .map(|map| serde_json::from_value(serde_json::Value::Object(map)))
            .collect::<Result<_, _>>()?;
        rows.sort_by_key(|r| r.seq);
        let start = rows.len().saturating_sub(limit);
        Ok(rows[start..].iter().rev().map(|r| r.entry.clone()).collect())
    }

    /// 空 filter（匹配集合内全部文档）。
    fn empty_filter() -> Result<Filter, StoreError> {
        Ok(Filter::from_value(&json!({}))?)
    }

    /// 文档 Map → StoredItem。
    fn doc_to_item(map: serde_json::Map<String, serde_json::Value>) -> Result<StoredItem, StoreError> {
        Ok(serde_json::from_value(serde_json::Value::Object(map))?)
    }
}

impl ItemStore for NodbStore {
    type Error = StoreError;

    /// 覆盖写：先删后插。两个事务间崩溃的窗口由上层 version 兜底
    /// （旧版或新版必居其一，不会出现半条）。
    fn put(&mut self, item: &StoredItem) -> Result<(), StoreError> {
        let mut doc = Document::from_value(serde_json::to_value(item)?)?; // _id 已由 serde rename 带入
        self.db.delete_by_id(COLLECTION_ITEMS, &item.id)?;
        let inserted = self.db.insert(COLLECTION_ITEMS, &mut doc)?;
        debug_assert_eq!(inserted, item.id, "插入主键应与条目 id 一致");
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Option<StoredItem>, StoreError> {
        match self.db.get_by_id(COLLECTION_ITEMS, id)? {
            Some(map) => Ok(Some(Self::doc_to_item(map)?)),
            None => Ok(None),
        }
    }

    fn list_alive(&self) -> Result<Vec<StoredItem>, StoreError> {
        let filter = Self::empty_filter()?;
        Ok(self
            .db
            .find(COLLECTION_ITEMS, &filter)?
            .into_iter()
            .map(Self::doc_to_item)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|it| it.deleted_at.is_none())
            .collect())
    }

    fn tombstone(&mut self, id: &str, at: i64) -> Result<(), StoreError> {
        let update = Update::from_value(&json!({ "$set": { "deleted_at": at, "updated_at": at } }))?;
        match self.db.update_by_id(COLLECTION_ITEMS, id, &update)? {
            true => Ok(()),
            false => Err(StoreError::Db(format!("条目不存在，无法打墓碑：{id}"))),
        }
    }
}

// ================================================================ 桥接与工具

/// 当前时间（Unix 秒）。
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 生成条目 id：UUID v4（OsRng 16 字节 + 版本/变体位；不引 uuid crate）。
pub fn new_item_id() -> String {
    use chacha20poly1305::aead::rand_core::RngCore;
    let mut b = [0u8; 16];
    chacha20poly1305::aead::OsRng.fill_bytes(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40; // version 4
    b[8] = (b[8] & 0x3f) | 0x80; // RFC 4122 variant
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// 明文条目 → 落盘形态：JSON 化 → 信封加密（AAD 绑定条目 id）。
/// item.id 为空时生成新 UUID（返回值带回，调用方据此回填）。
pub fn seal_item_for_storage(
    vault: &UnlockedVault,
    item: &Item,
) -> Result<StoredItem, StoreError> {
    let id = if item.id.is_empty() {
        new_item_id()
    } else {
        item.id.clone()
    };
    // 密文内的 id 必须与 AAD/主键三方一致：先回填再序列化
    let mut filled = item.clone();
    filled.id = id.clone();
    let plaintext = serde_json::to_vec(&filled)?;
    let envelope = vault
        .seal_item(&plaintext, id.as_bytes())
        .map_err(StoreError::Crypto)?;
    Ok(StoredItem {
        id,
        kind: item.kind,
        favorite: item.favorite,
        envelope,
        version: item.version,
        updated_at: now_secs(),
        deleted_at: None,
    })
}

/// 落盘形态 → 明文条目：信封解密（AAD 校验 id）→ JSON 反序列化。
pub fn open_item_from_storage(
    vault: &UnlockedVault,
    stored: &StoredItem,
) -> Result<Item, StoreError> {
    let plaintext = vault
        .open_item(&stored.envelope, stored.id.as_bytes())
        .map_err(StoreError::Crypto)?;
    let item: Item = serde_json::from_slice(&plaintext)?;
    // 密文内的 id 必须与文档主键一致（AAD 已在密码学层强制，这里再显式断言一次）
    debug_assert_eq!(item.id, stored.id, "信封内 id 与主键不一致");
    Ok(item)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymgr::{generate_salt, KdfParams};

    /// 弱 KDF 参数：测试加速（生产路径用 KdfParams::default()）。
    fn weak_params() -> KdfParams {
        KdfParams { memory_kib: 8192, iterations: 1, parallelism: 1 }
    }

    /// 唯一临时 .nodb 路径（仿 nosqlite 自身测试的做法）。
    fn temp_nodb(name: &str) -> std::path::PathBuf {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "passpoart_test_{}_{}_{}.nodb",
            name,
            std::process::id(),
            n
        ))
    }

    /// 构造一条明文条目（含敏感字段，供红队扫描用）。
    fn plain_item() -> Item {
        Item {
            id: String::new(),
            kind: ItemKind::Login,
            title: "GitHub-Secret-Title".into(),
            subtitle: "titor-secret@dev.io".into(),
            url: "github.com/login".into(),
            secret: "plain-secret-password-42".into(),
            totp_secret: Some("BASE32TOTP".into()),
            favorite: true,
            note: "机密备注内容 red-team-marker".into(),
            version: 1,
        }
    }

    /// 造库 + 存一条 → 返回 (路径, 存储条目)。
    fn setup_with_one_item(name: &str) -> (std::path::PathBuf, StoredItem) {
        let path = temp_nodb(name);
        let salt = generate_salt();
        let mut store = NodbStore::open(&path).unwrap();
        store.init_vault_meta(&salt, weak_params(), "test-verifier").unwrap();

        let vault = UnlockedVault::unlock("right password", &salt, weak_params()).unwrap();
        let stored = seal_item_for_storage(&vault, &plain_item()).unwrap();
        store.put(&stored).unwrap();
        store.log_action("put", Some(&stored.id)).unwrap();
        (path, stored)
    }

    /// 契约：put → get 往返还原；信封内明文字段被完整还原。
    #[test]
    fn nodb_put_get_roundtrip() {
        let (path, stored) = setup_with_one_item("roundtrip");
        // 注意：redb 文件锁同进程独占——旧句柄存活期间不得重开同一文件，
        // 所以 meta 一律从当前句柄读（见 Renzhi 认知修正条目）。
        let store = NodbStore::open(&path).unwrap();
        let meta = store.read_vault_meta().unwrap().unwrap();
        let back = store.get(&stored.id).unwrap().unwrap();
        assert_eq!(back.id, stored.id);
        assert_eq!(back.kind, ItemKind::Login);
        assert!(back.favorite);
        assert_eq!(back.envelope.nonce, stored.envelope.nonce);

        let vault = UnlockedVault::unlock("right password", &meta.kdf_salt, weak_params()).unwrap();
        let item = open_item_from_storage(&vault, &back).unwrap();
        assert_eq!(item.title, "GitHub-Secret-Title");
        assert_eq!(item.secret, "plain-secret-password-42");
        let _ = std::fs::remove_file(&path);
    }

    /// 从库文件读回 KDF salt（解锁路径的最小验证）。
    fn read_meta_salt(path: &Path) -> Vec<u8> {
        let store = NodbStore::open(path).unwrap();
        store.read_vault_meta().unwrap().unwrap().kdf_salt
    }

    /// 契约：墓碑后 list_alive 消失、get 保留（同步判定需要）。
    #[test]
    fn nodb_tombstone_semantics() {
        let (path, stored) = setup_with_one_item("tombstone");
        let mut store = NodbStore::open(&path).unwrap();
        assert_eq!(store.list_alive().unwrap().len(), 1);
        store.tombstone(&stored.id, 2000).unwrap();
        assert_eq!(store.list_alive().unwrap().len(), 0);
        assert!(store.get(&stored.id).unwrap().unwrap().deleted_at.is_some());
        let _ = std::fs::remove_file(&path);
    }

    /// 契约：覆盖写以 id 为准（同 id 再 put = 更新而非新增）。
    #[test]
    fn nodb_put_overwrites_same_id() {
        let (path, stored) = setup_with_one_item("overwrite");
        let mut store = NodbStore::open(&path).unwrap();
        let mut updated = stored.clone();
        updated.version = 2;
        updated.favorite = false;
        store.put(&updated).unwrap();
        let all = store.list_alive().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].version, 2);
        assert!(!all[0].favorite);
        let _ = std::fs::remove_file(&path);
    }

    /// 审计：只存哈希与元数据，可按时间序取回最近条目。
    #[test]
    fn nodb_audit_metadata_only() {
        let (path, stored) = setup_with_one_item("audit");
        let store = NodbStore::open(&path).unwrap();
        store.log_action("lock", None).unwrap();
        let recent = store.recent_audit(10).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].action, "lock"); // 最新在前
        assert_eq!(recent[1].action, "put");
        assert_eq!(recent[1].item_id_hash.len(), 16); // blake3 前 16 位
        // 审计里绝不能出现条目 id 明文
        let raw = serde_json::to_string(&recent).unwrap();
        assert!(!raw.contains(&stored.id));
        let _ = std::fs::remove_file(&path);
    }

    /// 红队（计划书 13-1）：拷走 .nodb 文件，字节级扫描不得出现任何明文正文。
    #[test]
    fn redteam_file_contains_no_plaintext() {
        let (path, _stored) = setup_with_one_item("redteam");
        // 关库落盘后再读文件字节
        drop(NodbStore::open(&path).unwrap());
        let bytes = std::fs::read(&path).unwrap();
        for marker in [
            "GitHub-Secret-Title",
            "titor-secret@dev.io",
            "plain-secret-password-42",
            "red-team-marker",
            "BASE32TOTP",
        ] {
            assert!(
                !bytes
                    .windows(marker.len())
                    .any(|w| w == marker.as_bytes()),
                "明文 {marker} 泄漏进 .nodb 文件"
            );
        }
        let _ = std::fs::remove_file(&path);
    }

    /// 红队（计划书 13-1 的换机/错误口令变体）：错误口令解锁后逐条解密必败。
    #[test]
    fn redteam_wrong_password_opens_nothing() {
        let (path, stored) = setup_with_one_item("wrongpw");
        let salt = read_meta_salt(&path);
        let impostor = UnlockedVault::unlock("wrong password", &salt, weak_params()).unwrap();
        let store = NodbStore::open(&path).unwrap();
        for it in store.list_alive().unwrap() {
            assert!(open_item_from_storage(&impostor, &it).is_err());
        }
        // 对照组：正确口令可解
        let vault = UnlockedVault::unlock("right password", &salt, weak_params()).unwrap();
        assert!(open_item_from_storage(&vault, &stored).is_ok());
        let _ = std::fs::remove_file(&path);
    }

    /// 库元数据：salt 与 KDF 参数写读一致（换机解密的参数还原路径）。
    #[test]
    fn nodb_meta_roundtrip() {
        let path = temp_nodb("meta");
        {
            let store = NodbStore::open(&path).unwrap();
            assert!(store.read_vault_meta().unwrap().is_none()); // 未初始化
            let salt = generate_salt();
            store.init_vault_meta(&salt, weak_params(), "test-verifier").unwrap();
        }
        let store = NodbStore::open(&path).unwrap();
        let meta = store.read_vault_meta().unwrap().unwrap();
        assert_eq!(meta.kdf_params.memory_kib, 8192);
        assert_eq!(meta.kdf_salt.len(), 16);
        let _ = std::fs::remove_file(&path);
    }
}
