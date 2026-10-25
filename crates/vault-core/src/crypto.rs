//! 加解密：trait 契约 + XChaCha20-Poly1305 信封加密实现。
//!
//! 信封加密（Envelope Encryption）：每条条目一把随机 DataKey（DK）加密正文，
//! 库密钥（VK）只用来包裹 DK。收益：改主密码/换 VK 时只需重包裹各条目的 DK，
//! 不必重加密全部条目正文。
//!
//! 安全红线：
//! - nonce 由 CSPRNG（OsRng）每次现生成，**永不复用**；
//! - AEAD 认证失败立即 Err，绝不返回部分明文；
//! - AAD 绑定条目 id：密文挪到别的条目上直接解密失败。

use chacha20poly1305::aead::{Aead, AeadCore, OsRng, Payload};
use chacha20poly1305::{Key, KeyInit, XChaCha20Poly1305, XNonce};

/// AEAD 加解密契约：实现方必须保证「同明文每次加密产生不同密文」（nonce 不复用）。
pub trait VaultCipher {
    /// 加密出错类型（实现方自定，这里只要求可调试）。
    type Error: std::fmt::Debug;

    /// 加密一段明文，返回密文二元组（ciphertext 已含 AEAD 认证标签）。
    fn seal(&self, plaintext: &[u8], aad: &[u8]) -> Result<SealedBox, Self::Error>;

    /// 解密并校验认证标签；失败即拒绝（绝不返回「尽力解出」的内容）。
    fn open(&self, sealed: &SealedBox, aad: &[u8]) -> Result<Vec<u8>, Self::Error>;
}

/// 密文二元组：单层加密的落盘形态（ciphertext + nonce）。
#[derive(Clone, Debug)]
pub struct SealedBox {
    /// 密文本体（内含 Poly1305 认证标签）。
    pub ciphertext: Vec<u8>,
    /// 本次加密的一次性 nonce（XChaCha20 为 24 字节）。
    pub nonce: Vec<u8>,
}

/// 信封加密的落盘形态：四个字段随存储层文档一起序列化。
///
/// 四个字段缺一不可：正文与 DK 各有独立的 nonce（24B），
/// 密文均内含认证标签。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Envelope {
    /// DK 加密的条目正文（含标签）。
    pub ciphertext: Vec<u8>,
    /// 加密正文用的 nonce。
    pub nonce: Vec<u8>,
    /// VK 加密包裹的 DK（含标签）。
    pub wrapped_dk: Vec<u8>,
    /// 加密 DK 用的 nonce。
    pub wrap_nonce: Vec<u8>,
}

/// 加密错误：只区分「失败」，不区分原因——错误信息本身不能成为侧信道。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoError {
    /// AEAD 认证失败（密钥错误 / 密文被篡改 / AAD 不符）。
    AuthFailed,
    /// 输入形状非法（nonce 长度不对等，视为数据损坏）。
    Malformed,
}

impl std::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CryptoError::AuthFailed => write!(f, "认证失败：密钥错误或数据被篡改"),
            CryptoError::Malformed => write!(f, "密文结构非法"),
        }
    }
}

impl std::error::Error for CryptoError {}

/// 用一把 32 字节密钥做单层 XChaCha20-Poly1305 加密（信封的原子操作）。
fn seal_once(cipher: &XChaCha20Poly1305, plaintext: &[u8], aad: &[u8]) -> Result<SealedBox, CryptoError> {
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, Payload { msg: plaintext, aad })
        .map_err(|_| CryptoError::AuthFailed)?;
    Ok(SealedBox {
        ciphertext,
        nonce: nonce.as_slice().to_vec(),
    })
}

/// 单层解密：认证失败即 Err。
fn open_once(cipher: &XChaCha20Poly1305, sealed: &SealedBox, aad: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let nonce = XNonce::from_slice(&sealed.nonce);
    cipher
        .decrypt(nonce, Payload { msg: &sealed.ciphertext, aad })
        .map_err(|_| CryptoError::AuthFailed)
}

