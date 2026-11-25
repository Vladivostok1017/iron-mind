import UIKit
import WebKit

/// 界面的全部实现：一块铺满屏幕的 `WKWebView`，指向内核的 `127.0.0.1`。
///
/// 这个类只做四件事——其余全在网页和内核里（和安卓壳的分工一模一样）：
///
/// 1. 起 WebView、指到 `http://127.0.0.1:<port>/`；
/// 2. 键盘弹出时把 WebView 的底边抬起来（否则电报台的输入框会被键盘整个盖住）；
/// 3. 接管网页里的 `alert` / `confirm` / `prompt`。**不接管就是静默返回 false**：
///    玩家点「换战役 → 继续」什么都不会发生，看起来像按钮坏了；
/// 4. 内核起不来时显示一句人话 + 「重试」，而不是一块黑屏。
final class GameViewController: UIViewController {

    private enum Palette {
        /// 和网页 `#0b0e12` 一致：加载时不闪白。
        static let background = UIColor(red: 0.043, green: 0.055, blue: 0.071, alpha: 1)
        static let text = UIColor(red: 0.812, green: 0.839, blue: 0.871, alpha: 1)
        static let dim = UIColor(red: 0.475, green: 0.510, blue: 0.557, alpha: 1)
        static let gold = UIColor(red: 0.890, green: 0.690, blue: 0.294, alpha: 1)
    }

    private var webView: WKWebView!
    /// WebView 的底边：键盘弹出时改成负数，把页面压到键盘上面。
    private var bottomEdge: NSLayoutConstraint!
    private var failureView: UIView?
    private var retries = 0

    // MARK: - 生命周期

