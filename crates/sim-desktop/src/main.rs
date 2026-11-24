//! 钢心 / Iron Mind —— 桌面外壳。
//!
//! 为什么要这一层：引擎本身是"本地 HTTP 服务 + 一个页面"。这在浏览器里能跑，
//! 但不像一个能装、能双击、有窗口的游戏。这一步只做三件事：
//!
//! 1. 起一个**原生窗口**（WebView2），里面就是那个页面；
//! 2. 把 `sim-server` 作为子进程拉起来，窗口关掉它跟着死；
//! 3. 端口随机挑空闲的，不跟已经在跑的开发服务器抢 8899。
//!
//! 它不含任何战役逻辑，也不改引擎——把 shell 和引擎分开，模块仓库依旧只认
//! `IRONMIND_MODULES`。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::window::{Icon, WindowBuilder};
use wry::WebViewBuilder;

const TITLE: &str = "钢心 · Iron Mind";
/// 窗口图标：tools/make-icon.ps1 生成的 32x32 raw RGBA。
/// 用 include_bytes! 而不是运行时读文件——装好之后少一个能丢的文件。
const ICON_RGBA: &[u8] = include_bytes!("../assets/icon32.rgba");

fn main() {
    // 服务在哪：装好的布局是 exe 旁边的 sim-server.exe；
    // 开发时是同一个 target 目录，所以两种写法都能命中。
    let Some(server) = find_server() else {
        fatal("找不到 sim-server（应与本程序放在同一目录）。");
        return;
    };
    // 端口：不写死 8899。开发时那边可能正跑着一个，撞上去会连到别人的世界。
    let port = free_port().unwrap_or(8899);
    let root = data_root()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    log_line(&root, &format!(
        "启动 server={} port={} root={}",
        server.display(), port, root.display()
    ));
    let mut child = match spawn_server(&server, port) {
        Ok(c) => c,
        Err(e) => {
            fatal(&format!("启动内核失败：{e}"));
            return;
        }
    };
    // 把内核锁进 Job：之后不管我们是正常退出还是被硬杀，它都活不下来。
    #[cfg(windows)]
    {
        match job::Job::new() {
            Some(j) => {
                if j.adopt(child.id()) {
                    // 句柄故意不释放：Drop 一跑就等于关掉 Job，内核立刻被杀。
                    // 让它开到这个进程结束为止，正是我们要的语义。
                    std::mem::forget(j);
                } else {
                    log_line(&root, "警告：内核没能加入 Job 对象，硬杀时会残留进程");
                }
            }
            None => log_line(&root, "警告：建不了 Job 对象，硬杀时会残留进程"),
        }
    }
    if !wait_ready(port, Duration::from_secs(25)) {
        let _ = child.kill();
        fatal("内核没有在 25 秒内就绪。看看同目录下的 logs/desktop.log。");
        return;
    }

    let url = format!("http://127.0.0.1:{port}/");
    log_line(&root, &format!("内核就绪 {url}"));
    let event_loop = EventLoop::new();
    let icon = Icon::from_rgba(ICON_RGBA.to_vec(), 32, 32).ok();
    let window = match WindowBuilder::new()
        .with_title(TITLE)
        .with_window_icon(icon)
        .with_inner_size(LogicalSize::new(1440.0, 900.0))
        .with_min_inner_size(LogicalSize::new(1024.0, 680.0))
        .build(&event_loop)
    {
        Ok(w) => w,
        Err(e) => {
            let _ = child.kill();
            fatal(&format!("开不了窗口：{e}"));
            return;
        }
    };
    let webview = match WebViewBuilder::new().with_url(&url).build(&window) {
        Ok(v) => v,
        Err(e) => {
            let _ = child.kill();
            fatal(&format!(
                "建不了 WebView2 窗口：{e}\n通常是因为系统缺 WebView2 运行时（Win10 1803 以下需装一次）。"
            ));
            return;
        }
    };

    let mut child = Some(child);
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                // 窗口和内核同生共死：关窗之后不留一个后台进程占着端口和存档目录。
                if let Some(mut c) = child.take() {
                    let _ = c.kill();
                    let _ = c.wait();
                }
                *control_flow = ControlFlow::Exit;
            }
            Event::MainEventsCleared => {
                // 窗口尺寸变了要让 WebView 跟着重排，否则页面停在一块小画布上。
                let _ = &webview;
            }
            _ => {}
        }
    });
}

fn fatal(msg: &str) {
    // 没有控制台（windows_subsystem），所以用系统对话框把话说清楚。
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let utf16: Vec<u16> = std::ffi::OsStr::new(msg)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let title: Vec<u16> = std::ffi::OsStr::new("钢心")
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            MessageBoxW(std::ptr::null_mut(), utf16.as_ptr(), title.as_ptr(), 0x10);
        }
    }
    #[cfg(not(windows))]
    eprintln!("{msg}");
}

#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    fn MessageBoxW(hwnd: *mut std::ffi::c_void, text: *const u16, caption: *const u16, kind: u32) -> i32;
}

