# 25 · iOS：同一个内核，第三种壳

前面那一堆讲的是**这场战争怎么模拟**。这一篇讲的是第三件事：
怎么把同一个内核塞进一台 iPhone，**一行模拟逻辑都不改**。

和 [`24-桌面版.md`](24-桌面版.md)（原生窗口）以及 `HANDOFF.md` 第 14 节（安卓）
是同一个故事的第三幕。三者的分工完全一样：

```
         ┌──────────────────────────┐
         │  sim-core / sim-content  │  确定性内核 + 模块加载（一行没动）
         ├──────────────────────────┤
         │  sim-server              │  HTTP 接口 + 单文件网页（一行没动）
         ├──────────────────────────┤
桌面：sim-desktop（子进程）  安卓：sim-mobile（同进程 .so）  iOS：sim-ios（同进程 .a）
         │                          │                          │
      原生窗口                   tao + wry                  Swift + WKWebView
```

**iOS 这一层只多了 4 个 C 函数和 5 个 Swift 文件。** 引擎那边真正的改动只有一处：
出站 HTTP 多了一个可以"换传输层"的口子（下面第 4 节，iOS 上没有 `curl`）。

---

## 1. 为什么不用 wry 的 iOS 后端

`HANDOFF.md` 里原来的判断是"iOS 没碰：`wry` 有 iOS 后端，但要 Mac + Xcode"。
真的开始做的时候，理由变成了三条，每条都不是"麻烦"，而是"做不到"：

| 要的东西 | wry/tao 的 iOS 后端 | 用 Swift 写壳 |
|---|---|---|
| 网页里的 `confirm()` | 得自己往 tao 里塞 `WKUIDelegate` | 一个 extension，20 行 |
| 键盘弹出时把页面顶上去 | tao 不转发这个通知 | 一条系统通知，10 行 |
| 状态栏 / home 指示条 / 方向锁 / 文件共享 | 都要改 tao 内部 | Info.plist + 一个属性 |
| 崩溃点在哪 | Rust ↔ ObjC ↔ Swift 三层 | Xcode 的调用栈一眼看到底 |

**安卓之所以用 wry 是对的**：那边一套 Rust 代码就把窗口和 WebView 都办了，
不必碰 Kotlin。**iOS 反过来**：这四件事没有一件能用"跨平台窗口库"办得比 UIKit
更省事，而它们恰好是"手机上能不能玩"的全部——键盘盖住输入框、`confirm()`
静默返回 false、页面被双指缩放搞坏，三个都是**玩家会当成游戏坏了**的那种 bug。

所以 iOS 这层是：**Rust 管内核，Swift 管那台机器**。

---

## 2. 结构

```
crates/sim-ios/            iOS 壳的 Rust 半边（staticlib → libsim_ios.a）
  build.rs                 内嵌战役模块（和安卓共用 crates/sim-bundle）
  src/lib.rs               4 个 C 函数：start / set_http_post / version /（错误缓冲区）
  tests/kernel.rs          **在 macOS 上真起内核、真打 HTTP** 的测试
crates/sim-bundle/         外壳 build.rs 的公共部分：模块目录 → .imod（zip）
crates/sim-server/src/http.rs   出站 HTTP：桌面/安卓走 curl，iOS 走 URLSession
ios/
  project.yml              Xcode 工程的**源文件**（XcodeGen 生成下面的 .xcodeproj）
  IronMind.xcodeproj/      生成好并提交，装了 Xcode 就能直接打开
  IronMind/
    IronMind-Bridging-Header.h   Rust 的 C 接口声明（Swift 与 Rust 唯一的契约）
    Kernel.swift            起内核、算数据目录、拿端口
    HttpBridge.swift        出站 HTTPS → URLSession（iOS 上没有 curl）
    GameViewController.swift 一块全屏 WKWebView + 键盘 + JS 对话框 + 失败页
    SceneDelegate.swift     一个窗口
    AppDelegate.swift       入口，尽早起内核
    Info.plist              ATS / 方向 / 文件共享 / 启动画面
    Assets.xcassets/        图标（tools/make-icon-ios.py 生成）
tools/build-ios-rust.sh     cargo → target/ios/libsim_ios.a（Xcode 的 preBuildScript 调它）
tools/build-ios.sh          一条命令出未签名 .ipa / 在模拟器里跑起来
tools/make-icon-ios.py      全套图标（18 个尺寸 + Contents.json）
```