    override func loadView() {
        let root = UIView(frame: .zero)
        root.backgroundColor = Palette.background

        let wv = makeWebView()
        root.addSubview(wv)
        wv.translatesAutoresizingMaskIntoConstraints = false
        // **贴安全区**，不指望页面的 `env(safe-area-inset-*)`。
        //
        // 原本的打算是"全屏铺满 + 让 CSS 自己躲刘海"（页面里写了那套，还带
        // `viewport-fit=cover`）。真机/模拟器上量了一下：WKWebView 里
        // `env(safe-area-inset-top)` 返回 **0**，于是顶栏直接贴上屏幕最上沿，
        // 「钢」字徽记被灵动岛压住一半 —— 截图里一眼就能看见。
        //
        // 所以改成由 UIKit 负责：上下左右都贴 safeAreaLayoutGuide，页面的
        // `env()` 恒为 0，`calc(46px + env(...))` 就退化成 46px，正好不重复。
        // 代价是横屏时左右各让出一条安全区（`#0b0e12` 底色），换来"永远不被压住"。
        bottomEdge = wv.bottomAnchor.constraint(equalTo: root.safeAreaLayoutGuide.bottomAnchor)
        NSLayoutConstraint.activate([
            wv.topAnchor.constraint(equalTo: root.safeAreaLayoutGuide.topAnchor),
            wv.leadingAnchor.constraint(equalTo: root.safeAreaLayoutGuide.leadingAnchor),
            wv.trailingAnchor.constraint(equalTo: root.safeAreaLayoutGuide.trailingAnchor),
            bottomEdge,
        ])

        webView = wv
        view = root
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(keyboardWillChange(_:)),
            name: UIResponder.keyboardWillChangeFrameNotification,
            object: nil)
        load()
    }

    // 游戏要满屏：状态栏和 home 指示条都不该压在图上。
    override var prefersStatusBarHidden: Bool { true }
    override var prefersHomeIndicatorAutoHidden: Bool { true }

    override var supportedInterfaceOrientations: UIInterfaceOrientationMask {
        // 横竖都放开——安卓锁横屏是为了绕开"WebView 不重排"，iOS 没这个毛病。
        // iPhone 不要倒置（刘海在下面时安全区算法别扭）；iPad 允许。
        UIDevice.current.userInterfaceIdiom == .pad ? .all : .allButUpsideDown
    }

    // MARK: - WebView

    private func makeWebView() -> WKWebView {
        let cfg = WKWebViewConfiguration()
        // 默认的数据存储：localStorage 要跨启动活着。内核端口固定之后 origin 就稳定，
        // "引导看过了""上次玩到哪"这些标记才留得住。
        cfg.websiteDataStore = .default()
        cfg.defaultWebpagePreferences.allowsContentJavaScript = true
        cfg.allowsInlineMediaPlayback = true
        cfg.userContentController.addUserScript(Self.blockPageZoom)

        let wv = WKWebView(frame: .zero, configuration: cfg)
        wv.navigationDelegate = self
        wv.uiDelegate = self
        wv.scrollView.contentInsetAdjustmentBehavior = .never
        wv.scrollView.bounces = false
        // 页面是"固定布局 + 内部各自滚动"（地图、侧栏抽屉），外层不该再滚：
        // 否则单指拖地图会同时拖动整个页面，手感是地图在飘。
        wv.scrollView.isScrollEnabled = false
        wv.allowsBackForwardNavigationGestures = false
        wv.allowsLinkPreview = false
        wv.isOpaque = false
        wv.backgroundColor = Palette.background
        #if DEBUG
        // 能在 Mac 的 Safari「开发 → 你的 iPhone」里直接调页面。
        if #available(iOS 16.4, *) { wv.isInspectable = true }
        #endif
        return wv
    }

    /// 掐掉 WebKit 自己的**页面级**缩放。
    ///
    /// 地图实现了自己的双指缩放（`index.html` 里那套 pinch）。但 iOS 从 10 起
    /// 出于无障碍考虑**忽略** `user-scalable=no`，于是双指会先缩放整个页面、
    /// 再交给地图，手感是"越捏越歪"，松手后页面还留在放大状态。
    /// `gesturestart/change/end` 是 WebKit 的页面缩放手势事件，拦掉它们之后
    /// 只剩地图自己的 `touchstart/touchmove`（不受影响）。
    private static let blockPageZoom = WKUserScript(
        source: """
        ['gesturestart', 'gesturechange', 'gestureend'].forEach(function (name) {
            document.addEventListener(name, function (e) { e.preventDefault(); }, { passive: false });
        });
        """,
        injectionTime: .atDocumentStart,
        forMainFrameOnly: true)

    // MARK: - 起内核 / 加载页面

    private func load() {
        Kernel.boot()
        guard Kernel.port > 0 else {
            showFailure(Kernel.errorText)
            return
        }
        hideFailure()
        guard let url = URL(string: "http://127.0.0.1:\(Kernel.port)/") else { return }
        webView.load(URLRequest(url: url))
    }

    @objc private func retry() {
        retries = 0
        load()
    }

    // MARK: - 键盘

    /// 键盘一出来就把 WebView 的底边抬到键盘上沿。
    ///
    /// 为什么不是给页面加内边距：页面的高度是 `100dvh`，视口一变它就自己重排——
    /// 抬 WebView 等于同时改了 CSS 视口和 `dvh`，一行 JS 都不用加。
    /// 电报台的输入框、底部工具栏因此正好落在键盘上面。
    ///
    /// 注意这是**唯一**必须在真机上对着看的东西：iOS 的键盘高度、快捷输入栏、
    /// 外接键盘的浮动键盘都走这条通知，数值转屏时会变，所以每次重算。
    @objc private func keyboardWillChange(_ note: Notification) {
        guard let info = note.userInfo,
              let end = (info[UIResponder.keyboardFrameEndUserInfoKey] as? NSValue)?.cgRectValue,
              let duration = (info[UIResponder.keyboardAnimationDurationUserInfoKey] as? NSNumber)?.doubleValue
        else { return }

        let frame = view.convert(end, from: nil)
        let overlap = max(0, view.bounds.maxY - frame.minY)
        guard bottomEdge.constant != -overlap else { return }

        bottomEdge.constant = -overlap
        UIView.animate(withDuration: duration, delay: 0, options: [.beginFromCurrentState]) {
            self.view.layoutIfNeeded()
        }
    }

    /// 转屏后**整个重载页面**。
    ///
    /// 试过的、没用的办法：在页面 `resize` 里把 `meta viewport` 重写一遍（那是给普通
    /// 网页的招）。WKWebView 上不管用——竖 → 横 → 竖 转一圈回来，页面会停在放大过的
    /// 缩放级别：字变大、右边被裁，而且只有重载能恢复。
    ///
    /// 所以走安卓那条老路（`MainActivity` 转屏重建同理）：直接 reload。
    /// 代价是地图的平移/缩放复位；换来的是方向和字号永远对。
    /// 能让这个代价可接受的前提是**状态都在内核里**：页面只是视图，
    /// 电报、公文、请示重载后照样从 `/api/*` 拉回来；`sessionStorage.im_in_game`
    /// 也活着，所以重载后直接回到这一局，不会把玩家丢回首页。
    override func viewWillTransition(
        to size: CGSize,
        with coordinator: UIViewControllerTransitionCoordinator
    ) {
        super.viewWillTransition(to: size, with: coordinator)
        coordinator.animate(alongsideTransition: nil) { [weak self] _ in
            self?.webView.reload()
        }
    }

    // MARK: - 起不来的时候

    private func showFailure(_ text: String) {
        hideFailure()

        let box = UIView(frame: .zero)
        box.backgroundColor = Palette.background
        box.translatesAutoresizingMaskIntoConstraints = false

        let title = UILabel()
        title.text = "内核没能起来"
        title.textColor = Palette.text
        title.font = .systemFont(ofSize: 17, weight: .semibold)
        title.textAlignment = .center

        let detail = UILabel()
        detail.text = text.isEmpty ? "没有更多信息（日志在「文件」App 的 钢心/logs/ 里）" : text
        detail.textColor = Palette.dim
        detail.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
        detail.numberOfLines = 0
        detail.textAlignment = .center

        let button = UIButton(type: .system)
        button.setTitle("重试", for: .normal)
        button.setTitleColor(Palette.gold, for: .normal)
        button.titleLabel?.font = .systemFont(ofSize: 15, weight: .medium)
        button.addTarget(self, action: #selector(retry), for: .touchUpInside)

        let stack = UIStackView(arrangedSubviews: [title, detail, button])
        stack.axis = .vertical
        stack.spacing = 16
        stack.alignment = .fill
        stack.translatesAutoresizingMaskIntoConstraints = false
        box.addSubview(stack)

        view.addSubview(box)
        NSLayoutConstraint.activate([
            box.topAnchor.constraint(equalTo: view.topAnchor),
            box.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            box.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            box.bottomAnchor.constraint(equalTo: view.bottomAnchor),
            stack.leadingAnchor.constraint(equalTo: box.safeAreaLayoutGuide.leadingAnchor, constant: 28),
            stack.trailingAnchor.constraint(equalTo: box.safeAreaLayoutGuide.trailingAnchor, constant: -28),
            stack.centerYAnchor.constraint(equalTo: box.centerYAnchor),
        ])
        failureView = box
    }

    private func hideFailure() {
        failureView?.removeFromSuperview()
        failureView = nil
    }
}

