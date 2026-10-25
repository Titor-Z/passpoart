//! `vault-cli` 薄壳：命令实现与测试全在 lib（`vault_cli::run` 可测试入口），
//! 这里只做 argv 接线、stdout/stderr 分流与退出码。

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match vault_cli::run(&args) {
        Ok(text) => println!("{text}"),
        Err(e) => {
            eprintln!("vault-cli: {e}");
            std::process::exit(1);
        }
    }
}