/// 信封加密：VK 包裹随机 DK，DK 加密正文。AAD 用条目 id（域绑定）。
pub fn seal_envelope(vault_key: &[u8; 32], plaintext: &[u8], aad: &[u8]) -> Result<Envelope, CryptoError> {
    // DK 与 VK 同级安全：直接作为 XChaCha 密钥使用
    let mut dk = ZeroizingKey::random();
    let dk_cipher = XChaCha20Poly1305::new(Key::from_slice(dk.as_ref()));
    let inner = seal_once(&dk_cipher, plaintext, aad)?;

    let vk_cipher = XChaCha20Poly1305::new(Key::from_slice(vault_key));
    let wrapped = seal_once(&vk_cipher, dk.as_ref(), aad)?;

    dk.zero_now();
    Ok(Envelope {
        ciphertext: inner.ciphertext,
        nonce: inner.nonce,
        wrapped_dk: wrapped.ciphertext,
        wrap_nonce: wrapped.nonce,
    })
}

/// 信封解密：先拆 VK 得 DK，再用 DK 解正文。任一步认证失败即 Err。
pub fn open_envelope(vault_key: &[u8; 32], envelope: &Envelope, aad: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if envelope.nonce.len() != 24 || envelope.wrap_nonce.len() != 24 {
        return Err(CryptoError::Malformed);
    }
    let vk_cipher = XChaCha20Poly1305::new(Key::from_slice(vault_key));
    let wrapped = SealedBox {
        ciphertext: envelope.wrapped_dk.clone(),
        nonce: envelope.wrap_nonce.clone(),
    };
    // DK 拆出即清零（用完立即释放，不等到函数返回）
    let mut dk = ZeroizingKey::from_vec(open_once(&vk_cipher, &wrapped, aad)?);
    if dk.as_ref().len() != 32 {
        return Err(CryptoError::Malformed);
    }
    let dk_cipher = XChaCha20Poly1305::new(dk.as_key());
    let inner = SealedBox {
        ciphertext: envelope.ciphertext.clone(),
        nonce: envelope.nonce.clone(),
    };
    let plaintext = open_once(&dk_cipher, &inner, aad);
    dk.zero_now();
    plaintext
}

/// 32 字节密钥的清零包装：比 Zeroizing<Box<[u8]>> 多一层「用完立即清」语义。
struct ZeroizingKey {
    key: Box<[u8; 32]>,
}

impl ZeroizingKey {
    /// CSPRNG 生成新密钥。
    fn random() -> Self {
        use chacha20poly1305::aead::rand_core::RngCore;
        let mut buf = Box::new([0u8; 32]);
        OsRng.fill_bytes(buf.as_mut());
        Self { key: buf }
    }

    /// 包装现成密钥字节（长度不符时截断到 32B——调用方已保证）。
    fn from_vec(bytes: Vec<u8>) -> Self {
        let mut buf = Box::new([0u8; 32]);
        let len = bytes.len().min(32);
        buf[..len].copy_from_slice(&bytes[..len]);
        Self { key: buf }
    }

    fn as_ref(&self) -> &[u8; 32] {
        &self.key
    }

    /// 引用形态给 XChaCha20Poly1305::new（Key = GenericArray<u8, 32>）。
    fn as_key(&self) -> &Key {
        Key::from_slice(&self.key[..])
    }

