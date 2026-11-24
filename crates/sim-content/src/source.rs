//! 模块的"内容从哪来"：一个目录，或者一个 `.imod` 压缩包。
//!
//! 为什么要有这一层：`.imod` 一开始是**文件夹后缀**（`nanxun_1938.imod/` 里面
//! 放四个 json）。那是把扩展名当目录标记用，带来三个实际麻烦：
//!
//! 1. 没法当一个文件下载/上传——发布时只能再套一层 zip，玩家拿到的是
//!    `xxx.imod.zip`，解压出来才是模块；
//! 2. 浏览器文件夹选择器（webkitdirectory）在不同浏览器里行为不一，
//!    "导入模块"变成"请在整个文件夹里点选正确的那一层"；
//! 3. 系统的文件关联、杀毒、网盘同步都把它当目录，行为不可预期。
//!
//! 所以现在 `.imod` 是一个**真 zip**（内含 module.json / map.json / oob.json /
//! scenario.json，可选 provinces_geo.json）。
//!
//! **目录形态继续永久支持**：作者开发时就是一堆 json 放在文件夹里改，
//! 那样才能 git diff、才能用 SDK 的热重载。压缩包只是"发布形态"。
//!
//! 安全性：zip 内的条目按**精确文件名**取，不做任何路径拼接，所以
//! 天然没有 zip-slip（`../../etc/passwd` 这种名字压根不会被查找）。
//! 同时限制单文件与总解压体积，防止一个几 KB 的压缩包炸出几十 GB。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::LoadError;

/// 单个成员的大小上限（图片/省界数据最大也就几 MB）。
const MAX_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
/// 整个模块解压后的总上限。
const MAX_TOTAL_BYTES: u64 = 128 * 1024 * 1024;

pub enum Source {
    /// 开发形态：一个文件夹。
    Dir(PathBuf),
    /// 发布形态：一个 `.imod` 压缩包（文件名 → 内容）。
    Zip {
        path: PathBuf,
        files: HashMap<String, Vec<u8>>,
    },
}

impl Source {
    /// 打开一个模块来源。目录就用目录，文件就当 zip 读。
    pub fn open(path: &Path) -> Result<Self, LoadError> {
        if path.is_dir() {
            return Ok(Source::Dir(path.to_path_buf()));
        }
        if !path.is_file() {
            return Err(LoadError::Io {
                path: path.to_path_buf(),
                message: "既不是目录也不是文件".into(),
            });
        }
        let bytes = std::fs::read(path).map_err(|e| LoadError::Io {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        let mut files: HashMap<String, Vec<u8>> = HashMap::new();
        let mut total: u64 = 0;
        let reader = std::io::Cursor::new(bytes);
        let mut zip = zip::ZipArchive::new(reader).map_err(|e| LoadError::Io {
            path: path.to_path_buf(),
            message: format!("不是有效的 .imod 压缩包：{e}"),
        })?;
        for i in 0..zip.len() {
            let mut f = zip.by_index(i).map_err(|e| LoadError::Io {
                path: path.to_path_buf(),
                message: format!("压缩包第 {i} 项读不出来：{e}"),
            })?;
            if f.is_dir() {
                continue;
            }
            // 只保留最后一段，避免 `sub/module.json` 这类打包方式读不到；
            // 同时把 Windows 的 `\` 统一成 `/`。
            let raw = f.name().replace('\\', "/");
            let name = raw.rsplit('/').next().unwrap_or("").to_string();
            if name.is_empty() {
                continue;
            }
            if f.size() > MAX_ENTRY_BYTES {
                return Err(LoadError::Io {
                    path: path.to_path_buf(),
                    message: format!("{name} 太大（{} 字节）", f.size()),
                });
            }
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut f, &mut buf).map_err(|e| LoadError::Io {
                path: path.to_path_buf(),
                message: format!("{name} 解压失败：{e}"),
            })?;
            total += buf.len() as u64;
            if total > MAX_TOTAL_BYTES {
                return Err(LoadError::Io {
                    path: path.to_path_buf(),
                    message: "解压后超过 128 MB，拒绝加载".into(),
                });
            }
            files.insert(name, buf);
        }
        Ok(Source::Zip {
            path: path.to_path_buf(),
            files,
        })
    }

    /// 给人看的名字（错误信息里用）。
    pub fn display(&self) -> PathBuf {
        match self {
            Source::Dir(p) => p.clone(),
            Source::Zip { path, .. } => path.clone(),
        }
    }

    /// 这个成员在不在（错误信息里用 `.join()` 拼个像样的路径）。
    fn where_is(&self, name: &str) -> PathBuf {
        match self {
            Source::Dir(p) => p.join(name),
            Source::Zip { path, .. } => path.join(name),
        }
    }

    /// 读一个必须存在的文本成员。
    pub fn read_text(&self, name: &str) -> Result<String, LoadError> {
        match self {
            Source::Dir(dir) => std::fs::read_to_string(dir.join(name)).map_err(|e| LoadError::Io {
                path: dir.join(name),
                message: e.to_string(),
            }),
            Source::Zip { path, files } => match files.get(name) {
                Some(b) => String::from_utf8(b.clone()).map_err(|e| LoadError::Io {
                    path: path.join(name),
                    message: format!("不是 UTF-8 文本：{e}"),
                }),
                None => Err(LoadError::Io {
                    path: path.join(name),
                    message: "压缩包里没有这个文件".into(),
                }),
            },
        }
    }

    /// 读一个可选成员（省界数据）。
    pub fn read_optional(&self, name: &str) -> Option<String> {
        self.read_text(name).ok()
    }

    /// 错误信息里显示的路径。
    pub fn path_of(&self, name: &str) -> PathBuf {
        self.where_is(name)
    }
}
