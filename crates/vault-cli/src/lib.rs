//! `vault-cli` 库：命令实现全部在此（`run(&[String])` 可测试入口），
//! `main.rs` 只是薄壳。刻意零第三方依赖：红队自测要在最干净的依赖树下跑。

use std::io::Write;
use std::path::{Path, PathBuf};

use vault_core::keymgr::{derive_local_verifier, derive_master_key, generate_salt, KdfParams};
use vault_core::model::{Item, ItemKind};
use vault_core::storage::{
    open_item_from_storage, seal_item_for_storage, ItemStore, NodbStore, StoreError, VaultMeta,
};

/// CLI 错误：归一为可打印文本，main 据此设退出码。
#[derive(Debug)]
pub enum CliError {
    /// 用法错误（缺参数 / 未知命令）。
    Usage(String),
    /// 文件系统错误。
    Io(String),
    /// 解锁失败（口令错误或 KDF 异常）。
    Unlock(String),
    /// 存储层错误。
    Store(String),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::Usage(m) => write!(f, "用法错误：{m}"),
            CliError::Io(m) => write!(f, "文件错误：{m}"),
            CliError::Unlock(m) => write!(f, "解锁失败：{m}"),
            // StoreError 的 Display 自带「存储错误：」前缀，这里不再包一层避免重复
            CliError::Store(m) => write!(f, "{m}"),
        }
    }
}

/// 命令入口：解析参数并分发；返回要打印到 stdout 的文本。
pub fn run(args: &[String]) -> Result<String, CliError> {
    match args.first().map(String::as_str) {
        None | Some("help") => Ok(usage()),
        Some("--version") | Some("-V") => Ok(version()),
        Some("init") => cmd_init(&args[1..]),
        Some("verify") => cmd_verify(&args[1..]),
        Some("add") => cmd_add(&args[1..]),
        Some("list") => cmd_list(&args[1..]),
        Some("get") => cmd_get(&args[1..]),
        Some("remove") => cmd_remove(&args[1..]),
        Some("lock") => cmd_lock(&args[1..]),
        Some("redteam") => cmd_redteam(&args[1..]),
        Some(other) => Err(CliError::Usage(format!("未知命令 {other}，见 help"))),
    }
}

/// 版本信息：聚合 workspace 与 vault-core 版本。
fn version() -> String {
    format!(
        "passpoart vault-cli v{} (vault-core v{})",
        env!("CARGO_PKG_VERSION"),
        vault_core::VERSION
    )
}

/// 用法说明：子命令清单即本 CLI 的验收面。
fn usage() -> String {
    "passpoart vault-cli —— 本地密码库命令行（开发与红队自测）\n\
     \n\
     可用命令：\n\
       init   <path> --password P [--weak-kdf]   创建密码库\n\
       verify <path> --password P                校验主密码（不碰条目）\n\
       add    <path> --password P --title T [--kind login|note|card|identity]\n\
              [--user U] [--secret S] [--url U] [--note N] [--totp B32]\n\
       list   <path> --password P                解密并列出全部存活条目\n\
       get    <path> --password P --id ID        查看单条明文（ID 用全量或 list 前缀）\n\
       remove <path> --password P --id ID        软删除（墓碑，ID 用全量或前缀）\n\
       lock   <path>                             记审计（CLI 进程即会话）\n\
       redteam <path>                            弱口令探测 + 文件体检\n\
       --version / help\n\
     \n\
     口令来源优先级：--password > VAULT_PASSWORD 环境变量 > stdin（回显可见）\n\
     --weak-kdf 仅供测试与红队演练（8MiB/t=1），生产默认 64MiB/t=3"
        .to_string()
}

// ================================================================ 参数与口令

