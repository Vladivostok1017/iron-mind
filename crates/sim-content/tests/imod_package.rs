//! `.imod` 是压缩包，不是"带扩展名的文件夹"。
//!
//! 这一组测试守两件事：
//! 1. 两种形态（目录 / 压缩包）读出来的东西**必须完全一样**——否则作者
//!    "开发时好好的，打包发布就不对"；
//! 2. 坏包要**明确报错**，不能静默当成空模块（那会让玩家以为"下载坏了"）。

use std::io::Write;
use std::path::{Path, PathBuf};

use sim_content::load_module;

fn demo_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../content/scenarios/demo_1805.imod")
        .canonicalize()
        .expect("找不到引擎自带的 demo 模块")
}

fn temp_imod(tag: &str) -> PathBuf {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("imod-test-{tag}-{n}.imod"))
}

/// 把一个模块目录打成 `.imod`（和 tools/package.ps1 做的事一样）。
fn pack(dir: &Path, out: &Path) {
    let f = std::fs::File::create(out).expect("建 zip 失败");
    let mut zw = zip::ZipWriter::new(f);
    let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for entry in std::fs::read_dir(dir).expect("读模块目录失败") {
        let p = entry.expect("目录项").path();
        if !p.is_file() {
            continue;
        }
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        zw.start_file(name, opts).expect("写 zip 条目");
        zw.write_all(&std::fs::read(&p).expect("读文件")).expect("写内容");
    }
    zw.finish().expect("收尾");
}

#[test]
fn package_loads_the_same_as_directory() {
    let dir = demo_dir();
    let out = temp_imod("same");
    pack(&dir, &out);

    let from_dir = load_module(&dir).expect("目录形态应该能加载");
    let from_zip = load_module(&out).expect("压缩包形态应该能加载");

    assert_eq!(from_dir.meta.id, from_zip.meta.id);
    assert_eq!(from_dir.meta.name, from_zip.meta.name);
    assert_eq!(from_dir.map.provinces.len(), from_zip.map.provinces.len());
    assert_eq!(from_dir.oob.units.len(), from_zip.oob.units.len());
    assert_eq!(from_dir.scenario.days, from_zip.scenario.days);
    // 建出来的世界也必须一致（种子、省份、单位都是同一套输入）。
    let wd = sim_content::build_world(&from_dir);
    let wz = sim_content::build_world(&from_zip);
    assert_eq!(wd.state_hash(), wz.state_hash(), "两种形态必须逐位一致");

    let _ = std::fs::remove_file(&out);
}

#[test]
fn peek_reads_meta_from_package() {
    let dir = demo_dir();
    let out = temp_imod("peek");
    pack(&dir, &out);
    let meta = sim_content::peek_meta_json(&out).expect("应该能只读 module.json");
    assert_eq!(meta.get("name").and_then(|v| v.as_str()), Some("演示 · 1805 前电报时代"));
    let _ = std::fs::remove_file(&out);
}

#[test]
fn garbage_is_rejected_loudly() {
    let out = temp_imod("garbage");
    std::fs::write(&out, b"this is not a zip at all").unwrap();
    let err = load_module(&out).err().expect("坏包必须报错，不能静默");
    let text = format!("{err}");
    assert!(
        text.contains("压缩包") || text.contains("imod"),
        "错误信息要说清是包坏了，实测：{text}"
    );
    let _ = std::fs::remove_file(&out);
}

#[test]
fn package_missing_a_file_names_it() {
    // 只放 module.json：缺 map.json 时要指出**缺哪个**，
    // 否则作者只会看到"加载失败"然后去猜。
    let out = temp_imod("partial");
    let f = std::fs::File::create(&out).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
    zw.start_file("module.json", opts).unwrap();
    zw.write_all(br#"{"id":"a.b","name":"x","version":"0.1.0"}"#).unwrap();
    zw.finish().unwrap();

    let err = load_module(&out).err().expect("缺文件必须报错");
    let text = format!("{err}");
    assert!(text.contains("scenario.json") || text.contains("map.json"), "实测：{text}");
    let _ = std::fs::remove_file(&out);
}
