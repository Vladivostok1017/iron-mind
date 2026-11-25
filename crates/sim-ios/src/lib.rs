//! iOS 外壳的 Rust 半边：**同一个内核 + 一层 C 接口**。
//!
//! 和安卓那层（`sim-mobile`）的区别只有一个，但很大：
//!
//! | | 安卓 | iOS（这里） |
//! |---|---|---|
//! | 窗口 / WebView | tao + wry，Rust 里写 | Swift + WKWebView，这个 crate **不碰 UI** |
//! | 为什么不一样 | 一套代码统一到底 | iOS 的键盘顶起、转屏重排、安全区、JS 的 `confirm()` 都要 App 自己接管 |
//!
//! 共同点：内核在**同一个进程**里跑（`sim_server::start`），页面指
//! `http://127.0.0.1:<port>/`。iOS 也禁止 App fork/exec 别的二进制，所以不能像
//! 桌面版那样起一个 `sim-server` 子进程。
//!
//! 这个 crate 只做四件事：
//!
//! 1. [`ironmind_start`]：定数据目录、把内嵌模块铺到沙盒、起内核、回一个端口；
//! 2. [`ironmind_set_http_post`]：把**出站** HTTPS 交给 Swift 的 URLSession
//!    （iOS 上没有 `curl`，内核默认那条路走不通，见 `sim-server/src/http.rs`）；
//! 3. [`ironmind_version`]：版本号，日志和"关于"里用；
//! 4. 失败时把原因写进调用方给的缓冲区，让 Swift 能显示在屏幕上——
//!    iOS 上没有控制台窗口，"起不来但什么都不说"等于让玩家看一块黑屏。
//!
//! 暴露出去的这几个函数**只认 C 类型**（`char*` / `uint16_t` / `size_t`），因为
//! 对面是 Swift。签名要和 `ios/IronMind/IronMind-Bridging-Header.h` 一字不差：
//! C 没有重载也没有类型检查，改错一个参数就会在运行期变成乱码或者直接崩。
//!
//! 这一层刻意**没有任何 iOS 专用 API**：数据目录和模块路径都由 Swift 传进来，
//! Rust 只负责写文件和起服务。好处是这些代码在 macOS 上就能跑、
//! 也就有了 `tests/kernel.rs` 里那几条"真起内核、真打 HTTP"的测试。

use std::ffi::{c_char, c_void, CStr};
use std::path::{Path, PathBuf};

/// 打进静态库的战役模块（`build.rs` 经 `sim-bundle` 打的 zip）。
const BUNDLED: &[u8] = include_bytes!(env!("SIM_BUNDLED_MODULE"));

/// 内核自己的版本号。C 字符串，**调用方不要 free**（它是静态数据）。
static VERSION_C: &str = concat!(env!("CARGO_PKG_VERSION"), "\0");

/// 版本号，例如 `"1.3.0"`。
#[no_mangle]
pub extern "C" fn ironmind_version() -> *const c_char {
    VERSION_C.as_ptr() as *const c_char
}

/// 起内核。成功返回监听的端口（> 0），失败返回 `0`。
///
/// 参数：
/// - `data_dir`：**必填**。app 沙盒里可写的目录（iOS 上是 `Documents`）。
///   存档 `saves/`、日志 `logs/`、模块 `modules/`、临时文件 `tmp/` 全在它下面。
///   内核只认 `IRONMIND_DATA` 这个环境变量，这里替外壳设好再调 `start`。
///   —— iOS 上进程的当前目录是 `/`（不可写），所以"用相对路径"这条路不存在。
/// - `module_path`：开局用哪个模块。传 `NULL` 或空串 = 用内嵌的那个（正常玩法）；
///   传路径 = 用那个模块（调试用：`Documents/modules/xxx.imod`）。
/// - `preferred_port`：想要的端口。固定端口不是洁癖——浏览器按
///   `http://127.0.0.1:<port>` 区分 origin，端口一变 `localStorage` 就是空的
///   （"看过引导了"这个标记会因此每次启动都丢）。被占了会自动退到
///   `preferred+1 / +2 / +3`，再不行让系统随便分一个，绝不因为端口冲突起不来。
/// - `err_buf` / `err_len`：失败原因写这儿，UTF-8、NUL 结尾、超长截断。可以传 `NULL`。
///
/// 可以重复调用（内核本身是可重入的），但每次都会起一份新世界——
/// Swift 那边用「已经起过就不再起」把住了，别指望这里去重。
#[no_mangle]
pub extern "C" fn ironmind_start(
    data_dir: *const c_char,
    module_path: *const c_char,
    preferred_port: u16,
    err_buf: *mut c_char,
    err_len: usize,
) -> u16 {
    let Some(data) = cstr_to_string(data_dir).filter(|s| !s.trim().is_empty()) else {
        write_buf(err_buf, err_len, "没有给出数据目录（data_dir 为空）");
        return 0;
    };
    let data = PathBuf::from(data.trim());

    let modules = data.join("modules");
    if let Err(e) = std::fs::create_dir_all(&modules) {
        write_buf(err_buf, err_len, &format!("建数据目录失败：{e}（{}）", data.display()));
        return 0;
    }

    // 内核的所有相对路径都归到数据目录。桌面版是 `.`（exe 旁边），
    // iOS 上没有"当前目录"这回事，必须显式指过去。
    std::env::set_var("IRONMIND_DATA", &data);
    std::env::set_var("IRONMIND_MODULES", &modules);

    let module = match cstr_to_string(module_path).filter(|s| !s.trim().is_empty()) {
        Some(p) => PathBuf::from(p.trim()),
        None => match install_bundled(&modules) {
            Ok(p) => p,
            Err(e) => {
                write_buf(err_buf, err_len, &format!("铺内嵌模块失败：{e}"));
                return 0;
            }
        },
    };
    println!("钢心 iOS · 内核 {} · 数据 {}", env!("CARGO_PKG_VERSION"), data.display());
    println!("模块：{}", module.display());

    let spec = module.to_string_lossy().to_string();
    let mut last_error = String::new();
    for port in candidate_ports(preferred_port) {
        match sim_server::start(&spec, port) {
            Ok(p) => {
                write_buf(err_buf, err_len, "");
                return p;
            }
            Err(e) => last_error = e,
        }
    }
    write_buf(err_buf, err_len, &last_error);
    eprintln!("内核起不来：{last_error}");
    0
}