/// 解析 `--key value` 旗标对与位置参数。
fn parse_kv(args: &[String]) -> Result<(Vec<String>, Vec<(String, String)>), CliError> {
    let mut positional = Vec::new();
    let mut flags = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if let Some(name) = a.strip_prefix("--") {
            // 取值规则：下一参存在且不以 -- 开头才算「带值旗标」；
            // 否则视为无值开关（如 --weak-kdf），配空串占位。
            // 代价是 `--note --草稿` 这类以 -- 开头的值传不进来，CLI 场景可接受。
            match args.get(i + 1) {
                Some(value) if !value.starts_with("--") => {
                    flags.push((name.to_string(), value.clone()));
                    i += 2;
                }
                _ => {
                    flags.push((name.to_string(), String::new()));
                    i += 1;
                }
            }
        } else {
            positional.push(a.clone());
            i += 1;
        }
    }
    Ok((positional, flags))
}

/// 取旗标值。
fn flag<'a>(flags: &'a [(String, String)], name: &str) -> Option<&'a str> {
    flags
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

/// 口令三通道：--password > VAULT_PASSWORD > stdin（回显可见，帮助文本已注明）。
fn resolve_password(flags: &[(String, String)]) -> Result<String, CliError> {
    if let Some(p) = flag(flags, "password") {
        if !p.is_empty() {
            return Ok(p.to_string());
        }
    }
    if let Ok(p) = std::env::var("VAULT_PASSWORD") {
        if !p.is_empty() {
            return Ok(p);
        }
    }
    let mut line = String::new();
    print!("主密码：");
    let _ = std::io::stdout().flush();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| CliError::Io(e.to_string()))?;
    let trimmed = line.trim().to_string();
    if trimmed.is_empty() {
        return Err(CliError::Usage("口令为空".into()));
    }
    Ok(trimmed)
}

/// 打开库；文件不存在转成友好的用法错误。
fn open_store(path: &Path) -> Result<NodbStore, CliError> {
    NodbStore::open(path).map_err(|e| match e {
        StoreError::Db(m) if m.contains("不是有效的") || m.contains("文件不存在") => {
            CliError::Usage(format!("库文件不存在或已损坏：{}", path.display()))
        }
        other => CliError::Store(other.to_string()),
    })
}

/// 打开库并解锁：读 meta → 校验器比对 → 开库会话。
fn unlock_at(path: &Path, password: &str) -> Result<(NodbStore, UnlockedSession), CliError> {
    let store = open_store(path)?;
    let meta = store
        .read_vault_meta()
        .map_err(|e| CliError::Store(e.to_string()))?
        .ok_or_else(|| CliError::Usage("库未初始化（缺 vault_meta），请先 init".into()))?;
    let vault = meta.unlock(password).map_err(|e| match e {
        vault_core::storage::UnlockError::BadPassword => CliError::Unlock("主密码错误".into()),
        vault_core::storage::UnlockError::Kdf(e) => CliError::Unlock(e.to_string()),
    })?;
    Ok((store, UnlockedSession { vault, meta }))
}

/// 会话：解密上下文 + 库元数据快照。
struct UnlockedSession {
    vault: vault_core::keymgr::UnlockedVault,
    #[allow(dead_code)]
    meta: VaultMeta,
}

/// 条目类型旗标 → 枚举。
fn parse_kind(text: Option<&str>) -> Result<ItemKind, CliError> {
    match text {
        None => Ok(ItemKind::Login),
        Some("login") => Ok(ItemKind::Login),
        Some("note") => Ok(ItemKind::Note),
        Some("card") => Ok(ItemKind::Card),
        Some("identity") => Ok(ItemKind::Identity),
        Some(other) => Err(CliError::Usage(format!(
            "未知条目类型 {other}（可选 login/note/card/identity）"
        ))),
    }
}

