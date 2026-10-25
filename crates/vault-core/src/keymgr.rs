//! 密钥层级：主密码 →（Argon2id）主密钥 MK →（blake3 域分离）库密钥 VK →（信封）条目 DK。
//!
//! ```text
//! 主密码（仅记忆，永不落盘、永不入系统凭据库）
//!   └─ Argon2id(salt 16B, KdfParams) → MK 32B（Zeroizing）
//!        ├─ VK = blake3::derive_key("passpoart/vault-key/v1", MK)   ← 本模块实现
//!        └─ AK = blake3::derive_key("passpoart/auth-key/v1",  MK)   ← 同步任务接入
//! 条目级：DK 每条随机 32B，见 crypto::seal_envelope。
//! ```
//!
//! 红线（AGENTS 与计划书第 4 节）：MK / VK / DK / 主密码**永不**写入
//! 系统凭据库；全部经 Zeroizing 包装，drop 即清零。

use argon2::{Algorithm, Argon2, Params, Version};
use zeroize::{Zeroize, Zeroizing};

use crate::crypto::{open_envelope, seal_envelope, CryptoError, Envelope};

/// VK 派生的 context 域：改这串等于换库（所有条目需重新包裹 DK），版本化命名防误改。
pub const CONTEXT_VAULT_KEY: &str = "passpoart/vault-key/v1";
/// AK 派生的 context 域：预留给零知识登录（同步任务），先占位防撞名。
pub const CONTEXT_AUTH_KEY: &str = "passpoart/auth-key/v1";
/// 本地口令校验器的 context 域：verifier 随 vault_meta 落盘，解锁先比对其再开库。
pub const CONTEXT_LOCAL_VERIFIER: &str = "passpoart/local-verifier/v1";

/// Argon2id 参数：换机解密与防爆破的平衡点（计划书：memory ≥ 64MB、iterations ≥ 3）。
/// Serialize/Deserialize：随 vault_meta 表落盘（换机解密必须还原同一组参数）。
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub struct KdfParams {
    /// 内存用量（KiB）。
    pub memory_kib: u32,
    /// 迭代次数。
    pub iterations: u32,
    /// 并行度。
    pub parallelism: u32,
}

/// 默认参数：目标解锁耗时 < 800ms（计划书风险表第 5 条），实机调参后覆盖。
impl Default for KdfParams {
    fn default() -> Self {
        Self {
            memory_kib: 65536,
            iterations: 3,
            parallelism: 1,
        }
    }
}

/// KDF 出错：参数非法（内存/迭代低于下限）或派生失败。
#[derive(Debug)]
pub enum KdfError {
    /// 参数低于安全下限，拒绝执行（防「顺手调弱」）。
    ParamsTooWeak,
    /// Argon2id 内部错误。
    Hash(String),
}

impl std::fmt::Display for KdfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KdfError::ParamsTooWeak => write!(f, "KDF 参数低于安全下限"),
            KdfError::Hash(e) => write!(f, "Argon2id 派生失败：{e}"),
        }
    }
}

impl std::error::Error for KdfError {}

/// Argon2id 最小 salt 长度（算法要求 ≥ 8 字节，本项目固定 16 字节）。
pub const SALT_LEN: usize = 16;

/// 生成 KDF salt（CSPRNG）。独立成函数是因为 salt 生成只有这一处正确入口。
pub fn generate_salt() -> [u8; SALT_LEN] {
    use chacha20poly1305::aead::rand_core::RngCore;
    let mut salt = [0u8; SALT_LEN];
    chacha20poly1305::aead::OsRng.fill_bytes(&mut salt);
    salt
}