/// 装一个**出站** HTTP 传输（`NULL` = 卸掉，退回 `curl`）。
///
/// 为什么需要：内核调大模型原本是 `Command::new("curl")`。iOS 上既没有
/// `/usr/bin/curl`，App 也被禁止 fork/exec——不接这一层，将领就只能退回本地
/// 规则判断（游戏照常能玩，但"LLM 将领"这个特性等于没有）。
///
/// 契约见 `sim_server::http::BridgeFn`。Swift 那边在启动时调一次，之后
/// 内核的 LLM 请求全部走 URLSession；桌面/安卓不调，行为一字不变。
#[no_mangle]
pub extern "C" fn ironmind_set_http_post(
    f: Option<sim_server::http::BridgeFn>,
    ctx: *mut c_void,
) {
    sim_server::http::set_bridge(f, ctx);
}

// ─────────────────────────── 内部 ───────────────────────────

/// 端口候选：先试想要的，再试它后面两个，最后让系统分。
fn candidate_ports(preferred: u16) -> Vec<u16> {
    if preferred == 0 {
        return vec![0];
    }
    let mut out = vec![preferred];
    for d in 1..=2u16 {
        if let Some(p) = preferred.checked_add(d) {
            out.push(p);
        }
    }
    out.push(0);
    out
}

/// 把内嵌模块铺到 `modules/bundled.imod`。
///
/// **每次都覆盖**：模块是内容，不是玩家数据（存档在 `saves/`，不受影响）。
/// 装一版新 App 就等于换一版内嵌战役，不会出现"更新了但玩的还是旧地图"。
fn install_bundled(modules: &Path) -> std::io::Result<PathBuf> {
    let target = modules.join("bundled.imod");
    std::fs::write(&target, BUNDLED)?;
    Ok(target)
}

/// 读一个 C 字符串。空指针 → `None`；不是合法 UTF-8 → `None`
/// （宁可当成"没给"，也不要拿一串乱码去拼路径）。
fn cstr_to_string(p: *const c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(p) }.to_str().ok().map(|s| s.to_string())
}

/// 往调用方的缓冲区写一段 UTF-8（含结尾 NUL，超长截断）。
///
/// 截断**按字符边界**退：中文错误信息被切在半个字符上，Swift 读出来就是一堆
/// `�`，等于白写。宁可少几个字，也要能读。
fn write_buf(buf: *mut c_char, cap: usize, s: &str) {
    if buf.is_null() || cap == 0 {
        return;
    }
    let bytes = s.as_bytes();
    let mut n = bytes.len().min(cap - 1);
    while n > 0 && !s.is_char_boundary(n) {
        n -= 1;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf as *mut u8, n);
        *buf.add(n) = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_a_c_string() {
        let v = cstr_to_string(ironmind_version()).unwrap();
        assert_eq!(v, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn candidate_ports_always_ends_with_system_assigned() {
        assert_eq!(candidate_ports(0), vec![0]);
        assert_eq!(candidate_ports(18731), vec![18731, 18732, 18733, 0]);
        // 贴边的端口不该 panic，也不能回绕到 0 之外的怪值
        assert_eq!(candidate_ports(65535), vec![65535, 0]);
    }

    #[test]
    fn write_buf_truncates_and_terminates() {
        let mut buf = vec![9 as c_char; 8];
        write_buf(buf.as_mut_ptr(), buf.len(), "一二三四五六七八");
        let n = buf.iter().position(|c| *c == 0).unwrap();
        assert!(n < 8, "必须留一个字节给结尾 NUL");
        let text_raw: Vec<u8> = buf[..n].iter().map(|c| *c as u8).collect();
        let text = std::str::from_utf8(&text_raw).expect("截断不能落在半个字符上");
        assert_eq!(text, "一二");
    }
}
