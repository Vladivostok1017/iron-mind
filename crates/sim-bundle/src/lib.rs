//! 外壳的 `build.rs` 公共部分：把一场战役模块打成 `.imod`（一个 zip），编进壳里。
//!
//! 为什么单独一个 crate：安卓壳（`sim-mobile`）和 iOS 壳（`sim-ios`）要做
//! **完全一样**的一件事——同一份遍历顺序、同一份扩展名过滤、同一份默认值。
//! 各抄一份的后果不是编不过，而是两个平台对同一个模块目录打出来的包可能不一样；
//! 出问题时还得先怀疑"是不是两边打的包不同"。放在一处就没有这个可能。
//!
//! 用法（壳 crate 的 `build.rs` 全文就一行）：
//!
//! ```ignore
//! fn main() {
//!     sim_bundle::embed_module();
//! }
//! ```
//!
//! 之后同一个 crate 的 `lib.rs` 就能：
//!
//! ```ignore
//! const BUNDLED: &[u8] = include_bytes!(env!("SIM_BUNDLED_MODULE"));
//! ```
//!
//! 选哪个模块由环境变量 `IRONMIND_BUNDLE_MODULE` 决定（一个模块**目录**，或者
//! 一个已经打好的 `.imod` 文件）；不设就用引擎自带的演示模块，保证任何情况下
//! 都编得出一个能玩的包。
//!
//! 为什么不走"把模块放进 app 资源包"：模块本来就是一个 zip，`include_bytes!`
//! 进来再在启动时写一份到沙盒，省掉一层"资源清单 / AssetManager / Bundle 读取"
//! 的胶水，也省掉 Xcode 工程里一个必须手工维护的 Copy Files 阶段。
//! 代价是换模块要重编壳——对玩家无所谓（模块下载到沙盒再导入是另一条路），
//! 对作者也一样（作者用 SDK 的开发循环，不重编壳）。

use std::io::Write;
use std::path::{Path, PathBuf};

/// 编好之后 `include_bytes!(env!("SIM_BUNDLED_MODULE"))` 就指向这个文件。
pub const ENV_NAME: &str = "SIM_BUNDLED_MODULE";

/// 打包内嵌模块，并把结果路径写进 `SIM_BUNDLED_MODULE`。
///
/// 在 `build.rs` 里调用；它会自己打印 `cargo:rerun-if-*`，不需要调用方再管。
pub fn embed_module() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let repo = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("壳 crate 应当在 <仓库>/crates/<名字> 下面")
        .to_path_buf();

    let src = std::env::var("IRONMIND_BUNDLE_MODULE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| repo.join("content/scenarios/demo_1805.imod"));

    // 模块内容变了要重新打包。目录形态就盯每个文件的时间戳。
    println!("cargo:rerun-if-env-changed=IRONMIND_BUNDLE_MODULE");
    println!("cargo:rerun-if-changed={}", src.display());
    if src.is_dir() {
        for e in walk(&src) {
            println!("cargo:rerun-if-changed={}", e.display());
        }
    }

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
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

    println!("cargo:rustc-env={ENV_NAME}={}", out.display());
}

/// 把一个模块目录压成 `.imod`。
///
/// 只收模块用得上的文本与几何数据；README 之类不必进包（省体积也无害）。
pub fn pack_dir(dir: &Path, out: &Path) -> zip::result::ZipResult<()> {
    let file = std::fs::File::create(out)?;
    let mut zw = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for path in walk(dir) {
        let rel = path
            .strip_prefix(dir)
            .expect("walk 只返回 dir 下的路径")
            .to_string_lossy()
            .replace('\\', "/");
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

/// 递归列出文件（顺序排好，保证两个平台打出来的包字节一致）。
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_a_module_dir_into_a_zip() {
        let tmp = std::env::temp_dir().join(format!("sim-bundle-{}", std::process::id()));
        let mods = tmp.join("m.imod");
        std::fs::create_dir_all(&mods).unwrap();
        std::fs::write(mods.join("module.json"), "{\"id\":\"org.t\"}").unwrap();
        std::fs::write(mods.join("map.json"), "{}").unwrap();
        // 不该进包的东西
        std::fs::write(mods.join("notes.bin"), "x").unwrap();

        let out = tmp.join("bundled.imod");
        pack_dir(&mods, &out).unwrap();

        let bytes = std::fs::read(&out).unwrap();
        assert_eq!(&bytes[..2], b"PK", "产出的必须是一个真 zip");
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let names: Vec<String> = (0..z.len()).map(|i| z.by_index(i).unwrap().name().to_string()).collect();
        assert!(names.contains(&"module.json".to_string()));
        assert!(names.contains(&"map.json".to_string()));
        assert!(!names.contains(&"notes.bin".to_string()));

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