页面从哪来：**和安卓一样编在二进制里**。`sim-server` 用 `include_str!` 把
`web/index.html` 编进去，`sim-ios` 把整个 `.imod` 编进去。装上之后：

- 不需要电脑、不需要联网（接了模型才用网络）；
- 不需要任何外部文件（战役模块启动时摊到沙盒的 `modules/bundled.imod`）。

---

## 3. 启动时序

```
AppDelegate.didFinishLaunching
  └─ Kernel.boot()
       ├─ HttpBridge.install()            先装传输（第一 tick 就可能要问模型）
       ├─ ironmind_start(Documents, NULL, 18731, err, 1024)
       │     ├─ 建 Documents/{modules,saves,logs,tmp}
       │     ├─ 设 IRONMIND_DATA / IRONMIND_MODULES
       │     ├─ 写 Documents/modules/bundled.imod（每次覆盖：模块是内容，不是玩家数据）
       │     ├─ sim_server::start(模块, 18731)
       │     │     └─ 绑不上就退 18732 / 18733 / 0（让系统分），绝不因为端口冲突起不来
       │     └─ 返回端口（失败返回 0 + 一句中文错误）
       └─ port == 0 ? 显示「内核没能起来 + 重试」 : 记下端口

SceneDelegate → GameViewController.loadView
  └─ WKWebView（全屏、不做安全区内缩、内容缩放关掉、弹窗代理接上）
       └─ load("http://127.0.0.1:<port>/")
```

**端口为什么要固定**：页面按 `http://127.0.0.1:<port>` 区分 origin。端口一变，
`localStorage` 就是另一份——"引导看过了没有"这个标记会每次启动都丢，
玩家每次进来都被上手引导拦一遍。所以试 `18731`，被占才往后顺延。

---

## 4. iOS 特有的五个坑

这五条每条都能让游戏"能装、能开、但玩不了"，而且**都不会报错**。

### 4.1 `curl` 不存在

内核调大模型原来是 `Command::new("curl")`（`docs/24` 说的"不引第三方 HTTP 依赖"）。
iOS 上既没有 `/usr/bin/curl`，App 也被禁止 fork/exec 别的二进制。

不处理的表现：将领全部退回本地规则判断 —— 游戏照玩，但"LLM 将领"这个 v1.0 的
卖点没了，**日志里只有一行"调用 curl 失败"**。

做法（`crates/sim-server/src/http.rs`）：把"发一个 POST、拿回一段文本"抽成一个口子，
默认仍走 curl（桌面/安卓的行为一个字没变），iOS 外壳在启动时装一个函数进来：

```rust
pub type BridgeFn = unsafe extern "C" fn(
    ctx, url, key, body, timeout_secs, out, out_len,
) -> c_int;   // 返回 HTTP 状态码，< 0 表示传输失败
```

Swift 那边用 URLSession + 信号量实现（`HttpBridge.swift`）：内核在**后台线程**
同步等结果，URLSession 在它自己的队列上回调，写完缓冲区再放行。所以它绝不能
跑在主线程上——内核里所有模型请求都走 worker 线程，这条前提成立。

> 没选"给内核引一个 rustls/Hyper"：那会把"用谁的证书、允不允许明文"变成内核的事。
> 现在内核只管发请求，传输归外壳，三端的行为都能各自解释清楚。

### 4.2 ATS：http://127.0.0.1 默认被拦