/// 主密码 → 主密钥 MK（32B）。同口令 + 同 salt + 同参数 ⇒ 同 MK；
/// 参数低于下限直接拒绝（红线在代码层锁死）。
pub fn derive_master_key(
    password: &str,
    salt: &[u8],
    params: KdfParams,
) -> Result<Zeroizing<[u8; 32]>, KdfError> {
    if params.memory_kib < 8192 || params.iterations < 1 || params.parallelism < 1 {
        return Err(KdfError::ParamsTooWeak);
    }
    if salt.len() < 8 {
        return Err(KdfError::ParamsTooWeak);
    }
    let a2_params =
        Params::new(params.memory_kib, params.iterations, params.parallelism, Some(32))
            .map_err(|e| KdfError::Hash(e.to_string()))?;
    let engine = Argon2::new(Algorithm::Argon2id, Version::V0x13, a2_params);
    let mut out = Zeroizing::new([0u8; 32]);
    engine
        .hash_password_into(password.as_bytes(), salt, out.as_mut())
        .map_err(|e| KdfError::Hash(e.to_string()))?;
    Ok(out)
}

/// MK → 库密钥 VK：blake3 context 派生（域分离，VK 泄露不影响 AK 反之亦然）。
pub fn derive_vault_key(master_key: &[u8; 32]) -> Zeroizing<[u8; 32]> {
    Zeroizing::new(blake3::derive_key(CONTEXT_VAULT_KEY, master_key))
}

/// MK → 认证密钥 AK：零知识登录用（同步任务接入），此处一并实现保证派生链完整。
pub fn derive_auth_key(master_key: &[u8; 32]) -> Zeroizing<[u8; 32]> {
    Zeroizing::new(blake3::derive_key(CONTEXT_AUTH_KEY, master_key))
}

/// MK → 本地口令校验器（64 位 hex）：只证口令对错，不泄露任何密钥材料；
/// 防爆破强度由 Argon2id 同源承担（攻击者拿到 verifier 与拿到密文成本一致）。
pub fn derive_local_verifier(master_key: &[u8; 32]) -> String {
    let dk = blake3::derive_key(CONTEXT_LOCAL_VERIFIER, master_key);
    dk.iter().map(|b| format!("{b:02x}")).collect()
}

/// 解锁后的密码库会话：持有 VK，提供条目级信封加解密。
///
/// 生命周期即安全边界：`lock()` 或 drop 时 VK 清零；
/// 锁定后的内存里不再有任何可用密钥（红队「内存残留」项的应对）。
pub struct UnlockedVault {
    /// 库密钥（信封解包的根钥匙）。
    vault_key: Zeroizing<[u8; 32]>,
}

impl UnlockedVault {
    /// 解锁：主密码 + salt + KDF 参数 → MK → VK。
    pub fn unlock(password: &str, salt: &[u8], params: KdfParams) -> Result<Self, KdfError> {
        let mk = derive_master_key(password, salt, params)?;
        Ok(Self::from_master_key(mk))
    }

    /// 已持有 MK 时直接构造（校验器比对通过后的路径，避免二次 Argon2id）。
    pub fn from_master_key(master_key: Zeroizing<[u8; 32]>) -> Self {
        Self {
            vault_key: derive_vault_key(&*master_key),
        }
    }

    /// 加密一条条目正文（AAD 传条目 id，防跨条目挪用）。
    pub fn seal_item(&self, plaintext: &[u8], item_id: &[u8]) -> Result<Envelope, CryptoError> {
        seal_envelope(&*self.vault_key, plaintext, item_id)
    }

    /// 解密一条条目正文；认证失败即 Err。
    pub fn open_item(
        &self,
        envelope: &Envelope,
        item_id: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        open_envelope(&*self.vault_key, envelope, item_id)
    }

    /// 主动锁定：消耗自身并清零 VK（drop 兜底之外的第二道保险）。
    pub fn lock(self) {
        let mut this = self;
        this.vault_key.zeroize();
    }
}

