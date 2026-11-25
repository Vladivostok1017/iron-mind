//! 结局判定。
//!
//! **不做"胜负"这一栏。** 一场战役的结束不该是一个字：守住/没守住。
//! 淞沪打完，中国丢了上海，但"以空间换时间"的战略实现了——这算赢还是输？
//!
//! 所以这里只做一件事：**在战役结束时，按一组可读的指标挑一条结局**。
//! 句式由引擎提供（指标 / 比较 / 优先级），内容决定"什么算苦撑"。
//! 引擎不认识"武汉""南浔线"，也不认识"天炉"——那是模块的事。
//!
//! 判定顺序按 `priority` 从小到大；第一个全部子句满足的就是它。

use crate::world::{Side, World};

/// 可以拿来判定的指标。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Metric {
    /// 已过去的战役天数
    Day,
    /// 控制省份数
    Provinces,
    /// 总兵力（千人）
    Strength,
    /// 累计伤亡（千人）
    Losses,
    /// 还在编成内的部队数
    UnitsAlive,
    /// 阵亡（含殉国）的将领数
    CommandersLost,
    /// 控制某个指定省（省名由 `Clause::province` 给出）：1 = 控制，0 = 不控制
    HoldsProvince,
}

impl Metric {
    pub fn code(self) -> u32 {
        match self {
            Metric::Day => 0,
            Metric::Provinces => 1,
            Metric::Strength => 2,
            Metric::Losses => 3,
            Metric::UnitsAlive => 4,
            Metric::CommandersLost => 5,
            Metric::HoldsProvince => 6,
        }
    }
}

/// 比较方式。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cmp {
    Ge,
    Gt,
    Le,
    Lt,
}

/// 一条子句：`指标 [某方] 比较 阈值`。
#[derive(Clone, Debug)]
pub struct Clause {
    pub metric: Metric,
    /// 指标属于哪一方。`None` = 不区分（目前只有 `Day` 这样用）。
    pub side: Option<Side>,
    pub cmp: Cmp,
    /// 阈值。`HoldsProvince` 用省名，其余用数值。
    pub value: f64,
    pub province: Option<String>,
}

impl Clause {
    fn holds(&self, w: &World) -> bool {
        let actual = match self.metric {
            Metric::Day => w.tick as f64,
            Metric::Provinces => {
                let s = self.side.unwrap_or(Side::A);
                w.provinces.iter().filter(|p| p.owner == s).count() as f64
            }
            Metric::Strength => {
                let s = self.side.unwrap_or(Side::A);
                w.total_strength(s).to_display_f64()
            }
            Metric::Losses => {
                let s = self.side.unwrap_or(Side::A);
                w.units
                    .iter()
                    .filter(|u| u.side == s)
                    .map(|u| u.losses.to_display_f64())
                    .sum()
            }
            Metric::UnitsAlive => {
                let s = self.side.unwrap_or(Side::A);
                w.units
                    .iter()
                    .filter(|u| u.side == s && u.in_being())
                    .count() as f64
            }
            Metric::CommandersLost => {
                let s = self.side.unwrap_or(Side::A);
                w.units
                    .iter()
                    .filter(|u| u.side == s && u.commander.fate == crate::world::Fate::Killed)
                    .count() as f64
            }
            Metric::HoldsProvince => {
                let s = self.side.unwrap_or(Side::A);
                let name = self.province.as_deref().unwrap_or("");
                let held = w
                    .provinces
                    .iter()
                    .any(|p| p.name == name && p.owner == s);
                if held {
                    1.0
                } else {
                    0.0
                }
            }
        };
        match self.cmp {
            Cmp::Ge => actual >= self.value,
            Cmp::Gt => actual > self.value,
            Cmp::Le => actual <= self.value,
            Cmp::Lt => actual < self.value,
        }
    }
}

/// 一条结局。
#[derive(Clone, Debug)]
pub struct Ending {
    pub id: String,
    pub name: String,
    pub text: String,
    /// 越小越先判。
    pub priority: u32,
    /// true = 只在战役期满时判；false = 每天都可以提前触发（"崩盘""速溃"）。
    pub at_end_only: bool,
    pub clauses: Vec<Clause>,
}

/// 判定结果。
#[derive(Clone, Debug)]
pub struct Outcome {
    pub id: String,
    pub name: String,
    pub text: String,
    pub day: u32,
    /// 关键数字，给玩家对账用：(标签, 值)
    pub metrics: Vec<(String, f64)>,
}

/// 现在该判出哪个结局？
///
/// 返回 `None` = 还没到该结束的时候。
pub fn evaluate(w: &World) -> Option<Outcome> {
    let finished = w.tick >= w.campaign_days;
    let mut rules: Vec<&Ending> = w.endings.iter().collect();
    rules.sort_by_key(|e| e.priority);

    for e in &rules {
        if e.at_end_only && !finished {
            continue;
        }
        if e.clauses.iter().all(|c| c.holds(w)) {
            return Some(Outcome {
                id: e.id.clone(),
                name: e.name.clone(),
                text: e.text.clone(),
                day: w.tick,
                metrics: key_metrics(w),
            });
        }
    }
    if finished {
        // 模块没写匹配的结局：也要给一个，别让战役静静地卡在最后一天
        return Some(Outcome {
            id: "E0".into(),
            name: "未分胜负".into(),
            text: "战役期满，双方都没能达成决定性的结果。".into(),
            day: w.tick,
            metrics: key_metrics(w),
        });
    }
    None
}

/// 给玩家看的几个数：这些是"为什么是这个结局"的依据。
///
/// 标签里**不写任何国名/阵营名**——引擎不知道这一仗是谁在打。
/// 用位置记号 `A方`/`B方`（= 模块 `sides` 里的第 1、2 个），
/// 由界面在显示前换成模块给这一方起的名字。
fn key_metrics(w: &World) -> Vec<(String, f64)> {
    let a_prov = w.provinces.iter().filter(|p| p.owner == Side::A).count();
    let b_prov = w.provinces.iter().filter(|p| p.owner == Side::B).count();
    let a_loss: f64 = w
        .units
        .iter()
        .filter(|u| u.side == Side::A)
        .map(|u| u.losses.to_display_f64())
        .sum();
    let b_loss: f64 = w
        .units
        .iter()
        .filter(|u| u.side == Side::B)
        .map(|u| u.losses.to_display_f64())
        .sum();
    let a_alive = w.units.iter().filter(|u| u.side == Side::A && u.in_being()).count();
    let b_alive = w.units.iter().filter(|u| u.side == Side::B && u.in_being()).count();
    vec![
        ("天数".into(), w.tick as f64),
        (format!("{SIDE_A_LABEL}控制省份"), a_prov as f64),
        (format!("{SIDE_B_LABEL}控制省份"), b_prov as f64),
        (format!("{SIDE_A_LABEL}兵力（千）"), w.total_strength(Side::A).to_display_f64()),
        (format!("{SIDE_B_LABEL}兵力（千）"), w.total_strength(Side::B).to_display_f64()),
        (format!("{SIDE_A_LABEL}伤亡（千）"), a_loss),
        (format!("{SIDE_B_LABEL}伤亡（千）"), b_loss),
        (format!("{SIDE_A_LABEL}尚存番号"), a_alive as f64),
        (format!("{SIDE_B_LABEL}尚存番号"), b_alive as f64),
    ]
}

/// 结局指标里的位置记号。界面拿它去换模块声明的阵营名。
pub const SIDE_A_LABEL: &str = "A方";
pub const SIDE_B_LABEL: &str = "B方";
