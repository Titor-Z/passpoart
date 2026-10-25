//! 同步抽象：「瞎子服务器」契约——服务端只见密文 blob、条目 id 与版本号。
//!
//! 计划书第 9 节要点（真实现于 Phase 3）：
//! - 上传内容 = 密文三元组 + version + 时间戳；服务端不索引任何明文字段；
//! - 增量拉取：version + updated_at 大于本地者；
//! - 冲突：同 id 双端版本分叉 → **保留双份**标记 conflict，由用户手动合并，
//!   绝不自动「选最新覆盖」；
//! - 删除走墓碑（tombstone），双端都收到后再物理清理。

use crate::storage::StoredItem;

/// 一次拉取的结果：新条目 + 需要用户裁决的冲突对。
pub struct PullResult {
    /// 服务端比本地新的条目（密文形态，回来后本地解密）。
    pub newer: Vec<StoredItem>,
    /// 双端都改过的冲突条目（保留双份，UI 标记 conflict）。
    pub conflicts: Vec<StoredItem>,
}

/// 同步引擎契约：实现方对接自建 axum 服务或 WebDAV。
pub trait SyncEngine {
    /// 错误类型由实现方定义（网络 / 鉴权 / 协议错误）。
    type Error: std::fmt::Debug;

    /// 推送本地领先版本（含墓碑）。
    fn push(&mut self, items: &[StoredItem]) -> Result<(), Self::Error>;

    /// 拉取远端增量；冲突只上报，不自动合并。
    fn pull(&self) -> Result<PullResult, Self::Error>;
}