/// Job 对象：把内核进程放进一个"父进程一死就一起死"的组里。
///
/// 为什么需要：正常关窗时我们已经在 `Child::kill` 里收拾了，但外壳被硬杀
/// （任务管理器、崩溃、蓝屏前的那一刻）时那段代码根本不会跑，
/// 于是每个 sim-server 都成了孤儿——占着内存，还锁着存档目录。
/// Job 对象把这件事交给内核（操作系统内核），不依赖我们的代码跑完。
#[cfg(windows)]
mod job {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};

    pub struct Job(HANDLE);

    impl Job {
        pub fn new() -> Option<Job> {
            unsafe {
                let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if h.is_null() {
                    return None;
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    h,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const std::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 {
                    CloseHandle(h);
                    return None;
                }
                Some(Job(h))
            }
        }
        /// 把已启动的子进程拉进这个 Job。子进程已经跑起来了再加入也可以——
        /// 内核不会再 spawn 别的东西，不存在"漏掉孙进程"的问题。
        pub fn adopt(&self, pid: u32) -> bool {
            unsafe {
                let p = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
                if p.is_null() {
                    return false;
                }
                let ok = AssignProcessToJobObject(self.0, p);
                CloseHandle(p);
                ok != 0
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }
}

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn find_server() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("IRONMIND_SERVER") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let dir = exe_dir();
    let name = if cfg!(windows) { "sim-server.exe" } else { "sim-server" };
    for cand in [dir.join(name), dir.join("bin").join(name), dir.join("..").join("bin").join(name)] {
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

fn free_port() -> Option<u16> {
    let l = TcpListener::bind("127.0.0.1:0").ok()?;
    l.local_addr().ok().map(|a| a.port())
}

/// settings.env：一行一个 `KEY=VALUE`，`#` 开头是注释。
/// 已经存在的同名环境变量优先——命令行上调试时不能被文件里的值盖掉。
/// 注意：这里只记**键名**，值（可能是 API key）绝不写日志。
fn read_settings(root: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Ok(text) = std::fs::read_to_string(root.join("settings.env")) else { return out };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        let Some((k, v)) = line.split_once('=') else { continue };
        let k = k.trim();
        let v = v.trim().trim_matches('"');
        if k.is_empty() || std::env::var_os(k).is_some() { continue; }
        out.push((k.to_string(), v.to_string()));
    }
    out
}

fn log_line(root: &Path, msg: &str) {
    use std::io::Write;
    let dir = root.join("logs");
    std::fs::create_dir_all(&dir).ok();
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("desktop.log"))
    {
        let _ = writeln!(f, "[desktop] {msg}");
    }
}

/// 装好的布局里 exe 在 `%LOCALAPPDATA%\IronMind\`，旁边就是 modules/ 和 saves/。
/// 那时把工作目录设到 exe 旁边，存档才跟着安装目录走；
/// 开发时（exe 在 target/release）保持调用者的工作目录，免得存档散到 target 里。
fn data_root() -> Option<PathBuf> {
    let dir = exe_dir();
    if dir.join("modules").is_dir() || dir.join("saves").is_dir() {
        Some(dir)
    } else {
        None
    }
}

fn spawn_server(server: &Path, port: u16) -> std::io::Result<Child> {
    let mut cmd = Command::new(server);
    cmd.arg("--port").arg(port.to_string());
    if let Some(root) = data_root() {
        std::fs::create_dir_all(root.join("logs")).ok();
        cmd.current_dir(&root);
        // 只有调用者没指定模块目录时才替它指到安装目录里的 modules/：
        // 显式设置的 IRONMIND_MODULES 永远优先。
        if std::env::var_os("IRONMIND_MODULES").is_none() {
            let mods = root.join("modules");
            if mods.is_dir() {
                cmd.env("IRONMIND_MODULES", &mods);
            }
        }
        // settings.env：给"双击快捷方式的人"一条配置通路。
        // 他们的桌上没有环境变量面板，但记事本能改一个文本文件。
        let settings = read_settings(&root);
        if !settings.is_empty() {
            log_line(&root, &format!(
                "settings.env: {} 项（{}）",
                settings.len(),
                settings.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(", ")
            ));
            for (k, v) in settings {
                cmd.env(k, v);
            }
        }
    }
    cmd.stdin(Stdio::null());
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("logs/desktop.log")
        .ok();
    match log {
        Some(f) => {
            let f2 = f.try_clone().ok();
            cmd.stdout(Stdio::from(f));
            if let Some(f2) = f2 {
                cmd.stderr(Stdio::from(f2));
            } else {
                cmd.stderr(Stdio::null());
            }
        }
        None => {
            cmd.stdout(Stdio::null());
            cmd.stderr(Stdio::null());
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW：不要给玩家弹一个黑框控制台。
        cmd.creation_flags(0x0800_0000);
    }
    cmd.spawn()
}

fn wait_ready(port: u16, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
            Duration::from_millis(400),
        )
        .is_ok()
        {
            // 端口通不代表路由就绪，但状态接口是最便宜的探针。
            return true;
        }
        std::thread::sleep(Duration::from_millis(120));
    }
    false
}
