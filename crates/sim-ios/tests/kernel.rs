//! 宿主上的真跑：起内核 → 真打一次 HTTP → 看 JSON → 再验错误路径。
//!
//! 为什么这些测试在 macOS 上就能跑：iOS 壳最容易坏的地方——数据目录、内嵌模块
//! 落盘、端口回退、C 字符串与缓冲区——**一件都不属于 UIKit**，全在这一层。
//! 所以不用等真机、不用 Xcode 就能验；真机上只剩"窗口、键盘、转屏"那部分。
//!
//! 为什么全挤在**一个** `#[test]` 里：内核的落脚点（`IRONMIND_DATA`）是进程级
//! 环境变量，两个测试并行起来会互相改对方的存档路径。宁可写成有编号的一串，
//! 也不要一个偶发失败的测试——那种失败最后总是被 `#[ignore]` 掉。

use std::ffi::{c_char, c_int, c_void};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};

const ERR_LEN: usize = 1024;

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sim-ios-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn cstring(s: &str) -> std::ffi::CString {
    std::ffi::CString::new(s).unwrap()
}

/// 读回调写进缓冲区的错误文本。
fn read_err(buf: &[c_char]) -> String {
    let n = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    let bytes: Vec<u8> = buf[..n].iter().map(|c| *c as u8).collect();
    String::from_utf8_lossy(&bytes).to_string()
}

/// 调一次 `ironmind_start`（模块传 `None` = 用内嵌的）。
fn start(data: &Path, module: Option<&str>, port: u16) -> (u16, String) {
    let d = cstring(&data.to_string_lossy());
    let m = module.map(cstring);
    let mut err = vec![0 as c_char; ERR_LEN];
    let p = sim_ios::ironmind_start(
        d.as_ptr(),
        m.as_ref().map(|s| s.as_ptr()).unwrap_or(std::ptr::null()),
        port,
        err.as_mut_ptr(),
        err.len(),
    );
    (p, read_err(&err))
}

/// 极简 HTTP GET（测试用；生产路径是 WKWebView）。
fn http_get(port: u16, path: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("连不上内核");
    s.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    out
}

#[test]
fn kernel_end_to_end() {
    // ① 正常启动：端口、内嵌模块、页面、状态、日志
    let dir = scratch("serve");
    let (port, err) = start(&dir, None, 18_731);
    assert!(port > 0, "内核没起来：{err}");

    let bundled = dir.join("modules/bundled.imod");
    assert!(bundled.is_file(), "内嵌模块没落盘：{}", bundled.display());
    let bytes = std::fs::read(&bundled).unwrap();
    assert_eq!(&bytes[..2], b"PK", "落盘的模块应当是一个 zip");

    let page = http_get(port, "/");
    assert!(page.contains("200 OK"), "{}", &page[..200.min(page.len())]);
    assert!(page.contains("钢心"), "首页正文没出来");

    let state = http_get(port, "/api/state");
    assert!(state.contains("200 OK"), "{}", &state[..200.min(state.len())]);
    // 前端一进来就要这两样：战役天数与省份表（地图画不出来就是白屏）
    for key in ["\"campaign_days\"", "\"provinces\""] {
        assert!(
            state.contains(key),
            "状态里没有 {key}：{}",
            &state[..240.min(state.len())]
        );
    }

    // 出问题时这是唯一能拿到的东西
    assert!(dir.join("logs/server.log").is_file(), "没有写运行日志");

    // ② 内嵌模块必须能被**真正的加载器**读进来（不是只看文件在不在）
    let m = sim_content::load_module(&bundled).expect("内嵌模块加载失败");
    assert!(!m.meta.id.trim().is_empty(), "模块 id 不该是空的");
    assert!(!m.meta.name.trim().is_empty(), "模块名不该是空的");

    // ③ 端口被占：退到下一个，不许起不来
    //    （前端按 origin 存 localStorage，端口一变"看过引导了"就丢了。）
    let (same_again, _) = start(&dir, None, port);
    assert!(same_again > 0, "第二个端口没退过去");
    assert_ne!(same_again, port, "端口必须换一个");

    // ④ 坏模块：给 0 + 一句能看懂的话，不许黑屏
    let (bad_port, bad_err) = start(&dir, Some("/nope/does-not-exist.imod"), 0);
    assert_eq!(bad_port, 0, "坏模块不该给出端口");
    assert!(!bad_err.is_empty(), "起不来却什么都不说，界面上就是一块黑屏");
    assert!(bad_err.contains("模块"), "错误信息要能看懂：{bad_err}");

    // ⑤ 没给数据目录：拒绝，不许 panic
    //    （C 接口上"少传一个参数"编译器查不出来，只能在这里钉住行为。）
    let mut null_err = vec![0 as c_char; ERR_LEN];
    let p = sim_ios::ironmind_start(
        std::ptr::null(),
        std::ptr::null(),
        0,
        null_err.as_mut_ptr(),
        null_err.len(),
    );
    assert_eq!(p, 0);
    assert!(read_err(&null_err).contains("数据目录"));

    let _ = std::fs::remove_dir_all(&dir);
}

/// 外壳装的传输必须真的接管请求。
///
/// iOS 上没有 `curl`，这条路不通就只剩本地规则 AI——游戏能玩，但"LLM 将领"
/// 这个特性等于没有，而且**不会有任何报错**。所以这条测试盯的是"接上了"。
#[test]
fn the_http_bridge_takes_over_from_curl() {
    struct Seen {
        url: String,
        key: String,
        hit: bool,
    }

    unsafe extern "C" fn fake(
        ctx: *mut c_void,
        url: *const c_char,
        key: *const c_char,
        _body: *const c_char,
        _timeout: c_int,
        out: *mut c_char,
        out_len: usize,
    ) -> c_int {
        let seen = &mut *(ctx as *mut Seen);
        seen.url = std::ffi::CStr::from_ptr(url).to_string_lossy().to_string();
        seen.key = std::ffi::CStr::from_ptr(key).to_string_lossy().to_string();
        seen.hit = true;
        let reply = b"{\"choices\":[{\"message\":{\"content\":\"ok\"}}]}";
        let n = reply.len().min(out_len.saturating_sub(1));
        std::ptr::copy_nonoverlapping(reply.as_ptr(), out as *mut u8, n);
        *out.add(n) = 0;
        200
    }

    let mut seen = Seen { url: String::new(), key: String::new(), hit: false };
    sim_ios::ironmind_set_http_post(Some(fake), &mut seen as *mut Seen as *mut c_void);
    assert!(sim_server::http::bridge_installed());

    let text =
        sim_server::http::post("https://api.example/v1/chat/completions", "sk-test", "{}", 9, "llm")
            .expect("假传输应该成功");
    sim_ios::ironmind_set_http_post(None, std::ptr::null_mut());

    assert!(text.contains("choices"));
    assert_eq!(seen.url, "https://api.example/v1/chat/completions");
    assert_eq!(seen.key, "sk-test", "密钥要原样交给外壳");
    assert!(seen.hit);
    assert!(!sim_server::http::bridge_installed(), "卸掉之后要回到 curl");
}
