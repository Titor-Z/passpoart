//! 红队集成测试（计划书 §13 Day 3 验收）：黑盒只经 `vault_cli::run`，
//! 完整走 init → add → 文件字节扫描（无明文）→ 错误口令全拒 →
//! 正确口令可解 → remove 墓碑，最后 redteam 体检收口。
//! 单元测试之外的独立二进制：验证「真进程、真文件、真口令」下的闭环。

/// 唯一临时 .nodb 路径。
fn temp_nodb(name: &str) -> String {
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    std::env::temp_dir()
        .join(format!(
            "vaultcli_redteam_{}_{}_{}.nodb",
            name,
            std::process::id(),
            n
        ))
        .display()
        .to_string()
}

/// 组装 run 参数。
fn v(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

/// 执行必须成功的命令。
fn ok(parts: &[&str]) -> String {
    vault_cli::run(&v(parts)).unwrap_or_else(|e| panic!("命令应成功却失败：{e}"))
}

/// 从 list 输出截取条目 id 的 8 位前缀。
fn id_prefix(listing: &str, title: &str) -> String {
    listing
        .lines()
        .find(|l| l.contains(title))
        .unwrap_or_else(|| panic!("list 应含条目 {title}"))
        .trim()
        .chars()
        .take(8)
        .collect()
}

/// 主闭环：攻击者拷走 .nodb 文件后一无所获；持正确口令者全量可解。
#[test]
fn redteam_full_closed_loop() {
    let path = temp_nodb("loop");
    let pw = "correct horse battery staple";

    // 1. 建库（弱 KDF：测试环境加速；参数体检由 redteam 命令自报）
    let out = ok(&["init", &path, "--password", pw, "--weak-kdf"]);
    assert!(out.contains("已创建密码库"), "{out}");

    // 2. 两条高辨识度明文条目（登录 + 银行卡）
    ok(&["add", &path, "--password", pw, "--title", "GitHub",
         "--user", "titor@dev.io", "--secret", "REDTEAM-LOGIN-SECRET",
         "--url", "github.com"]);
    ok(&["add", &path, "--password", pw, "--title", "工资卡",
         "--kind", "card", "--secret", "REDTEAM-CARD-6222"]);

    // 3. 攻击者视角：拷走文件、字节级扫描——标题/账号/密文正文/口令都不得出现
    let bytes = std::fs::read(&path).unwrap();
    for marker in [
        "GitHub",
        "titor@dev.io",
        "REDTEAM-LOGIN-SECRET",
        "工资卡",
        "REDTEAM-CARD-6222",
        "correct horse",
    ] {
        assert!(
            !bytes.windows(marker.len()).any(|w| w == marker.as_bytes()),
            "明文 {marker} 泄漏进 .nodb 文件"
        );
    }

    // 4. 错误口令（含常见弱口令）全线拒绝：verify / list / get / add
    for wrong in ["wrong-password", "password", "123456"] {
        assert!(vault_cli::run(&v(&["verify", &path, "--password", wrong])).is_err());
        assert!(vault_cli::run(&v(&["list", &path, "--password", wrong])).is_err());
        assert!(
            vault_cli::run(&v(&["get", &path, "--id", "00000000", "--password", wrong])).is_err()
        );
        assert!(
            vault_cli::run(&v(&["add", &path, "--password", wrong, "--title", "x"])).is_err()
        );
    }

    // 5. 正确口令：list 全量 + get 前缀 id 取回完整明文（人肉闭环）
    let listing = ok(&["list", &path, "--password", pw]);
    assert!(
        listing.contains("共 2 条") && listing.contains("GitHub") && listing.contains("工资卡"),
        "{listing}"
    );
    let prefix = id_prefix(&listing, "GitHub");
    let got = ok(&["get", &path, "--password", pw, "--id", &prefix]);
    assert!(
        got.contains("REDTEAM-LOGIN-SECRET") && got.contains("titor@dev.io"),
        "{got}"
    );

    // 6. remove 墓碑：list 消失、全量 id 仍可 get（密文保留待同步裁决）
    let full_id = got
        .lines()
        .find(|l| l.starts_with("id:"))
        .unwrap()
        .trim_start_matches("id:")
        .trim()
        .to_string();
    ok(&["remove", &path, "--password", pw, "--id", &prefix]);
    let listing = ok(&["list", &path, "--password", pw]);
    assert!(listing.contains("共 1 条") && !listing.contains("GitHub"), "{listing}");
    let got = ok(&["get", &path, "--password", pw, "--id", &full_id]);
    assert!(got.contains("REDTEAM-LOGIN-SECRET"));

    // 7. redteam 体检：主口令不弱 → 常见口令全拒、结论通过
    let out = ok(&["redteam", &path]);
    assert!(out.contains("全部拒绝"), "{out}");

    // 8. lock：审计可记（CLI 进程即会话）
    ok(&["lock", &path]);

    let _ = std::fs::remove_file(&path);
}
