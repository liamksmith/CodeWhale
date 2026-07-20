#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> std::process::ExitCode {
    // 将 SIGPIPE 重置为 SIG_DFL，这样当 codewhale 输出通过管道传入提前退出的命令时
    //（例如 `codewhale doctor | head`），进程会以退出码 141 正常终止，而不是因管道断裂写入而 panic。
    // 许多执行环境（systemd、Docker、某些 shell）继承 SIGPIPE 为 SIG_IGN，
    // 这会使 write(2) 返回 EPIPE；Rust 的 `println!` 随后将该 io::Error 视为致命错误并 panic。
    // 参见 issue #4030。
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    codewhale_cli::run_cli()
}
