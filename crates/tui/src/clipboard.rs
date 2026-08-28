//! 系统剪贴板写入（fd 重定向 + panic 兜底）。

/// 复制文本到系统剪贴板。
/// 优先 arboard（原生 X11/Wayland），fd 1/2 重定向到 /dev/null 防止污染 TUI；
/// 失败则回退命令行工具（stdout/stderr 已 null）。
/// 全程 catch_unwind：内部任何 panic 转为 Err 并记日志，绝不杀死 TUI。
pub(crate) fn copy_to_clipboard(text: &str) -> Result<(), String> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // arboard 优先：重定向 fd 1/2 → /dev/null，防止任何输出污染 TUI
        // 安全性：此函数在 spawn_blocking 线程执行且被 await，主线程不会 draw
        with_silent_stdout(|| {
            let mut cb = arboard::Clipboard::new().map_err(|e| format!("arboard init: {e}"))?;
            cb.set_text(text.to_owned())
                .map_err(|e| format!("arboard set: {e}"))
        })
    }));

    let arboard_result = match result {
        Ok(r) => r,
        Err(panic_payload) => {
            let msg = panic_payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic_payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".into());
            tracing::error!("剪贴板 arboard panic: {msg}");
            return Err(format!("剪贴板内部错误: {msg}"));
        }
    };
    if arboard_result.is_ok() {
        return Ok(());
    }

    // 回退：命令行工具（stdout/stderr → /dev/null）
    use std::io::Write;
    use std::process::{Command, Stdio};
    let candidates: [(&str, Vec<&str>); 4] = [
        ("xclip", vec!["-selection", "clipboard"]),
        ("wl-copy", vec![]),
        ("pbcopy", vec![]),
        ("clip", vec![]),
    ];
    for (cmd, args) in &candidates {
        let r = Command::new(cmd)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .and_then(|mut child| {
                if let Some(mut stdin) = child.stdin.take() {
                    stdin.write_all(text.as_bytes())?;
                }
                child.wait()
            });
        match r {
            Ok(_) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("启动 {cmd} 失败: {e}")),
        }
    }
    Err("无可用剪贴板（arboard 初始化失败且未安装 xclip/wl-copy）".into())
}

/// 在 fd 1/2 重定向到 /dev/null 的环境下执行闭包。
/// 异常安全：闭包 panic 时也保证 fd 恢复（否则后续 draw 永远输出到 /dev/null），
/// panic 再向外传播由上层 catch_unwind 兜底。
fn with_silent_stdout<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    unsafe {
        let saved_out = libc::dup(1);
        let saved_err = libc::dup(2);
        let devnull = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY);
        if devnull >= 0 {
            libc::dup2(devnull, 1);
            libc::dup2(devnull, 2);
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        if saved_out >= 0 {
            libc::dup2(saved_out, 1);
            libc::close(saved_out);
        }
        if saved_err >= 0 {
            libc::dup2(saved_err, 2);
            libc::close(saved_err);
        }
        if devnull >= 0 {
            libc::close(devnull);
        }
        match result {
            Ok(r) => r,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }
}
