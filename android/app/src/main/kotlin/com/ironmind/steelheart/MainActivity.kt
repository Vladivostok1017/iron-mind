package com.ironmind.steelheart

/**
 * 唯一的 Activity。
 *
 * 它**什么都不做**是有意的：窗口、WebView、其生命周期全由 Rust 那边（tao + wry）
 * 驱动，这个类只是 Android 要求存在的那个"入口"。真正的胶水在 wry 生成的文件里
 * （WryActivity.kt / Rust.kt / RustWebView.kt …）。
 *
 * 所以在这里加 Kotlin 业务代码是走错方向了——要改行为就改 Rust。
 */
class MainActivity : WryActivity()
