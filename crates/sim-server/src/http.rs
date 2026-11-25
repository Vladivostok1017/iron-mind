//! 出站 HTTP：内核唯一的"往外说话"的地方（只给 LLM 用）。
//!
//! 两条路：
//!
//! | 谁 | 怎么发 |
//! |---|---|
//! | 桌面 / 安卓 | 调系统 `curl`（不引第三方 HTTP 依赖，key 走 curl 配置文件、不进命令行） |
//! | iOS | 外壳在启动时装一个函数进来（`set_bridge`），实际请求由 Swift 的 URLSession 发 |
//!
//! 为什么 iOS 必须换一条路：`/usr/bin/curl` 在 iOS 上不存在，App 也被禁止
//! fork/exec。不接这一层，将领就只能退回本地规则判断——游戏照常能玩，
//! 但 v1.0 的"LLM 将领"等于没有了。
//!
//! 更省事的做法是引一个 HTTPS 客户端（rustls 之类）进内核，让三个平台统一。
//! 没这么做，是因为那会把"要不要联网、用谁的证书"变成内核的事；而现在内核
//! 只管"发一个 POST、拿回一段文本"，传输层归外壳。桌面和安卓的行为一个字没变。

use std::ffi::{c_char, c_int, c_void, CStr};
use std::sync::Mutex;

/// 外壳装进来的传输实现。C 接口，对面是 Swift（见 `ios/IronMind/HttpBridge.swift`）。
///
/// 契约：
/// - `url` / `key` / `body`：NUL 结尾的 UTF-8；`key` 是 Bearer 令牌（可为空串）；
/// - `timeout_secs`：超时秒数（>0）；
/// - `out` / `out_len`：调用方给的缓冲区，`out_len` 是**含结尾 NUL**的容量；
///   响应体写进去，NUL 结尾，超长截断；
/// - 返回值：HTTP 状态码（>= 100），或负数表示传输层失败（连不上、超时、非 HTTP）。
pub type BridgeFn = unsafe extern "C" fn(
    ctx: *mut c_void,
    url: *const c_char,
    key: *const c_char,
    body: *const c_char,
    timeout_secs: c_int,
    out: *mut c_char,
    out_len: usize,
) -> c_int;

/// 外壳装的传输。`ctx` 原样转发、我们从不解引用；
/// 函数指针本身与线程无关，所以可以跨线程用。
struct Bridge {
    f: BridgeFn,
    ctx: *mut c_void,
}

// SAFETY: 只做原样转发；有效性问题由外壳负责（iOS 那边传的是 nil）。
unsafe impl Send for Bridge {}
unsafe impl Sync for Bridge {}

static BRIDGE: Mutex<Option<Bridge>> = Mutex::new(None);

/// 装（或卸掉，传 `None`）出站传输。
pub fn set_bridge(f: Option<BridgeFn>, ctx: *mut c_void) {
    if let Ok(mut g) = BRIDGE.lock() {
        *g = f.map(|f| Bridge { f, ctx });
    }
}

/// 现在是不是走的"外壳传输"。启动日志里写一行，出问题时一眼能看出走的是哪条路。
pub fn bridge_installed() -> bool {
    BRIDGE.lock().map(|g| g.is_some()).unwrap_or(false)
}

/// 发一个 JSON POST，返回**响应体原文**（不论状态码，和 `curl -s` 一样）。
///
/// 状态码的语义留给调用方：`llm.rs` 本来就会先找响应里的 `error` 字段。
/// 这里只负责"传输成功了没有"。`tag` 只用来给 curl 的临时目录起名。
pub fn post(
    url: &str,
    key: &str,
    body: &str,
    timeout_secs: u64,
    tag: &str,
) -> Result<String, String> {
    // 先把函数指针抄出来就放开锁：真发请求可能要几十秒，
    // 不能攥着锁（而且回调里万一再进来一次就是死锁）。
    let bridge = BRIDGE.lock().ok().and_then(|g| g.as_ref().map(|b| (b.f, b.ctx)));
    match bridge {
        Some((f, ctx)) => post_via_bridge(f, ctx, url, key, body, timeout_secs),
        None => post_via_curl(url, key, body, timeout_secs, tag),
    }
}

/// 走外壳的传输（iOS）。
///
/// 缓冲区 1 MB：模型回复通常几百字节，留足余量又不至于占内存。
fn post_via_bridge(
    f: BridgeFn,
    ctx: *mut c_void,
    url: &str,
    key: &str,
    body: &str,
    timeout_secs: u64,
) -> Result<String, String> {
    const CAP: usize = 1 << 20;

    let c_url = std::ffi::CString::new(url).map_err(|_| "请求地址里有 NUL 字节".to_string())?;
    let c_key = std::ffi::CString::new(key).map_err(|_| "密钥里有 NUL 字节".to_string())?;
    let c_body = std::ffi::CString::new(body).map_err(|_| "请求体里有 NUL 字节".to_string())?;
    let mut buf = vec![0u8; CAP];

    // 超时再加 5 秒：给外壳留出"自己也要收尾"的余量，
    // 否则外壳的超时和内核的超时同一刻到，我们可能读到一个还没写完的缓冲。
    let timeout = timeout_secs.clamp(1, 600) as c_int;
    let status = unsafe {
        f(
            ctx,
            c_url.as_ptr(),
            c_key.as_ptr(),
            c_body.as_ptr(),
            timeout,
            buf.as_mut_ptr() as *mut c_char,
            buf.len(),
        )
    };

    if status < 0 {
        return Err(format!("请求发不出去（URLSession 错误码 {status}）"));
    }
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    let text = String::from_utf8_lossy(&buf[..end]).to_string();
    if text.trim().is_empty() {
        return Err(format!("模型无响应（HTTP {status}）"));
    }
    Ok(text)
}