impl Drop for UnlockedVault {
    /// drop 兜底清零：即使调用方忘了 lock() 也不留密钥。
    fn drop(&mut self) {
        self.vault_key.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 弱参数：单元测试加速用（8MiB / t=1），生产路径传 KdfParams::default()。
    fn weak_params() -> KdfParams {
        KdfParams {
            memory_kib: 8192,
            iterations: 1,
            parallelism: 1,
        }
    }

    /// 默认 KDF 参数不低于计划书下限（防「顺手调弱」回归）。
    #[test]
    fn kdf_defaults_meet_floor() {
        let p = KdfParams::default();
        assert!(p.memory_kib >= 65536);
        assert!(p.iterations >= 3);
        assert!(p.parallelism >= 1);
    }

    /// 低于下限的参数直接拒绝：安全红线在代码层锁死。
    #[test]
    fn kdf_rejects_too_weak_params() {
        let r = derive_master_key("pw", &[0u8; 16], KdfParams { memory_kib: 1024, iterations: 1, parallelism: 1 });
        assert!(matches!(r, Err(KdfError::ParamsTooWeak)));
        // salt 过短同样拒绝
        let r = derive_master_key("pw", &[0u8; 4], weak_params());
        assert!(matches!(r, Err(KdfError::ParamsTooWeak)));
    }

    /// 派生确定性：同口令 + 同 salt + 同参数 ⇒ 同 MK。
    #[test]
    fn master_key_deterministic() {
        let salt = [1u8; SALT_LEN];
        let a = derive_master_key("correct horse", &salt, weak_params()).unwrap();
        let b = derive_master_key("correct horse", &salt, weak_params()).unwrap();
        assert_eq!(*a, *b);
    }

    /// 换口令或换 salt ⇒ MK 不同（爆破者不能复用预计算表）。
    #[test]
    fn master_key_differs_on_password_or_salt() {
        let salt_a = [1u8; SALT_LEN];
        let salt_b = [2u8; SALT_LEN];
        let base = derive_master_key("pw", &salt_a, weak_params()).unwrap();
        let other_pw = derive_master_key("pw2", &salt_a, weak_params()).unwrap();
        let other_salt = derive_master_key("pw", &salt_b, weak_params()).unwrap();
        assert_ne!(*base, *other_pw);
        assert_ne!(*base, *other_salt);
    }

    /// 校验器确定性：同口令同 salt 同参数 ⇒ 同 verifier；口令变 ⇒ verifier 变。
    #[test]
    fn local_verifier_deterministic() {
        let a = derive_local_verifier(&*derive_master_key("pw", &[1u8; SALT_LEN], weak_params()).unwrap());
        let b = derive_local_verifier(&*derive_master_key("pw", &[1u8; SALT_LEN], weak_params()).unwrap());
        let c = derive_local_verifier(&*derive_master_key("pw2", &[1u8; SALT_LEN], weak_params()).unwrap());
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 64);
    }

    /// VK/AK 域分离：同 MK 派生出两把不同的钥匙，撞名即测试失败。
    #[test]
    fn vault_and_auth_keys_domain_separated() {
        let mk = derive_master_key("pw", &[1u8; SALT_LEN], weak_params()).unwrap();
        let vk = derive_vault_key(&*mk);
        let ak = derive_auth_key(&*mk);
        assert_ne!(*vk, *ak);
    }

    /// 端到端：解锁 → 加密条目 → 锁定 → 错误口令解锁 → 解密必败。
    /// 这是「错误主密码绝不泄出明文」的最小闭环验证。
    #[test]
    fn wrong_password_never_opens_item() {
        let salt = [9u8; SALT_LEN];
        let vault = UnlockedVault::unlock("right password", &salt, weak_params()).unwrap();
        let env = vault.seal_item(b"top secret", b"item-1").unwrap();
        vault.lock();

        let impostor = UnlockedVault::unlock("wrong password", &salt, weak_params()).unwrap();
        assert!(impostor.open_item(&env, b"item-1").is_err());
    }

    /// 正常路径：解锁 → 加密 → 同一 vault 解密还原（AAD 一致）。
    #[test]
    fn vault_roundtrip_item() {
        let vault = UnlockedVault::unlock("pw", &[3u8; SALT_LEN], weak_params()).unwrap();
        let env = vault.seal_item(b"payload", b"item-42").unwrap();
        assert_eq!(vault.open_item(&env, b"item-42").unwrap(), b"payload");
    }
}