/// 条目 id 解析：全量 id 精确命中（含墓碑）；否则按前缀在存活条目中唯一匹配。
/// list 只展示 8 位前缀，闭环要求人用前缀即可 get/remove；歧义必报错不猜。
fn resolve_item_id(store: &NodbStore, input: &str) -> Result<String, CliError> {
    if store
        .get(input)
        .map_err(|e| CliError::Store(e.to_string()))?
        .is_some()
    {
        return Ok(input.to_string());
    }
    let alive = store.list_alive().map_err(|e| CliError::Store(e.to_string()))?;
    let hits: Vec<&str> = alive
        .iter()
        .filter(|s| s.id.starts_with(input))
        .map(|s| s.id.as_str())
        .collect();
    match hits.as_slice() {
        [one] => Ok(one.to_string()),
        [] => Err(CliError::Usage(format!("条目不存在：{input}"))),
        many => Err(CliError::Usage(format!(
            "前缀 \"{input}\" 命中 {} 条存活条目，请补足字符",
            many.len()
        ))),
    }
}

// ================================================================ 命令

/// init：建库 + salt + KDF 参数 + 校验器落盘。
fn cmd_init(args: &[String]) -> Result<String, CliError> {
    let (positional, flags) = parse_kv(args)?;
    let path = positional
        .first()
        .ok_or_else(|| CliError::Usage("init 需要 <path>".into()))?;
    let password = resolve_password(&flags)?;
    let weak = flags.iter().any(|(k, _)| k == "weak-kdf");
    let params = if weak {
        KdfParams { memory_kib: 8192, iterations: 1, parallelism: 1 }
    } else {
        KdfParams::default()
    };
    let file = PathBuf::from(path);
    if file.exists() {
        return Err(CliError::Usage(format!("库文件已存在：{path}")));
    }

    let salt = generate_salt();
    let mk = derive_master_key(&password, &salt, params)
        .map_err(|e| CliError::Unlock(e.to_string()))?;
    let verifier = derive_local_verifier(&*mk);

    let store = NodbStore::open(&file).map_err(|e| CliError::Store(e.to_string()))?;
    store
        .init_vault_meta(&salt, params, &verifier)
        .map_err(|e| CliError::Store(e.to_string()))?;
    store.log_action("init", None).map_err(|e| CliError::Store(e.to_string()))?;

    Ok(format!(
        "已创建密码库 {path}\nKDF：Argon2id m={}KiB t={} p={}\n{}",
        params.memory_kib,
        params.iterations,
        params.parallelism,
        if weak { "（弱参数：仅供测试/红队演练）" } else { "" }
    ))
}

/// verify：只校验主密码（校验器比对），不碰任何条目密文。
fn cmd_verify(args: &[String]) -> Result<String, CliError> {
    let (positional, flags) = parse_kv(args)?;
    let path = positional
        .first()
        .ok_or_else(|| CliError::Usage("verify 需要 <path>".into()))?;
    let password = resolve_password(&flags)?;
    let store = open_store(Path::new(path))?;
    let meta = store
        .read_vault_meta()
        .map_err(|e| CliError::Store(e.to_string()))?
        .ok_or_else(|| CliError::Usage("库未初始化，请先 init".into()))?;
    match meta.unlock(&password) {
        Ok(_) => Ok("主密码校验通过".into()),
        Err(_) => Err(CliError::Unlock("主密码错误".into())),
    }
}

/// add：解锁 → 信封加密 → 落盘。
fn cmd_add(args: &[String]) -> Result<String, CliError> {
    let (positional, flags) = parse_kv(args)?;
    let path = positional
        .first()
        .ok_or_else(|| CliError::Usage("add 需要 <path>".into()))?;
    let password = resolve_password(&flags)?;
    let title = flag(&flags, "title")
        .ok_or_else(|| CliError::Usage("add 需要 --title".into()))?
        .to_string();

    let (mut store, session) = unlock_at(Path::new(path), &password)?;
    let mut item = Item::new(parse_kind(flag(&flags, "kind"))?, title);
    item.subtitle = flag(&flags, "user").unwrap_or("").to_string();
    item.secret = flag(&flags, "secret").unwrap_or("").to_string();
    item.url = flag(&flags, "url").unwrap_or("").to_string();
    item.note = flag(&flags, "note").unwrap_or("").to_string();
    item.totp_secret = flag(&flags, "totp").map(str::to_string);

    let stored = seal_item_for_storage(&session.vault, &item).map_err(|e| CliError::Store(e.to_string()))?;
    store.put(&stored).map_err(|e| CliError::Store(e.to_string()))?;
    store.log_action("put", Some(&stored.id)).map_err(|e| CliError::Store(e.to_string()))?;
    Ok(format!("已保存条目 {}（版本 {}，类型 {}）", stored.id, stored.version, item.kind.label()))
}

