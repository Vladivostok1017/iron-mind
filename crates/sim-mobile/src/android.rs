//! 安卓专用实现：数据目录、内嵌模块、进程内内核、窗口与 WebView。

use std::path::PathBuf;
use std::sync::OnceLock;

use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::window::WindowBuilder;
use wry::WebViewBuilder;

/// 打进 `.so` 的战役模块（build.rs 打的 zip）。
const BUNDLED: &[u8] = include_bytes!(env!("SIM_BUNDLED_MODULE"));

static PORT: OnceLock<u16> = OnceLock::new();

/// app 的私有目录（`/data/user/0/<包名>/files`）。
///
/// 手机上唯一"想写哪就写哪"的地方。存档、日志、模块全落这儿。
fn files_dir() -> Option<PathBuf> {
    // SAFETY: ndk-context 由 tao/wry 在 Activity 起来时填好；
    // 我们只在 `_start_app`（Activity 已创建）之后读它。
    let ctx = ndk_context::android_context();
    let vm = unsafe { jni::JavaVM::from_raw(ctx.vm().cast()) }.ok()?;
    let mut env = vm.attach_current_thread().ok()?;
    let activity = unsafe { jni::objects::JObject::from_raw(ctx.context().cast()) };
    let file = env
        .call_method(&activity, "getFilesDir", "()Ljava/io/File;", &[])
        .ok()?
        .l()
        .ok()?;
    let path = env
        .call_method(&file, "getAbsolutePath", "()Ljava/lang/String;", &[])
        .ok()?
        .l()
        .ok()?;
    let s: String = env.get_string(&path.into()).ok()?.into();
    Some(PathBuf::from(s))
}

/// 把内嵌模块铺到 `modules/` 下。
///
/// 每次都覆盖：模块是内容，不是玩家数据（存档在 `saves/`，不受影响）。
/// 这样换一版 APK 就等于换了一版战役，不会出现"装了新版但玩的还是旧数据"。
fn install_bundled_module(data: &std::path::Path) -> Option<PathBuf> {
    let dir = data.join("modules");
    std::fs::create_dir_all(&dir).ok()?;
    let target = dir.join("bundled.imod");
    if let Err(e) = std::fs::write(&target, BUNDLED) {
        eprintln!("写内嵌模块失败：{e}");
        return None;
    }
    Some(target)
}

/// 定好环境、起内核，返回端口。
///
/// 环境变量是这个内核唯一认的"运行参数"来源（桌面版也一样），
/// 所以安卓这边不需要给内核开一个新接口——先设环境再调 `start` 就行。
fn boot_server() -> Option<u16> {
    let data = files_dir()?;
    std::fs::create_dir_all(&data).ok()?;
    let modules = data.join("modules");
    std::fs::create_dir_all(&modules).ok()?;

    // 存档 / 日志 / 模块的落脚点（内核里所有相对路径都归到这儿）
    std::env::set_var("IRONMIND_DATA", &data);
    std::env::set_var("IRONMIND_MODULES", &modules);

    let module = install_bundled_module(&data)?;
    println!("模块：{}", module.display());
    println!("数据：{}", data.display());

    // 端口 0 = 让系统分配。手机上不该跟别的 app 抢固定端口。
    match sim_server::start(&module.to_string_lossy(), 0) {
        Ok(p) => Some(p),
        Err(e) => {
            eprintln!("内核起不来：{e}");
            None
        }
    }
}

/// JNI 胶水。
///
/// 两个宏都展开成**一个块**（里面是 `#[no_mangle] extern "system" fn Java_…`），
/// 所以只能放在函数体里——放模块顶层会报 "macros that expand to items must be
/// delimited with braces"。`#[no_mangle]` 保证这些符号照样从 `.so` 里导出，
/// 放在哪个函数体里都一样。
fn install_jni() {
    // 包名写成下划线形式：com.ironmind.steelheart → com_ironmind + steelheart。
    // 必须和 AndroidManifest 里的包名一致，否则 Kotlin 调不到 `external fun`。
    //
    // 第三个参数是**声明 `external fun` 的那个类**，不是 Activity。
    // wry 0.57 的 Kotlin 把这些 external 都放在 `object Rust` 里
    // （`Rust.kt`：`external fun onCreate(activity: WryActivity)` …），
    // 所以这里必须写 `Rust`。写 `WryActivity` 会生成
    // `Java_..._WryActivity_onCreate` 这种没人调用的符号，
    // 运行时报 `UnsatisfiedLinkError: No implementation found for Rust.onCreate`。
    tao::android_binding!(
        com_ironmind,
        steelheart,
        Rust,
        wry::android_setup,
        _start_app
    );
    wry::android_binding!(com_ironmind, steelheart);
}

/// tao/wry 的安卓入口：由生成的 `WryActivity` 在 Activity 创建后调用。
///
/// 名字固定（`tao::android_binding!` 的最后两个参数指向它）。
pub fn _start_app() {
    install_jni();

    // 内核先起来：WebView 一加载就要拿 `/api/state`，晚一步页面就是一片"连不上"。
    let port = *PORT.get_or_init(|| boot_server().unwrap_or(0));
    if port == 0 {
        // 内核没起来：窗口照开，但页面会显示"连不上"。至少不要静默无反应。
        eprintln!("内核未启动：没有可用端口");
    }
    let url = format!("http://127.0.0.1:{port}/");
    println!("打开：{url}");

    let event_loop = EventLoop::new();
    let mut webview: Option<wry::WebView> = None;
    let mut built = false;

    event_loop.run(move |event, target, control_flow| {
        // 手机上不能忙等：Wait 让系统在没事件时把 CPU 放开（省电、不过热）。
        *control_flow = ControlFlow::Wait;

        // 窗口只能在事件循环里建（`WindowBuilder::build` 要 `&EventLoopWindowTarget`），
        // 而这个版本的 tao 没有 `Event::Resumed`，所以"第一次进来就建"是唯一的位置。
        // 此时 Activity 已经 onCreate 过（CONTEXTS 里有它），建得出来。
        if !built {
            built = true;
            let window = match WindowBuilder::new().with_title("钢心").build(target) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("建窗口失败：{e}");
                    return;
                }
            };
            match WebViewBuilder::new().with_url(&url).build(&window) {
                Ok(wv) => webview = Some(wv),
                Err(e) => eprintln!("建 WebView 失败：{e}"),
            }
            return;
        }

        // 安卓从后台回来会销毁窗口；句柄跟着失效，下次绘制前要让 WebView 重建。
        if let Event::WindowEvent { event: WindowEvent::Destroyed, .. } = event {
            webview = None;
        }
        // 读一下句柄。`wry::WebView` 一被 drop，页面就白了——它必须活到下一次事件。
        // 这一行不是废话，是把"只写不读"的意图写明（否则编译器会警告，
        // 下一个人可能真把它当无用变量删掉）。
        let _alive = webview.is_some();
    });
}
