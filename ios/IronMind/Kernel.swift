import Foundation

/// 内核：Rust 那一半（`crates/sim-ios`）。
///
/// 职责只有一件：把 C 接口包成人话，并把路径算对。**这里不写业务逻辑**——
/// 要改游戏就改 Rust 或网页（和安卓壳是同一个分工：壳只负责"把内核装进这台机器"）。
enum Kernel {
    /// 内核监听的端口。`0` = 没起来，原因在 `errorText`。
    private(set) static var port: UInt16 = 0
    /// 起不来的原因。要显示在屏幕上，不是只打日志——iOS 上没有控制台窗口，
    /// "起不来但什么都不说"等于让玩家看一块黑屏。
    private(set) static var errorText = ""

    /// 想要的端口。固定端口不是洁癖：页面按 `http://127.0.0.1:<port>` 区分 origin，
    /// 端口一变 `localStorage` 就换了一份——"引导看过了没有"这种标记会因此每次启动
    /// 都丢，玩家每次进来都被上手引导拦一遍。内核在被占时会自己往后顺延。
    private static let preferredPort: UInt16 = 18731

    /// app 沙盒里那个"玩家的目录"。
    ///
    /// 用 `Documents` 而不是 `Library`，是为了在 Info.plist 里开
    /// `UIFileSharingEnabled`：玩家能在「文件」App 里看到 `钢心/`，
    /// 把 `.imod` 直接拖进 `modules/`（和网页里「导入 .imod」是同一件事的两条路）。
    /// 存档 `saves/`、日志 `logs/`、模块 `modules/`、临时文件 `tmp/` 都在它下面。
    static var dataDir: URL {
        FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
    }

    /// 运行日志。真机上查问题时，这是唯一能拿到的东西。
    static var logFile: URL {
        dataDir.appendingPathComponent("logs/server.log")
    }

    /// 起内核。可以重复调（失败之后界面上的「重试」就是再调一次）。
    ///
    /// 第一次调用会：建 `Documents/modules/`、把打进静态库的那个战役摊成
    /// `modules/bundled.imod`、起 HTTP 服务。之后内核在后台线程里跑着，
    /// 页面指 `http://127.0.0.1:<port>/`。
    static func boot() {
        guard port == 0 else { return }   // 已经在跑，别起第二份世界

        // 先装传输：内核一旦开始跑，将领问询随时可能发生（第一 tick 就可能），
        // 那时候再去装就晚了——iOS 上没有 curl，晚一步就是一条"调用 curl 失败"。
        HttpBridge.install()

        var err = [CChar](repeating: 0, count: 1024)
        let cap = err.count      // 先取长度：`&err` 和 `err.count` 写在一个调用里会被独占性检查拦下
        let opened: UInt16 = dataDir.path.withCString { dir in
            ironmind_start(dir, nil, preferredPort, &err, cap)
        }
        port = opened
        errorText = opened == 0 ? String(cString: err) : ""
        NSLog("[钢心] 内核 %@ · 端口 %u · %@",
              String(cString: ironmind_version()),
              UInt32(opened),
              errorText.isEmpty ? "已启动" : errorText)
    }
}