`Info.plist` 里 `NSAppTransportSecurity → NSAllowsLocalNetworking = true`。
不写这条，WKWebView 加载 `http://127.0.0.1:<port>` 直接失败，表现是**白屏**。

### 4.3 网页里的 `confirm()` 静默返回 false

`WKWebView` 不会自己弹 JS 对话框。没实现 `WKUIDelegate` 的话：

- 点「换战役 → 继续」**没反应**（`confirm` 返回 false，代码以为你点了取消）；
- 点「撤换将领」也一样。

这个游戏用 `confirm` 的地方全都是"要丢进度了，确定吗"，所以"没反应"是最难查的
那种 bug。`GameViewController` 里三个代理（alert / confirm / prompt）都实现了。

### 4.4 键盘盖住电报台

iPhone 上键盘弹出时**窗口尺寸不变**，只是盖在上面。页面高度是 `100dvh`，
所以底部工具栏和发报按钮会被整个盖住——而"发电报"是这个游戏唯一的输入动作。

做法：监听 `keyboardWillChangeFrameNotification`，把 WebView 的**底边**抬到键盘上沿。
视口一变，`100dvh` 跟着变，页面自己重排，一行 JS 都不用加。

> 这条是**唯一**必须在真机上对着调的地方：键盘高度、快捷输入栏、外接键盘的浮动
> 键盘走的都是这条通知，数值转屏时会变，所以每次重算。

### 4.5 双指缩放会同时缩放整个页面

地图自己实现了双指缩放，但 iOS 从 10 起**忽略** `user-scalable=no`（无障碍原因）。
于是双指会先把整个页面缩放一遍、再交给地图，手感是"越捏越歪"。

做法：`WKUserScript` 在 `documentStart` 拦掉 `gesturestart / gesturechange / gestureend`
（WebKit 的页面缩放手势事件）。地图那套用的是 `touchstart / touchmove`，不受影响。

### 4.6 装完 Xcode ≠ 能编 iOS：平台包是另下的

Xcode 16 起，**iOS 平台支持**（设备支持 + 模拟器运行时）是一个**单独的下载**，
不再随 Xcode 一起装。`/Applications/Xcode.app/.../SDKs/iPhoneOS18.5.sdk` 明明在，
`xcodebuild -showsdks` 也能列出 `-sdk iphoneos18.5`，但：

```
xcodebuild: error: Unable to find a destination matching the provided destination specifier:
        { generic:1, platform:iOS }
  Ineligible destinations for the "IronMind" scheme:
        { platform:iOS, ..., error:iOS 18.5 is not installed.
          To use with Xcode, first download and install the platform }
```

更阴的是 `actool`（编图标）也要它，报的却是一句和图标毫无关系的话：

```
error: No available simulator runtimes for platform iphonesimulator.
       SimServiceContext supportedRuntimes=[]
```

修法只有一条（实测下的是 **8.86 GB**，落在 `~/Library/Developer` 下；Xcode 16.4
给的运行时是 **iOS 18.6 (22G86)**）：

```bash
xcodebuild -downloadPlatform iOS
```

> 踩坑记录：`-target IronMind -sdk iphoneos` 这种老式写法**能绕过 destination 检查**、
> 一路编到 Swift 和 Rust 静态库都过，但会死在 `CompileAssetCatalogVariant`
> —— 所以平台包躲不掉，只是能拿来验证"Swift 编得过、链接过得去"。
> `tools/build-ios.sh` 现在会先探一下，没平台包就自动退到 `-target` 模式并提示。

另外还有一道更早的闸：**Xcode 许可协议**没同意时，`clang` / `swiftc` / `actool` /
`codesign` 全部只回一句 "You have not agreed to the Xcode license agreements"，
看起来像工具没装。第一次装好后要：

```bash
sudo xcodebuild -license accept && sudo xcodebuild -runFirstLaunch
```

### 4.7 安全区：`env(safe-area-inset-*)` 在 WKWebView 里是 **0**

