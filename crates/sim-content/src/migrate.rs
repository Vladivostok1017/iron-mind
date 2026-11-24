//! 模块格式的**迁移承诺**。
//!
//! 之前这里只有一句口头约定："只增不改"。那不算承诺——靠人守纪律，
//! 一个人忘了就静默坏掉：字段改名之后，旧模块的旧名字对不上，
//! serde 用默认值顶上，**不报错**，作者的地图悄悄变了个样。
//!
//! 现在给的是代码级的承诺，由三件事组成：
//!
//! 1. **每个版本一步迁移**。`content_schema: N` 的模块加载时，
//!    依次跑 `N → N+1 → … → CURRENT` 的迁移函数，把旧结构改成新结构。
//!    作者什么都不用改。
//! 2. **缺迁移器就拒绝加载**，并明确说缺哪一步。
//!    宁可启动失败，也不要跑一个"看起来能跑、其实是默认值顶上去"的模块。
//! 3. **比引擎新的 schema 直接拒绝**，并说清楚要升级引擎还是降 schema。
//!
//! 加一次迁移的做法（改 `model.rs` 之后）：
//!
//! ```ignore
//! // 1. CURRENT_CONTENT_SCHEMA 加一
//! // 2. 在 steps() 里加一个分支：
//! 1 => Some(|r| {
//!     // 把 map.json 里的旧字段搬到新字段
//!     if let Some(arr) = r.map.get_mut("provinces").and_then(|p| p.as_array_mut()) {
//!         for p in arr {
//!             if let Some(old) = p.get("supply_throughput").cloned() {
//!                 p["throughput"] = old;
//!             }
//!         }
//!     }
//!     Ok(())
//! }),
//! // 3. 在 migrate_tests 里加一条：用旧结构的 JSON 断言迁移后能反序列化
//! ```
//!
//! 迁移只改**结构**，不改语义。凡是无法自动迁移的（比如某个字段的含义变了），
//! 迁移器应当返回 `Err`，让作者看到明确的改法——绝不猜。

use serde_json::Value;

/// 引擎能读的最低 schema。降到这个数以下就没有迁移路径了。
pub const MIN_SUPPORTED_SCHEMA: u32 = 1;

/// 一个模块的四个原始文件（还没反序列化）。
///
/// 迁移函数拿到的是**可变引用**：字段改名往往要跨文件搬数据
/// （比如 module.json 里的敌方统帅配置挪进 scenario.json）。
pub struct RawModule<'a> {
    pub module: &'a mut Value,
    pub scenario: &'a mut Value,
    pub map: &'a mut Value,
    pub oob: &'a mut Value,
}

pub type Step = fn(&mut RawModule<'_>) -> Result<(), String>;

/// 从 `from` 升到 `from + 1` 的迁移器。**没有就是拒绝加载。**
fn step_for(from: u32) -> Option<Step> {
    match from {
        // 目前 CURRENT_CONTENT_SCHEMA = 1，还没有任何一步。
        // 加格式变更时，在这里加分支，并把 CURRENT_CONTENT_SCHEMA 加一。
        _ => None,
    }
}

/// 读一个模块声明的 schema 版本。写在 `module.json` 的 `content_schema`，
/// 省略按 1 处理（这是"最早的那个格式"）。
pub fn declared_schema(module: &Value) -> u32 {
    module
        .get("content_schema")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .unwrap_or(MIN_SUPPORTED_SCHEMA)
}

/// 把模块升到当前 schema。返回实际跑过的迁移步（空 = 本来就是最新）。
pub fn ensure_schema(raw: &mut RawModule<'_>, current: u32) -> Result<Vec<u32>, String> {
    let declared = declared_schema(raw.module);
    if declared > current {
        return Err(format!(
            "这个模块用的是更新的模块格式（schema {declared}），当前引擎只支持到 {current}。\
             要么升级引擎，要么把 module.json 的 content_schema 改小（若你确认它能跑）。"
        ));
    }
    if declared < MIN_SUPPORTED_SCHEMA {
        return Err(format!(
            "schema {declared} 太旧，引擎已经不再支持（最低 {MIN_SUPPORTED_SCHEMA}）。"
        ));
    }
    let mut ran = Vec::new();
    for from in declared..current {
        let Some(step) = step_for(from) else {
            return Err(format!(
                "缺少模块格式的迁移器：schema {from} → {}。\
                 这是引擎的 bug（格式升级了但没补迁移），请报 issue，不要改模块去迁就。",
                from + 1
            ));
        };
        step(raw).map_err(|e| format!("schema {from} → {} 迁移失败：{e}", from + 1))?;
        ran.push(from);
    }
    // 迁移完把声明版本改写成当前，后续步骤与日志都以它为准
    if let Some(obj) = raw.module.as_object_mut() {
        obj.insert("content_schema".into(), Value::from(current));
    }
    Ok(ran)
}

#[cfg(test)]
mod migrate_tests {
    use super::*;

    fn raw(module: Value) -> (Value, Value, Value) {
        (module, Value::Null, Value::Null)
    }

    #[test]
    fn newer_schema_is_refused_with_a_clear_message() {
        let (mut m, mut s, mut p) = raw(serde_json::json!({ "content_schema": 99 }));
        let mut o = Value::Null;
        let mut r = RawModule { module: &mut m, scenario: &mut s, map: &mut p, oob: &mut o };
        let err = ensure_schema(&mut r, 1).unwrap_err();
        assert!(err.contains("schema 99"), "错误里要写清声明的版本：{err}");
        assert!(err.contains("升级引擎"), "还要告诉作者怎么办：{err}");
    }

    #[test]
    fn current_schema_runs_no_steps() {
        let (mut m, mut s, mut p) = raw(serde_json::json!({ "content_schema": 1 }));
        let mut o = Value::Null;
        let mut r = RawModule { module: &mut m, scenario: &mut s, map: &mut p, oob: &mut o };
        assert_eq!(ensure_schema(&mut r, 1).unwrap(), Vec::<u32>::new());
    }

    #[test]
    fn missing_declaration_means_schema_one() {
        // 老模块没写 content_schema：按最早格式算，不是报错
        let (mut m, mut s, mut p) = raw(serde_json::json!({ "id": "x" }));
        let mut o = Value::Null;
        let mut r = RawModule { module: &mut m, scenario: &mut s, map: &mut p, oob: &mut o };
        assert_eq!(ensure_schema(&mut r, 1).unwrap(), Vec::<u32>::new());
    }

    #[test]
    fn missing_migrator_is_an_engine_bug_not_a_silent_default() {
        // 假装引擎升到了 2，但没有写 1→2 的迁移器：必须明确拒绝
        let (mut m, mut s, mut p) = raw(serde_json::json!({ "content_schema": 1 }));
        let mut o = Value::Null;
        let mut r = RawModule { module: &mut m, scenario: &mut s, map: &mut p, oob: &mut o };
        let err = ensure_schema(&mut r, 2).unwrap_err();
        assert!(err.contains("缺少模块格式的迁移器"), "{err}");
        assert!(err.contains("1 → 2"), "要点名缺哪一步：{err}");
    }
}
