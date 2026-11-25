import UIKit

/// App 入口。
///
/// 这里**只**做一件事：尽早把内核起起来。界面在 `SceneDelegate` / `GameViewController`。
@main
final class AppDelegate: UIResponder, UIApplicationDelegate {

    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
    ) -> Bool {
        // 比 Scene 早一步：WebView 一加载就会来敲 `/api/state`，晚一步首页会先闪
        // 一下"连接中……"。内核本身是同步起好的（模块已经在静态库里，不读网络）。
        Kernel.boot()
        return true
    }

    func application(
        _ application: UIApplication,
        configurationForConnecting connectingSceneSession: UISceneSession,
        options: UIScene.ConnectionOptions
    ) -> UISceneConfiguration {
        UISceneConfiguration(name: "Default", sessionRole: connectingSceneSession.role)
    }
}
