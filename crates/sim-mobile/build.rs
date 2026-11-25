//! 把一场战役模块打进 `.so`。
//!
//! 为什么不用 Android assets：asssets 要通过 Java 的 AssetManager 读，多一层 JNI
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

use std::io::Write;
use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let repo = manifest.parent().unwrap().parent().unwrap().to_path_buf();
    let src = std::env::var("IRONMIND_BUNDLE_MODULE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| repo.join("content/scenarios/demo_1805.imod"));

    // 模块内容变了要重新打包。目录形态就盯 module.json 的时间戳 + 目录本身。
    println!("cargo:rerun-if-env-changed=IRONMIND_BUNDLE_MODULE");
    println!("cargo:rerun-if-changed={}", src.display());
    if src.is_dir() {
        for e in walk(&src) {
            println!("cargo:rerun-if-changed={}", e.display());
        }
    }

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let out = out_dir.join("bundled.imod");

    // 已经是 zip 就直接拷：模块仓库里两种形态都有（目录 / 打包好的 .imod）。
    if src.is_file() {
        std::fs::copy(&src, &out).expect("拷贝模块包失败");
        println!("cargo:warning=内嵌模块：{}（原样拷贝）", src.display());
    } else if src.is_dir() {
        pack_dir(&src, &out).expect("打包模块目录失败");
        println!("cargo:warning=内嵌模块：{}（目录 → zip）", src.display());
    } else {
        panic!("找不到要内嵌的模块：{}", src.display());
    }

    // 让 lib.rs 能 include_bytes!(env!("SIM_BUNDLED_MODULE"))
    println!("cargo:rustc-env=SIM_BUNDLED_MODULE={}", out.display());
}

fn pack_dir(dir: &Path, out: &Path) -> zip::result::ZipResult<()> {
    let file = std::fs::File::create(out)?;
    let mut zw = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for path in walk(dir) {
        let rel = path.strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/");
        // 只收模块用得上的文本与几何数据；README 之类不必进包（省体积也无害）。
        let ext = rel.rsplit('.').next().unwrap_or("").to_lowercase();
        if !matches!(ext.as_str(), "json" | "md" | "txt") {
            continue;
        }
        zw.start_file(rel, opts)?;
        let mut f = std::fs::File::open(&path)?;
        std::io::copy(&mut f, &mut zw)?;
        zw.flush().ok();
    }
    zw.finish()?;
    Ok(())
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.is_file() {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}
