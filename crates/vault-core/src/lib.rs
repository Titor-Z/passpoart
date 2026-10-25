//! `vault-core`：密码库的核心库。
//!
//! 分层纪律（与 ~/rili、~/glowlink 同款）：
//! - 本 crate **零 IO、零 UI**：不碰文件、不碰网络、不碰 gpui；
//! - `model`：明文数据模型（仅存在于解锁后的内存中）；
//! - `crypto`：加解密抽象（算法实现见各 trait 的实现方）；
//! - `keymgr`：密钥层级（主密码 → 主密钥 MK → 库密钥 VK → 条目数据密钥 DK）；
//! - `storage`：本地存储抽象（SQLite 全密文实现后续任务落地）；
//! - `sync`：同步抽象（「瞎子服务器」：只见密文 blob 与版本号）。
//!
//! 桌面壳（vault-app）与命令行（vault-cli）只经由这里的 trait 与类型访问密码库。

pub mod crypto;
pub mod generator;
pub mod keymgr;
pub mod model;
pub mod storage;
pub mod totp;
pub mod sync;

/// 库版本（与 workspace 版本一致，供 CLI/UI 探测能力用）。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 条目 id → blake3 哈希前 16 位十六进制：审计日志用它代替明文 id（计划书第 8 节）。
pub fn audit_hash(id: &str) -> String {
    blake3::hash(id.as_bytes()).to_hex()[..16].to_string()
}