页面是按"全屏铺满 + CSS 自己躲刘海"写的（`viewport-fit=cover` + `env(safe-area-inset-*)`）。
这套在 Safari 里成立，在 WKWebView 里**实测不成立**：模拟器（iPhone 16 Pro）截图上
「钢」字徽记直接贴上屏幕最上沿、被灵动岛压掉一半。

所以 iOS 壳改成由 UIKit 负责：WebView 四条边都贴 `safeAreaLayoutGuide`。
页面的 `env()` 恒为 0，`calc(46px + env(...))` 退化成 46px，正好不重复。
代价是横屏时左右各让出一条 `#0b0e12` 底色，换来"永远不被压住"。

> 这条是 `docs/25` 早期版本里列的"必须真机看一眼"的第 7 条 —— 一看就中了。

### 4.8 模拟器架构：Intel 上要 x86_64，`generic` 目标还要 fat 库

- 这台是 Intel（i7-10700F），模拟器跑 **x86_64**，不是 arm64。
- `-destination 'generic/platform=iOS Simulator'` 会同时要 **arm64 + x86_64** 两种切片。

只给一种的后果是几百行 `found architecture 'arm64', required architecture 'x86_64'`
（忽略文件）然后 `symbol(s) not found`。`tools/build-ios-rust.sh` 现在按 `ARCHS`
逐个编、再 `lipo` 成一份 fat 库（模拟器 debug 版 105 MB）。

### 4.9 `simctl install` 不会重启已在跑的进程

改完代码重新装进模拟器，截图上还是**上一版的界面**——因为 `simctl install` 只替换
bundle，进程还在跑旧代码。必须先 `simctl terminate`。脚本里已经补上这一步。

（我第一次改完安全区就是这样：截图和上一张的 md5 一模一样，才发现根本没生效。）

### 4.10 横屏底栏比安卓高一截：安全区吃掉了宽度

现象：横过来之后底部工具栏比安卓高，因为「推进到有事」这类长标签换成了两行。

根因不在字，在**宽度**：iPhone 横屏时左右安全区各约 59pt，而 WebView 贴了安全区
（4.7 的修法），于是页面可用宽度从 874 掉到 **约 756 CSS px**。而横屏底栏原来是
**写死宽度**的：

```
#modes 固定 208px + 5 个管理按钮 × 54px = 270 + 间隙 45 + 内边距 16 = 539px
剩给三个推进按钮 = 756 − 539 = 217px → 60 / 75 / 60
「推进到有事 ⏎」至少要 ~77px → 换行 → 底栏撑成两行
```

安卓那边 WebView 是全宽的（874px），同样的样式就不换行 —— 所以这不是 iOS 专属问题，
只是 iOS 更早撞上。

改法（在共享的 `web/index.html` 里，三端一起受益）：

1. 横屏底栏的**定宽改比例**：`#modes` 从 `flex: 0 0 208px` 改 `1.6 1 0`，
   管理按钮从 `0 0 54px` 改 `0.7 1 0`，三个推进按钮 1.5 / 1.9 / 1.5；
2. `white-space: nowrap` + `text-overflow: ellipsis`：**任何标签都不许把底栏撑高**；
3. 触屏上隐藏「⏎ / ␣」这两个键盘提示（桌面才需要，手机上只占宽度）；
4. 加一档更窄的横屏（`max-height: 430px`，iPhone SE 一类）再让一步字号。

---

## 5. 怎么出包

