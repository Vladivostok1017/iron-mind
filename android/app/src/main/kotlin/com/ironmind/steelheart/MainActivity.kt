package com.ironmind.steelheart

/**
 * 唯一的 Activity。
 *
 * 窗口、WebView、其生命周期全由 Rust 那边（tao + wry）驱动，这个类只是 Android
 * 要求存在的那个"入口"。写业务逻辑是走错方向了——要改游戏就改 Rust / 网页。
 *
 * 关于方向：清单里声明的是 `android:screenOrientation="landscape"`。
 * **启动即横屏**，不是"竖屏启动再转过去"。这点很关键：
 *
 * 外壳为了不重启内核，声明了 `configChanges=orientation|screenSize`（转屏不重建
 * Activity）；可这版 tao/wry 不会在配置变化时通知 WebView。结果是"运行中旋转"
 * 之后页面仍按竖屏的 393 CSS px 排版，只占屏幕左边一条。
 *
 * 而 Android WebView 的 `device-width` 是**页面加载那一刻**定的，之后改 View 尺寸、
 * 甚至自己 measure/layout 都不会重新求值（试过，没用）。
 *
 * 所以干脆不让它转：一开始就横屏，WebView 第一次布局就是 873×393，一切正常。
 * 用户把手机横过来看就是了——横屏手游都是这么做的。
 */
class MainActivity : WryActivity()
