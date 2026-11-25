//! 内核不许认识任何一场具体的战争。
//!
//! 这条测试是"引擎/内容分层"的守门人。踩过的坑：多阵营支持第一版把
//! `Some("cn") => Side::A, Some("jp") => Side::B` 直接写在 HTTP 处理里，
//! 于是 `Side` 之外又多了一份"谁是国军谁是日军"的常识——引擎从此只知道
//! 一种战争。那不是框架，那是把一场战役钉进了骨架。
//!
//! 分层规则：
//! - **引擎**（sim-core / sim-content / sim-server / sim-desktop）：只认 A/B、
//!   只认"模块声明了什么"。可以在注释里提例子，不许在**代码字符串**里出现国名、
//!   阵营名，也不许拿 `"cn"`/`"jp"` 当逻辑判据。
//! - **内容**（content/scenarios 与外部模块仓库）：想叫什么叫什么。

use std::path::{Path, PathBuf};

/// 引擎里不许当逻辑用的 id（老模块的写法，只能出现在默认值/注释里）。
///
/// 允许出现的位置只有一处：`sim_content::side_ids` 的默认值——那是为了
/// 让没写 `sides` 的老模块还能加载，属于向后兼容的唯一入口。
const FORBIDDEN_IN_LOGIC: &[&str] = &[
    "国军", "日军", "共军", "八路军", "新四军", "解放军",
    "国民党", "日本军", "第九战区", "第11军",
];

fn engine_src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// 把注释剥掉：注释里举例是可以的（"谁是国军"正是最好的反例说明），
/// 会把整段 `//` 行与 `/* */` 去掉再看剩下的代码。
fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut in_block = false;
    for line in src.lines() {
        let t = line.trim_start();
        if in_block {
            if t.contains("*/") {
                in_block = false;
            }
            out.push('\n');
            continue;
        }
        if t.starts_with("/*") {
            if !t.contains("*/") {
                in_block = true;
            }
            out.push('\n');
            continue;
        }
        if t.starts_with("//") {
            out.push('\n');
            continue;
        }
        // 行尾注释：只砍 `//` 之后的部分（URL 里的 `//` 用 "//" 前面有 ':' 判掉）
        if let Some(p) = line.find("// ") {
            let before = &line[..p];
            if !before.contains(':') || before.contains('"') {
                out.push_str(before);
                out.push('\n');
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().and_then(|s| s.to_str()) == Some("rs") {
            out.push(p);
        }
    }
}

#[test]
fn kernel_source_mentions_no_particular_war() {
    let root = engine_src();
    let mut files = Vec::new();
    for c in ["sim-core", "sim-content", "sim-server", "sim-desktop"] {
        rust_files(&root.join("crates").join(c).join("src"), &mut files);
    }
    assert!(!files.is_empty(), "没找到引擎源码，路径算错了");

    let mut hits = Vec::new();
    for f in &files {
        let Ok(raw) = std::fs::read_to_string(f) else { continue };
        let code = strip_comments(&raw);
        for w in FORBIDDEN_IN_LOGIC {
            if code.contains(w) {
                hits.push(format!("{}: {w}", f.display()));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "引擎代码里出现了具体战争的名字：\n{}\n\n\
         这些东西属于模块（content/ 与外部模块仓库）。引擎只知道 A/B 两方，\
         谁是国军、谁是日军由模块的 `sides` 声明。",
        hits.join("\n")
    );
}

/// `"cn"`/`"jp"` 这类 id 只许出现在兼容默认值里，不许当 `match`/`if` 的判据。
#[test]
fn side_ids_are_not_matched_in_engine_logic() {
    let root = engine_src();
    let mut files = Vec::new();
    for c in ["sim-core", "sim-server"] {
        rust_files(&root.join("crates").join(c).join("src"), &mut files);
    }
    let mut hits = Vec::new();
    for f in &files {
        let Ok(raw) = std::fs::read_to_string(f) else { continue };
        let code = strip_comments(&raw);
        for pat in [
            r#"Some("cn")"#,
            r#"Some("jp")"#,
            r#""cn" =>"#,
            r#""jp" =>"#,
            r#"== "cn""#,
            r#"== "jp""#,
            r#"!= "cn""#,
            r#"!= "jp""#,
        ] {
            if code.contains(pat) {
                hits.push(format!("{}: {pat}", f.display()));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "引擎把某一方的 id 当成了判据：\n{}\n\n\
         阵营是内容：拿 `side_by_id`/`side_id_of`/`side_name` 走模块声明，\
         不要写死某个 id。",
        hits.join("\n")
    );
}