/// list：解锁 → 批量解密 → 表格输出。
fn cmd_list(args: &[String]) -> Result<String, CliError> {
    let (positional, flags) = parse_kv(args)?;
    let path = positional
        .first()
        .ok_or_else(|| CliError::Usage("list 需要 <path>".into()))?;
    let password = resolve_password(&flags)?;
    let (store, session) = unlock_at(Path::new(path), &password)?;

    let stored_items = store.list_alive().map_err(|e| CliError::Store(e.to_string()))?;
    let mut lines = vec![format!("共 {} 条", stored_items.len())];
    for it in &stored_items {
        let item = open_item_from_storage(&session.vault, it)
            .map_err(|e| CliError::Store(e.to_string()))?;
        let star = if item.favorite { "★" } else { " " };
        lines.push(format!(
            "  {}… [{}] v{} {} {} — {}",
            &it.id[..8],
            item.kind.label(),
            it.version,
            star,
            item.title,
            item.subtitle
        ));
    }
    Ok(lines.join("\n"))
}

/// get：单条完整明文（红队「解密必败/必成」的对照工具）。
fn cmd_get(args: &[String]) -> Result<String, CliError> {
    let (positional, flags) = parse_kv(args)?;
    let path = positional
        .first()
        .ok_or_else(|| CliError::Usage("get 需要 <path>".into()))?;
    let password = resolve_password(&flags)?;
    let id_input = flag(&flags, "id")
        .ok_or_else(|| CliError::Usage("get 需要 --id".into()))?;
    let (store, session) = unlock_at(Path::new(path), &password)?;
    let id = resolve_item_id(&store, id_input)?;

    let stored = store
        .get(&id)
        .map_err(|e| CliError::Store(e.to_string()))?
        .ok_or_else(|| CliError::Usage(format!("条目不存在：{id}")))?;
    let item = open_item_from_storage(&session.vault, &stored)
        .map_err(|e| CliError::Store(e.to_string()))?;
    Ok(format!(
        "id:      {}\n类型:    {}\n名称:    {}\n账号:    {}\n网址:    {}\n密文正文: {}\nTOTP:    {}\n备注:    {}\n版本:    v{}（更新于 {}）",
        item.id,
        item.kind.label(),
        item.title,
        item.subtitle,
        item.url,
        item.secret,
        item.totp_secret.as_deref().unwrap_or("—"),
        if item.note.is_empty() { "—" } else { &item.note },
        stored.version,
        stored.updated_at,
    ))
}

/// remove：软删除（墓碑），同步双端确认后才会物理清理。
fn cmd_remove(args: &[String]) -> Result<String, CliError> {
    let (positional, flags) = parse_kv(args)?;
    let path = positional
        .first()
        .ok_or_else(|| CliError::Usage("remove 需要 <path>".into()))?;
    let password = resolve_password(&flags)?;
    let id_input = flag(&flags, "id")
        .ok_or_else(|| CliError::Usage("remove 需要 --id".into()))?;
    let (mut store, _session) = unlock_at(Path::new(path), &password)?;
    let id = resolve_item_id(&store, id_input)?;
    store
        .tombstone(&id, vault_core::storage::now_secs())
        .map_err(|e| CliError::Store(e.to_string()))?;
    store.log_action("tombstone", Some(&id)).map_err(|e| CliError::Store(e.to_string()))?;
    Ok(format!("已打墓碑：{id}（list 不再显示，get 仍可取，物理清理待同步确认）"))
}

