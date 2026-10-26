//! 密码生成器：本地 OsRng 随机，永不上传。
//!
//! 规则：至少一小写、一大写、一数字、一符号（保证强度下限），其余在全集均匀取，
//! 最后 Fisher–Yates 洗牌（前四位保证位不暴露信息）。长度钳制在 8..=64。

use chacha20poly1305::aead::rand_core::RngCore;

/// 小写字母集。
const LOWER: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
/// 大写字母集。
const UPPER: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
/// 数字集。
const DIGIT: &[u8] = b"0123456789";
/// 符号集（避开易混淆与 shell 敏感字符的保守集合）。
const SYMBOL: &[u8] = b"!#%*+-=?@^_~";

/// 生成随机密码（len 钳制到 8..=64；每类字符至少一个）。
pub fn generate_password(len: usize) -> String {
    let len = len.clamp(8, 64);
    let mut rng = chacha20poly1305::aead::OsRng;
    let pools = [LOWER, UPPER, DIGIT, SYMBOL];
    let mut bytes: Vec<u8> = Vec::with_capacity(len);
    // 每池先取一个，保住强度下限
    for pool in pools {
        bytes.push(pool[rng.next_u32() as usize % pool.len()]);
    }
    // 其余在全集均匀取
    let total_len: usize = pools.iter().map(|p| p.len()).sum();
    let flat: Vec<u8> = pools.concat();
    while bytes.len() < len {
        bytes.push(flat[rng.next_u32() as usize % total_len]);
    }
    // Fisher–Yates 洗牌：保证位不泄露「前四位必含四类」
    for i in (1..bytes.len()).rev() {
        let j = rng.next_u32() as usize % (i + 1);
        bytes.swap(i, j);
    }
    // 取值都来自 ASCII 池，UTF-8 安全
    String::from_utf8(bytes).expect("密码字节全为 ASCII")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 长度钳制与四类字符下限。
    #[test]
    fn password_shape() {
        for len in [8usize, 16, 24, 64, 100] {
            let pw = generate_password(len);
            assert_eq!(pw.len(), len.clamp(8, 64));
            assert!(pw.chars().any(|c| c.is_ascii_lowercase()));
            assert!(pw.chars().any(|c| c.is_ascii_uppercase()));
            assert!(pw.chars().any(|c| c.is_ascii_digit()));
            assert!(pw.chars().any(|c| !c.is_ascii_alphanumeric()));
        }
    }

    /// 随机性冒烟：连发 8 个互不相同（固定种子的退化实现会在此暴露）。
    #[test]
    fn passwords_are_random() {
        let set: std::collections::HashSet<_> =
            (0..8).map(|_| generate_password(16)).collect();
        assert_eq!(set.len(), 8);
    }
}
