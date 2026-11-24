//! 加载与校验的错误类型。
//!
//! 不用 anyhow/thiserror：这个 crate 的错误集合很小，
//! 手写能避免依赖，也让错误信息完全可控。

use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Severity::Error => write!(f, "错误"),
            Severity::Warning => write!(f, "警告"),
        }
    }
}

/// 一条校验问题。
#[derive(Debug, Clone)]
pub struct Issue {
    pub severity: Severity,
    /// 稳定的机器可读代号，便于测试与文档引用。
    pub code: &'static str,
    pub message: String,
}

impl Issue {
    pub fn error(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code,
            message: message.into(),
        }
    }

    pub fn warn(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            code,
            message: message.into(),
        }
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}: {}", self.severity, self.code, self.message)
    }
}

/// 加载失败。
#[derive(Debug)]
pub enum LoadError {
    Io {
        path: PathBuf,
        message: String,
    },
    Parse {
        path: PathBuf,
        message: String,
    },
    /// 校验未通过。至少含一条 Error 级问题。
    Validation {
        module: String,
        issues: Vec<Issue>,
    },
    /// 模块格式迁移失败：版本比引擎新、缺迁移器、或迁移器明确拒绝。
    ///
    /// 单独一类而不是塞进 Parse：作者需要知道"这不是我写错了，是格式变了"。
    Migration { module: String, message: String },
}

impl LoadError {
    pub fn issues(&self) -> &[Issue] {
        match self {
            LoadError::Validation { issues, .. } => issues,
            _ => &[],
        }
    }
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Migration { module, message } => {
                write!(f, "模块 {module} 格式不兼容：{message}")
            }
            LoadError::Io { path, message } => {
                write!(f, "读取失败 {}: {}", path.display(), message)
            }
            LoadError::Parse { path, message } => {
                write!(f, "解析失败 {}: {}", path.display(), message)
            }
            LoadError::Validation { module, issues } => {
                writeln!(f, "模块 {} 校验未通过：", module)?;
                for i in issues.iter().filter(|i| i.is_error()) {
                    writeln!(f, "  {}", i)?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for LoadError {}