    /// 立即清零（不等 drop）。
    fn zero_now(&mut self) {
        self.key.as_mut().fill(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VK: [u8; 32] = [7u8; 32];

    /// 确定性测试实现：XOR「加密」——只验证 trait 契约形状（保留自脚手架阶段）。
    struct XorCipher;

    impl VaultCipher for XorCipher {
        type Error = &'static str;

        fn seal(&self, plaintext: &[u8], _aad: &[u8]) -> Result<SealedBox, Self::Error> {
            Ok(SealedBox {
                ciphertext: plaintext.iter().map(|b| !b).collect(),
                nonce: vec![plaintext.len() as u8],
            })
        }

        fn open(&self, sealed: &SealedBox, _aad: &[u8]) -> Result<Vec<u8>, Self::Error> {
            if sealed.nonce.len() != 1 {
                return Err("bad nonce");
            }
            Ok(sealed.ciphertext.iter().map(|b| !b).collect())
        }
    }

    /// trait 契约形状：seal → open 往返。
    #[test]
    fn cipher_roundtrip_shape() {
        let cipher = XorCipher;
        let sealed = cipher.seal(b"secret", b"aad").unwrap();
        assert_eq!(cipher.open(&sealed, b"aad").unwrap(), b"secret");
    }

    /// trait 契约形状：坏 nonce 拒绝。
    #[test]
    fn cipher_rejects_bad_nonce() {
        let cipher = XorCipher;
        let mut sealed = cipher.seal(b"secret", b"aad").unwrap();
        sealed.nonce = vec![];
        assert!(cipher.open(&sealed, b"aad").is_err());
    }

    /// 信封往返：seal → open 还原正文。
    #[test]
    fn envelope_roundtrip() {
        let env = seal_envelope(&VK, b"{\"title\":\"GitHub\"}", b"item-1").unwrap();
        let plain = open_envelope(&VK, &env, b"item-1").unwrap();
        assert_eq!(plain, b"{\"title\":\"GitHub\"}");
    }

    /// 错误的 VK 解密失败：认证必须拒绝，不能出脏数据。
    #[test]
    fn envelope_rejects_wrong_key() {
        let env = seal_envelope(&VK, b"secret", b"item-1").unwrap();
        let wrong_vk = [8u8; 32];
        assert_eq!(open_envelope(&wrong_vk, &env, b"item-1"), Err(CryptoError::AuthFailed));
    }

    /// 密文被篡改一个字节：AEAD 标签校验失败。
    #[test]
    fn envelope_rejects_tampered_ciphertext() {
        let mut env = seal_envelope(&VK, b"secret", b"item-1").unwrap();
        env.ciphertext[0] ^= 0x01;
        assert_eq!(open_envelope(&VK, &env, b"item-1"), Err(CryptoError::AuthFailed));
    }

    /// 包裹的 DK 被篡改：同样拒绝。
    #[test]
    fn envelope_rejects_tampered_wrapped_dk() {
        let mut env = seal_envelope(&VK, b"secret", b"item-1").unwrap();
        env.wrapped_dk[0] ^= 0x01;
        assert_eq!(open_envelope(&VK, &env, b"item-1"), Err(CryptoError::AuthFailed));
    }

    /// AAD 绑定条目 id：把条目 A 的信封挪到条目 B 上必须解密失败。
    #[test]
    fn envelope_rejects_aad_swap() {
        let env = seal_envelope(&VK, b"secret", b"item-A").unwrap();
        assert_eq!(open_envelope(&VK, &env, b"item-B"), Err(CryptoError::AuthFailed));
    }

    /// 同明文两次加密产生不同密文与 nonce（nonce 不复用的行为验证）。
    #[test]
    fn envelope_nonce_never_reused() {
        let a = seal_envelope(&VK, b"same plaintext", b"item-1").unwrap();
        let b = seal_envelope(&VK, b"same plaintext", b"item-1").unwrap();
        assert_ne!(a.nonce, b.nonce);
        assert_ne!(a.wrap_nonce, b.wrap_nonce);
        assert_ne!(a.ciphertext, b.ciphertext);
    }

    /// 每条目独立 DK：两次包裹的 wrapped_dk 必不相同。
    #[test]
    fn envelope_dk_per_item_independent() {
        let a = seal_envelope(&VK, b"x", b"item-1").unwrap();
        let b = seal_envelope(&VK, b"x", b"item-2").unwrap();
        assert_ne!(a.wrapped_dk, b.wrapped_dk);
    }

    /// nonce 长度非法：形状校验先行拒绝。
    #[test]
    fn envelope_rejects_malformed_nonce() {
        let mut env = seal_envelope(&VK, b"secret", b"item-1").unwrap();
        env.nonce = vec![1, 2, 3];
        assert_eq!(open_envelope(&VK, &env, b"item-1"), Err(CryptoError::Malformed));
    }
}
