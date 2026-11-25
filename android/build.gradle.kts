// 安卓壳的 Gradle 根工程。
//
// 这里**没有一行游戏逻辑**：地图、模拟、将领全在 Rust 里（`crates/sim-mobile`
// 编出 `libsim_mobile.so`）。Gradle 的职责只有三件：
// 1. 把 .so 打进 APK（`app/src/main/jniLibs/arm64-v8a/`）；
// 2. 把 wry 生成的 Kotlin 胶水（WryActivity / RustWebView …）编进去；
// 3. 签名出一个能装的 APK。
plugins {
    id("com.android.application") version "8.7.3" apply false
    id("org.jetbrains.kotlin.android") version "2.1.0" apply false
}