/// lock：CLI 进程即会话（命令结束内存密钥即 drop 清零），此处补一条审计。
fn cmd_lock(args: &[String]) -> Result<String, CliError> {
    let (positional, _) = parse_kv(args)?;
    let path = positional
        .first()
        .ok_or_else(|| CliError::Usage("lock 需要 <path>".into()))?;
    let store = open_store(Path::new(path))?;
    store.log_action("lock", None).map_err(|e| CliError::Store(e.to_string()))?;
    Ok("已记录锁定审计（CLI 每条命令都是独立会话，进程退出即锁定）".into())
}

/// redteam：攻击者视角体检——常见弱口令逐一探测 + 库文件体检。
fn cmd_redteam(args: &[String]) -> Result<String, CliError> {
    let (positional, _) = parse_kv(args)?;
    let path = positional
        .first()
        .ok_or_else(|| CliError::Usage("redteam 需要 <path>".into()))?;
    let store = open_store(Path::new(path))?;
    let meta = store
        .read_vault_meta()
        .map_err(|e| CliError::Store(e.to_string()))?
        .ok_or_else(|| CliError::Usage("库未初始化，请先 init".into()))?;

    let mut lines = vec![format!("红队体检：{}", path)];
    lines.push(format!(
        "KDF：Argon2id m={}KiB t={} p={} {}",
        meta.kdf_params.memory_kib,
        meta.kdf_params.iterations,
        meta.kdf_params.parallelism,
        if meta.kdf_params.memory_kib < 65536 { "（弱参数：仅供测试）" } else { "" }
    ));

    // 弱口令探测：全部必须拒绝；命中即报弱口令（探测成功 = 口令本身就是它）。
    let probes = ["", "password", "123456", "12345678", "qwerty", "letmein"];
    let mut weak_hits = Vec::new();
    for probe in probes {
        let verdict = match meta.unlock(probe) {
            Ok(_) => {
                weak_hits.push(probe);
                "[命中] 弱口令！"
            }
            Err(_) => "[拒绝]",
        };
        lines.push(format!("  尝试口令 \"{}\"（{} 字节）→ {}", probe, probe.len(), verdict));
    }
    lines.push(format!(
        "文件大小：{} 字节（单文件 .nodb，拷贝即备份；密文正文已由单元测试做字节级明文扫描）",
        std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
    ));
    lines.push(if weak_hits.is_empty() {
        "结论：常见弱口令全部拒绝 ✓".into()
    } else {
        "结论：存在弱口令，请立即更换主密码！".into()
    });
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 唯一临时 .nodb 路径。
    fn temp_nodb(name: &str) -> String {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        std::env::temp_dir()
            .join(format!("vaultcli_{}_{}_{}.nodb", name, std::process::id(), n))
            .display()
            .to_string()
    }

    /// 组装 run 参数。
    fn v(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    /// 从 list 输出截取条目 id 的 8 位前缀（get/remove 支持前缀，闭环不要求全量 id）。
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

    /// 用法文本必须覆盖全部计划子命令（计划书 Phase 0 命令清单）。
    #[test]
    fn usage_lists_all_subcommands() {
        let text = usage();
        for cmd in ["init", "verify", "add", "list", "get", "remove", "lock", "redteam"] {
            assert!(text.contains(cmd), "usage 缺少 {cmd}");
        }
    }

    /// kind 解析：合法值与非法值。
    #[test]
    fn kind_parsing() {
        assert_eq!(parse_kind(None).unwrap(), ItemKind::Login);
        assert_eq!(parse_kind(Some("card")).unwrap(), ItemKind::Card);
        assert!(parse_kind(Some("wallet")).is_err());
    }

    /// 端到端闭环：init → add×2 → list → get（前缀 id）→ verify 正/误 → remove（前缀）→ 墓碑语义。
    #[test]
    fn cli_full_flow() {
        let path = temp_nodb("flow");
        let pw = "correct horse battery";

        // init：弱 KDF 开关必须真实生效（输出回显参数，防「开关被解析器吞掉」回归）
        let out = run(&v(&["init", &path, "--password", pw, "--weak-kdf"])).unwrap();
        assert!(out.contains("已创建密码库"), "{out}");
        assert!(out.contains("8192"), "弱 KDF 开关未生效：{out}");

        // add 两条
        run(&v(&["add", &path, "--password", pw, "--title", "GitHub",
                 "--user", "titor@dev.io", "--secret", "s3cret-pw",
                 "--url", "github.com", "--kind", "login"])).unwrap();
        run(&v(&["add", &path, "--password", pw, "--title", "开发令牌",
                 "--kind", "note", "--secret", "tok-xyz"])).unwrap();

        // list：两条都在
        let listing = run(&v(&["list", &path, "--password", pw])).unwrap();
        assert!(listing.contains("共 2 条") && listing.contains("GitHub") && listing.contains("开发令牌"));

        // 空前缀命中全部存活条目 → 必须报歧义，不许静默挑一条
        let err = run(&v(&["get", &path, "--password", pw, "--id", ""])).unwrap_err();
        assert!(err.to_string().contains("命中 2 条"), "{err}");

        // get：list 只给 8 位前缀，前缀必须可用；并从输出解析全量 id
        let prefix = id_prefix(&listing, "GitHub");
        let got = run(&v(&["get", &path, "--password", pw, "--id", &prefix])).unwrap();
        assert!(got.contains("s3cret-pw") && got.contains("titor@dev.io"), "{got}");
        let full_id = got
            .lines()
            .find(|l| l.starts_with("id:"))
            .unwrap()
            .trim_start_matches("id:")
            .trim()
            .to_string();
        assert_eq!(full_id.len(), 36, "应为标准 UUID：{full_id}");
        run(&v(&["get", &path, "--password", pw, "--id", &full_id])).unwrap();

        // verify：正确通过 / 错误拒绝
        assert!(run(&v(&["verify", &path, "--password", pw])).is_ok());
        assert!(run(&v(&["verify", &path, "--password", "wrong"])).is_err());

        // remove（前缀）：墓碑后 list 消失、全量 id 仍可 get（同步裁决需要密文保留）
        run(&v(&["remove", &path, "--password", pw, "--id", &prefix])).unwrap();
        let listing = run(&v(&["list", &path, "--password", pw])).unwrap();
        assert!(listing.contains("共 1 条") && !listing.contains("GitHub"));
        let got = run(&v(&["get", &path, "--password", pw, "--id", &full_id])).unwrap();
        assert!(got.contains("s3cret-pw"));

        let _ = std::fs::remove_file(&path);
    }

    /// 弱口令探测：把库口令设成弱口令时 redteam 必须命中并告警。
    #[test]
    fn redteam_detects_weak_password() {
        let path = temp_nodb("weak");
        run(&v(&["init", &path, "--password", "123456", "--weak-kdf"])).unwrap();
        let out = run(&v(&["redteam", &path])).unwrap();
        assert!(out.contains("弱口令"), "{out}");
        assert!(out.contains("请立即更换主密码"));
        let _ = std::fs::remove_file(&path);
    }

    /// init 重复建库必须拒绝。
    #[test]
    fn init_rejects_existing_vault() {
        let path = temp_nodb("dup");
        run(&v(&["init", &path, "--password", "pw", "--weak-kdf"])).unwrap();
        assert!(run(&v(&["init", &path, "--password", "pw", "--weak-kdf"])).is_err());
        let _ = std::fs::remove_file(&path);
    }

    /// init 应自动补齐缺失的父目录（~/.passpoart 首次使用的真实场景；redb 不建目录）。
    #[test]
    fn init_creates_missing_parent_dirs() {
        let flat = std::path::PathBuf::from(temp_nodb("missing"));
        let nested = flat
            .parent()
            .unwrap()
            .join("a/b/c")
            .join(flat.file_name().unwrap());
        let path = nested.display().to_string();
        run(&v(&["init", &path, "--password", "pw", "--weak-kdf"])).unwrap();
        assert!(nested.exists(), "库文件应已创建：{path}");
        let _ = std::fs::remove_file(&nested);
    }
}