```bash
# 0) 先看这台机器缺什么：少 Xcode / 少 iOS SDK / 少模拟器运行时 / 少 Rust target，
#    每条都会给一句能直接粘进终端的修法（缺 Xcode 时它也会说，而不是等 linker 报天书）
tools/check-ios.sh

# 1) 一次性准备（Mac + 完整 Xcode，不是 Command Line Tools）
sudo xcodebuild -license accept            # 不跑这个，clang/swiftc/actool 全部不可用
xcodebuild -downloadPlatform iOS           # Xcode 16 起平台包是另下的（约 8 GB）
brew install xcodegen                      # 只有改 project.yml 时才需要
rustup target add aarch64-apple-ios aarch64-apple-ios-sim

# 2) 出未签名 .ipa → dist/IronMind-ios-1.3.0.ipa
tools/build-ios.sh

# 3) 想换内嵌的战役（默认是引擎自带的演示模块）
tools/build-ios.sh --module ../nanxun-1938/nanxun_1938.imod

# 4) 在模拟器里直接跑起来（不需要 Apple ID）
tools/build-ios.sh --simulator

# 5) 用自己的开发证书签好并装到连着的设备上
tools/build-ios.sh --signed --install
```

**为什么产物不签名**：签名要用**你的** Apple ID 和描述文件，那是人和机器的东西，
不该进仓库。安卓那张自签钥匙只需要证明"同一个 APK 的后续版本"，iOS 没有等价物
——每台设备上装东西都得 Apple 发一张证书。所以这里出未签名 `.ipa`，交给：

1. **Xcode 直接 Run**（最省事）：打开 `ios/IronMind.xcodeproj` → Signing &
   Capabilities 选你的 Team → 选设备 → ⌘R；
2. **侧载工具**（AltStore / Sideloadly）：把 `.ipa` 拖进去，用 Apple ID 重签。
   免费账号 7 天有效期，过期重签；
3. 自己的签名流程。

`project.yml` 是工程设置**唯一的源文件**（`IronMind.xcodeproj` 是它生成的，也提交了，
这样没装 XcodeGen 的人照样能打开）。改了设置跑一次：

```bash
cd ios && xcodegen generate --spec project.yml --project .
```

---

## 6. 模块从哪来

内嵌的那个在编译期就决定了（`IRONMIND_BUNDLE_MODULE`，和安卓同一个环境变量、
同一份打包代码 `crates/sim-bundle`）——两个平台对同一个模块目录必须打出**字节一致**的包，
否则"为什么我这台手机上不一样"永远查不完。

之后换模块有三条路，按省事排序：

| 路子 | 怎么做 | 说明 |
|---|---|---|
| **网页里导入** | 首页「新的战役 → 导入 .imod」 | `<input type=file>` 在 WKWebView 里会调起系统文件选择器，能用 |
| **「文件」App 拖进去** | 我的 iPhone → 钢心 → `modules/` | `UIFileSharingEnabled` 开的；放进去的文件列表里直接就是 |
| **重新出包** | `tools/build-ios.sh --module ...` | 模块打在 `.a` 里 |

**"导入文件夹"在 iOS 上不工作**：它靠 `<input webkitdirectory>`，iOS 不支持（安卓能用）。
好消息是发布形态本来就该是一个 `.imod` 文件，不是一堆 json。

> **注意**：从 Finder 双击打开 Xcode 时，`IRONMIND_BUNDLE_MODULE` **不在** Xcode 的
> 环境里——那次构建会退回引擎自带的演示模块。想固定内嵌某个战役，两条路：
> 用 `tools/build-ios.sh --module ...`（它把环境变量带进 xcodebuild），或者在
> Xcode 的 *Scheme → Run → Arguments → Environment Variables* 里加一条
> `IRONMIND_BUNDLE_MODULE=/路径/xxx.imod`。

---

## 7. 已知限制

