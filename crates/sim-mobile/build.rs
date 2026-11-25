//! 把一场战役模块打进 `.so`。
//!
//! 打包逻辑（遍历顺序、扩展名过滤、目录/文件两种形态、默认值）在
//! `sim-bundle` 里，iOS 壳用的是同一份——两个平台必须对同一个模块目录
//! 打出字节一致的包。
//!
//! 为什么不用 Android assets：assets 要通过 Java 的 AssetManager 读，多一层 JNI
//! 胶水；而模块本来就是一个 **zip**（`.imod`），直接把字节 `include_bytes!` 进来，
//! 启动时往 app 私有目录写一份就完事——手机上不需要任何外部文件。
//!
//! 选哪个模块由环境变量 `IRONMIND_BUNDLE_MODULE` 决定（一个模块**目录**）：
//!
//! ```powershell
//! $env:IRONMIND_BUNDLE_MODULE = "F:\dsh\nanxun-1938\nanxun_1938.imod"
//! cargo build -p sim-mobile --target aarch64-linux-android --release
//! ```
//!
//! 不设就用引擎自带的演示模块，保证任何情况下都编得出一个能玩的 APK。

fn main() {
    sim_bundle::embed_module();
}
