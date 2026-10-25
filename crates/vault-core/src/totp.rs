//! TOTP（RFC 6238）：动态验证码生成——仅解锁后的展示层使用。
//!
//! 算法：TOTP = HOTP(K, floor(unix / 30))，6 位数字、HMAC-SHA1，
//! 与主流验证器（Google Authenticator 等）默认参数一致。
//! 密钥来源：条目 `totp_secret` 的 base32（RFC 4648 无填充；
//! 容忍小写与空格/连字符分隔——导入导出场景常见）。

use hmac::{Hmac, Mac};
use sha1::Sha1;

type HmacSha1 = Hmac<Sha1>;

/// TOTP 周期（秒）。
pub const PERIOD: i64 = 30;
/// 数字位数。
pub const DIGITS: u32 = 6;

/// base32 解码（RFC 4648 无填充；忽略空格与连字符；非法字符返回 None）。
pub fn base32_decode(text: &str) -> Option<Vec<u8>> {
    let mut bits: u32 = 0;
    let mut nbits: u32 = 0;
    let mut out = Vec::new();
    for c in text.chars() {
        if c == ' ' || c == '-' {
            continue;
        }
        let up = c.to_ascii_uppercase();
        let value = match up {
            b @ 'A'..='Z' => b as u32 - 'A' as u32,
            b @ '2'..='7' => b as u32 - '2' as u32 + 26,
            _ => return None,
        };
        bits = (bits << 5) | value;
        nbits += 5;
        if nbits >= 8 {
            nbits -= 8;
            out.push(((bits >> nbits) & 0xFF) as u8);
        }
    }
    Some(out)
}

/// HOTP（RFC 4226）：8 字节大端计数器 → HMAC-SHA1 → 动态截断 → mod 10^digits。
fn hotp(key: &[u8], counter: u64, digits: u32) -> u32 {
    let mut mac = HmacSha1::new_from_slice(key).expect("HMAC 接受任意长度密钥");
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[19] & 0x0F) as usize;
    let bin = ((digest[offset] as u32 & 0x7F) << 24)
        | ((digest[offset + 1] as u32) << 16)
        | ((digest[offset + 2] as u32) << 8)
        | (digest[offset + 3] as u32);
    bin % 10u32.pow(digits)
}

/// 当前时刻的 6 位 TOTP 码；密钥非法或为空返回 None（UI 显示占位）。
pub fn totp_code(secret_base32: &str, unix_secs: i64) -> Option<String> {
    let key = base32_decode(secret_base32)?;
    if key.is_empty() {
        return None;
    }
    let counter = (unix_secs.max(0) / PERIOD) as u64;
    Some(format!("{:0width$}", hotp(&key, counter, DIGITS), width = DIGITS as usize))
}

/// 当前周期剩余秒数（1..=30；整周期刚开始时为 30）。
pub fn seconds_remaining(unix_secs: i64) -> u8 {
    (PERIOD - (unix_secs.max(0) % PERIOD)) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6238 附录 B 官方向量（SHA1，密钥 "12345678901234567890" 的 base32）。
    /// 8 位原向量取后 6 位对照（287082 / 081804 / 005924 / 279037）。
    const RFC_SECRET: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";

    #[test]
    fn rfc6238_vectors() {
        assert_eq!(totp_code(RFC_SECRET, 59).as_deref(), Some("287082"));
        assert_eq!(totp_code(RFC_SECRET, 1_111_111_109).as_deref(), Some("081804"));
        assert_eq!(totp_code(RFC_SECRET, 1_234_567_890).as_deref(), Some("005924"));
        assert_eq!(totp_code(RFC_SECRET, 2_000_000_000).as_deref(), Some("279037"));
    }

    /// 同一 30 秒周期内码不变，跨周期即变。
    #[test]
    fn code_stable_within_period() {
        let a = totp_code(RFC_SECRET, 990).unwrap(); // 周期 33：990..=1019
        let b = totp_code(RFC_SECRET, 1_019).unwrap();
        let c = totp_code(RFC_SECRET, 1_020).unwrap(); // 周期 34
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    /// base32：官方向量串解码回原文；容忍大小写/空格；非法字符拒绝。
    #[test]
    fn base32_decoding() {
        assert_eq!(
            base32_decode(RFC_SECRET).unwrap(),
            b"12345678901234567890".to_vec()
        );
        assert_eq!(
            base32_decode("gezd gnbv-gy3tqojq").unwrap(),
            b"1234567890".to_vec() // 16 字符 × 5bit = 10 字节
        );
        assert!(base32_decode("ABC1").is_none()); // 1 不在字母表
        assert!(base32_decode("").unwrap().is_empty());
        assert_eq!(totp_code("", 0), None); // 空密钥不出码
    }

    /// 倒计时：周期头剩 30 秒、尾剩 1 秒。
    #[test]
    fn remaining_seconds() {
        assert_eq!(seconds_remaining(30), 30);
        assert_eq!(seconds_remaining(59), 1);
        assert_eq!(seconds_remaining(60), 30);
        assert_eq!(seconds_remaining(0), 30);
    }
}
