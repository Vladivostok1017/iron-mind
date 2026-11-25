//! 安卓外壳。
//!
//! 和桌面外壳（`sim-desktop`）的分工一样，但有一处根本不同：
//!
//! | | 桌面 | 安卓 |
//! |---|---|---|
//! | 内核怎么跑 | 起一个 `sim-server.exe` **子进程** | **同一个进程**里调 `sim_server::start` |
//! | 为什么要不一样 | 出了事能单独重启内核 | Android 10+ 禁止 exec app 私有目录里的二进制 |
//! | 页面从哪来 | `http://127.0.0.1:<port>/` | 同上（页面本身编在 `.so` 里） |
//!
//! 这一层只做四件事：定数据目录、把内嵌的战役模块铺到盘上、起内核、开窗口。

#[cfg(not(target_os = "android"))]
mod stub {
    //! 桌面平台上这个 crate 是空的：它只服务于安卓。
    //!
    //! 保留它是因为 workspace 里的 `cargo build` 会编译所有成员——
    //! 在 Windows 上不该因为"这不是安卓"而报错。
}

#[cfg(target_os = "android")]
mod android;