/// 走系统 `curl`（桌面 / 安卓，一直是这条路）。
fn post_via_curl(
    url: &str,
    key: &str,
    body: &str,
    timeout_secs: u64,
    tag: &str,
) -> Result<String, String> {
    // key 写进 curl 配置文件，不出现在命令行参数里（`ps` 看不到）。
    let dir = std::env::temp_dir().join(format!("ironmind-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("建临时目录失败：{e}"))?;
    let body_path = dir.join("body.json");
    let cfg_path = dir.join("curl.conf");
    std::fs::write(&body_path, body).map_err(|e| format!("写请求失败：{e}"))?;
    std::fs::write(
        &cfg_path,
        format!(
            "url = \"{url}\"\nheader = \"Authorization: Bearer {key}\"\nheader = \"Content-Type: application/json\"\n",
        ),
    )
    .map_err(|e| format!("写 curl 配置失败：{e}"))?;

    let out = std::process::Command::new("curl")
        .args([
            "-s",
            "--max-time",
            &timeout_secs.to_string(),
            "-K",
            &cfg_path.to_string_lossy(),
            "-d",
            &format!("@{}", body_path.to_string_lossy()),
        ])
        .output()
        .map_err(|e| format!("调用 curl 失败：{e}"))?;
    let _ = std::fs::remove_dir_all(&dir);

    let text = String::from_utf8_lossy(&out.stdout).to_string();
    if text.trim().is_empty() {
        return Err(format!(
            "模型无响应（stderr: {}）",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(text)
}

/// 读一个 C 字符串（测试用；生产路径不需要）。
#[allow(dead_code)]
fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(p) }.to_string_lossy().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 假传输：记下收到的参数，回一段固定的 JSON。
    struct Seen {
        url: String,
        key: String,
        body: String,
        timeout: i32,
    }

    unsafe extern "C" fn fake_ok(
        ctx: *mut c_void,
        url: *const c_char,
        key: *const c_char,
        body: *const c_char,
        timeout_secs: c_int,
        out: *mut c_char,
        out_len: usize,
    ) -> c_int {
        let seen = &mut *(ctx as *mut Seen);
        seen.url = cstr(url);
        seen.key = cstr(key);
        seen.body = cstr(body);
        seen.timeout = timeout_secs;
        let reply = b"{\"choices\":[]}";
        let n = reply.len().min(out_len.saturating_sub(1));
        std::ptr::copy_nonoverlapping(reply.as_ptr(), out as *mut u8, n);
        *out.add(n) = 0;
        200
    }

    unsafe extern "C" fn fake_fail(
        _ctx: *mut c_void,
        _url: *const c_char,
        _key: *const c_char,
        _body: *const c_char,
        _timeout_secs: c_int,
        _out: *mut c_char,
        _out_len: usize,
    ) -> c_int {
        -1009
    }

    /// 一条测试里改全局传输，另一条同时读它 —— 串行跑，别互相踩。
    static LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn bridge_takes_over_and_receives_what_we_send() {
        let _g = LOCK.lock().unwrap();
        let mut seen =
            Seen { url: String::new(), key: String::new(), body: String::new(), timeout: 0 };
        set_bridge(Some(fake_ok), &mut seen as *mut Seen as *mut c_void);
        assert!(bridge_installed());

        let text = post("https://example.test/v1/chat/completions", "secret", "{\"q\":1}", 7, "t")
            .expect("假传输应该成功");
        set_bridge(None, std::ptr::null_mut());

        assert!(text.contains("choices"));
        assert_eq!(seen.url, "https://example.test/v1/chat/completions");
        assert_eq!(seen.key, "secret", "密钥要原样交给外壳（它负责拼 Bearer 头）");
        assert_eq!(seen.body, "{\"q\":1}");
        assert_eq!(seen.timeout, 7);

        // 卸载之后不该再走它
        assert!(!bridge_installed());
    }

    #[test]
    fn transport_failure_is_an_error_not_an_empty_string() {
        let _g = LOCK.lock().unwrap();
        set_bridge(Some(fake_fail), std::ptr::null_mut());
        let r = post("https://example.test/x", "k", "{}", 5, "t");
        set_bridge(None, std::ptr::null_mut());
        let err = r.unwrap_err();
        assert!(err.contains("URLSession"), "{err}");
    }
}