| 限制 | 为什么 / 能不能绕 |
|---|---|
| **产物未签名** | 见第 5 节。游戏本身没有技术限制，只是签名要你的证书 |
| 免费 Apple ID 装的应用 7 天过期 | Apple 的规则，和本项目无关 |
| 局域网 http 的模型地址连不上 | 只开了 `NSAllowsLocalNetworking`（回环 + `.local`）。要连 `http://192.168.x.x` 得改 `Info.plist` 加 `NSAllowsArbitraryLoads`；见该文件里的注释 |
| iPad 不能分屏 | 一个进程只有一份内核、一个端口，两个窗口会打架（`UIRequiresFullScreen`） |
| 内核 panic = App 闪退 | release 是 `panic = "abort"`（和安卓一致），没有"内核崩了重启一下"这条路 |
| 崩溃日志要连 Xcode 看 | 沙盒里的 `logs/server.log` 能在「文件」App 里看到；真正的崩栈只能连 Mac |
| 只出 arm64 | 和安卓一样：32 位设备跑不动这个规模 |
| 没做 iPad 键盘快捷键 | 桌面那套 `空格 = 推进一天 / 回车 = 推进到有事` 是键盘事件，iPad 接键盘应该直接能用，但没验过 |

---

## 8. 验到了什么程度

这一节原来写的是"手上只有 Command Line Tools，什么都验不了"。2026-11 装了
Xcode 16.4 + iOS 18.6 运行时之后，**下面这些是真跑过的**（都是可复现的命令）：

| 验的东西 | 怎么验的 | 结果 |
|---|---|---|
| Swift 能编 | `swiftc -typecheck -target arm64-apple-ios15.4`（5 个文件） | ✅ 零错误 |
| Swift + Rust 静态库能链 | 手工 `swiftc -c` + `clang` 链成可执行体 | ✅ 2.9 MB arm64，`_ironmind_start` 等符号在 |
| 真机包 | `tools/build-ios.sh` | ✅ `dist/IronMind-ios-1.3.0.ipa` 895 KB |
| `.app` 结构 | 解包看 | ✅ arm64 二进制 + `Assets.car` + `AppIcon60x60@2x.png` + `Info.plist` |
| **图标真的编进去了** | `actool` 跑通 | ✅（之前缺平台包时它报的是"没有模拟器运行时"） |
| **在模拟器里跑起来** | `--simulator` → 装 → 截屏 | ✅ 首页正常：引擎 1.3.0、模块卡、第 0 日 8.0 千 |
| 首次设置能写盘 | `POST /api/settings` → 看沙盒 | ✅ 写出 `Documents/settings.env` |
| 内核在 App 里监听 | 宿主机 `curl 127.0.0.1:18731/api/state` | ✅ 同一网络栈，直通 |
| 内嵌模块落盘 | 看 `Documents/modules/bundled.imod` | ✅ 1.7 KB，被真加载器读入 |
| 沙盒日志 | `Documents/logs/server.log` | ✅ 有请求记录 |
| 安全区 | 截图比对 | ❌→✅（改法见 4.7） |

**还没验的**：点进地图/电报台之后的界面（要有人点一下——`osascript` 发按键需要
辅助功能权限，这台机器没给）、键盘顶起（4.4）、双指缩放（4.5）、转屏重排、
`confirm()` 弹窗样式、真机。

## 9. 三端对照

| | 桌面 | 安卓 | iOS |
|---|---|---|---|
| 内核怎么跑 | `sim-server.exe` 子进程 | 同进程 `.so` | 同进程 `.a` |
| 窗口/WebView | tao + wry | tao + wry | **Swift + WKWebView** |
| 为什么不一样 | 崩了能单独重启内核 | Android 10+ 禁止 exec 私有二进制 | 键盘/对话框/安全区/缩放都得 UIKit 管 |
| 战役模块 | `modules/` 目录 | 打在 `.so` 里 | 打在 `.a` 里 |
| 数据目录 | exe 旁边 | `filesDir` | `Documents`（「文件」App 可见） |
| 出站 HTTP | `curl` | `curl` | **URLSession**（没有 curl） |
| 产物 | zip（未签名 exe） | 自签 APK | 未签名 .ipa |
| 屏幕方向 | 随意 | 锁横屏（WebView 不重排） | 竖横都行（WebKit 自己重排） |
| 键盘 | 不存在 | WebView 自己处理 | 自己抬底边（4.4） |