// MARK: - 页面加载

extension GameViewController: WKNavigationDelegate {

    func webView(_ webView: WKWebView,
                 didFailProvisionalNavigation navigation: WKNavigation!,
                 withError error: Error) {
        // 内核刚 listen 上的那一瞬，页面可能已经来敲了。这不算故障：
        // 等 0.3 秒再试，连试三次还不行才把错误摆到屏幕上。
        guard retries < 3 else {
            showFailure("连不上内核：\(error.localizedDescription)")
            return
        }
        retries += 1
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { [weak self] in
            self?.load()
        }
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        retries = 0
        hideFailure()
    }

    func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
        // 内存紧张时 iOS 会把 WebKit 的渲染进程杀掉，`WKWebView` 剩下一张空白
        // （切出去开几个别的 App 再回来就可能遇到）。重载一次就回来了。
        //
        // 这个恢复能"无痛"成立，靠的是内核和页面分离：世界状态在 Rust 那边，
        // 页面只是它的一个视图。换安卓那边是同一个理由。
        webView.reload()
    }
}

// MARK: - 网页里的对话框

/// `WKWebView` 不会自己弹 JS 的 `alert` / `confirm` / `prompt`：
/// 没实现这些代理，`confirm()` 会**静默返回 false**。
///
/// 这个游戏用 `confirm` 的地方都是"要丢进度了，确定吗"（换战役、撤换将领），
/// 静默 false 的表现就是"按钮点了没反应"——比报错更难查。
extension GameViewController: WKUIDelegate {

    func webView(_ webView: WKWebView,
                 runJavaScriptAlertPanelWithMessage message: String,
                 initiatedByFrame frame: WKFrameInfo,
                 completionHandler: @escaping () -> Void) {
        guard presentedViewController == nil else { return completionHandler() }
        let alert = UIAlertController(title: nil, message: message, preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "知道了", style: .default) { _ in completionHandler() })
        present(alert, animated: true)
    }

    func webView(_ webView: WKWebView,
                 runJavaScriptConfirmPanelWithMessage message: String,
                 initiatedByFrame frame: WKFrameInfo,
                 completionHandler: @escaping (Bool) -> Void) {
        guard presentedViewController == nil else { return completionHandler(false) }
        let alert = UIAlertController(title: nil, message: message, preferredStyle: .alert)
        // 顺序照桌面：先给"取消"，默认动作才是"继续"。
        alert.addAction(UIAlertAction(title: "算了吧", style: .cancel) { _ in completionHandler(false) })
        alert.addAction(UIAlertAction(title: "继续", style: .default) { _ in completionHandler(true) })
        present(alert, animated: true)
    }

    func webView(_ webView: WKWebView,
                 runJavaScriptTextInputPanelWithPrompt prompt: String,
                 defaultText: String?,
                 initiatedByFrame frame: WKFrameInfo,
                 completionHandler: @escaping (String?) -> Void) {
        // 前端已经不用 `prompt()` 了（存档/读档改成了自绘对话框），
        // 但模块和 SDK 可能用。给一个真能输入的，别静默返回 nil。
        guard presentedViewController == nil else { return completionHandler(nil) }
        let alert = UIAlertController(title: nil, message: prompt, preferredStyle: .alert)
        alert.addTextField { $0.text = defaultText }
        alert.addAction(UIAlertAction(title: "取消", style: .cancel) { _ in completionHandler(nil) })
        alert.addAction(UIAlertAction(title: "确定", style: .default) { [weak alert] _ in
            completionHandler(alert?.textFields?.first?.text)
        })
        present(alert, animated: true)
    }
}
