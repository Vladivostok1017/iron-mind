//! 模块加载与构建。
//!
//! 一个模块是一个目录（或打包后的 `.imod`），至少包含四个文件：
//!
//! ```text
//! module.json     元数据、引擎版本约束
//! scenario.json   战役定义（长度、种子）
//! map.json        省份与邻接
//! oob.json        单位
//! ```
//!
//! 加载流程：读 → 解析 → 校验 → 构建 `World`。
//! 校验有任何 Error 级问题就拒绝加载——宁可启动失败，也不要跑一个坏模块。

pub mod error;
pub mod migrate;
pub mod model;
pub mod source;
pub mod validate;

use std::path::{Path, PathBuf};

use sim_core::fixed::Fixed;
use sim_core::world::{Province, Side, Terrain, Unit, World};

pub use error::{Issue, LoadError, Severity};
pub use migrate::{ensure_schema, RawModule};
pub use model::*;

/// 加载成功的模块。
#[derive(Debug, Clone)]
pub struct LoadedModule {
    pub meta: ModuleMeta,
    pub scenario: ScenarioDef,
    pub map: MapDef,
    pub oob: OobDef,
    /// 校验过程中的 Warning（Error 已导致加载失败）。
    pub warnings: Vec<Issue>,
    /// 模块自带的省界（`provinces_geo.json`）原文。没有就是 None，前端退回 Voronoi。
    ///
    /// 放在这里而不是让各个调用方自己去读文件：模块有两种形态（目录 / 压缩包），
    /// 调用方不该关心它是从磁盘读的还是从 zip 里解出来的。
    pub geo: Option<String>,
}

impl LoadedModule {
    /// 这个模块允许玩家扮演哪几方（去重、保序）。
    ///
    /// 没写 `playable_sides` 就是只能玩 `playable_side`——老模块的行为一字不变。
    pub fn allowed_sides(&self) -> Vec<sim_core::Side> {
        let ids = side_ids(&self.meta);
        let mut out: Vec<sim_core::Side> = self
            .meta
            .playable_sides
            .iter()
            .filter_map(|s| side_of(s, &ids))
            .collect();
        if out.is_empty() {
            // 没写 playable_sides：就只有 playable_side 一方。
            if let Some(s) = side_of(&self.meta.playable_side, &ids) {
                out.push(s);
            }
        }
        let mut seen = Vec::new();
        out.retain(|s| {
            if seen.contains(s) {
                false
            } else {
                seen.push(*s);
                true
            }
        });
        out
    }

    /// 模块目录名。
    pub fn dir_name(&self) -> &str {
        self.meta.id.rsplit('.').next().unwrap_or(&self.meta.id)
    }
}

fn parse_json(src: &source::Source, name: &str) -> Result<serde_json::Value, LoadError> {
    let text = src.read_text(name)?;
    serde_json::from_str(&text).map_err(|e| LoadError::Parse {
        path: src.path_of(name),
        message: format!("{}（第 {} 行第 {} 列）", e, e.line(), e.column()),
    })
}

/// 去掉以 `_` 开头的键（`_comment` / `_note` 之类）。
///
/// JSON 没有注释语法，作者一定会想写点什么。约定：**下划线开头的键一律当注释忽略**。
/// 这样两种模式都说得通——普通模式本来也读不到它们，严格模式不会因为
/// 一句注释就报"未知字段"。想真正标注字段含义请用各省各单位的 `note`。
fn strip_comment_keys(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(map) => {
            map.retain(|k, _| !k.starts_with('_'));
            for (_, child) in map.iter_mut() {
                strip_comment_keys(child);
            }
        }
        serde_json::Value::Array(arr) => {
            for child in arr.iter_mut() {
                strip_comment_keys(child);
            }
        }
        _ => {}
    }
}

/// 反序列化一个已经迁移过的片段。错误里带上"哪一段不对"。
fn from_value<T: serde::de::DeserializeOwned>(v: serde_json::Value) -> Result<T, String> {
    serde_json::from_value(v).map_err(|e| e.to_string())
}

/// 把"迟到番号"装到单位上。
///
/// 两种写法都支持，效果一样：
/// - `scenario.json` 的 `arrivals: [{day, unit, province}]`（推荐，一眼能看全）；
/// - `oob.json` 里单位的 `arrives_on: 48`（简写，省名取它自己的 `position`）。
///
/// 解析不到的单位 id / 省名不 panic，只打一行警告——内容层永远可能写错，
/// 而"某个番号没能按时到场"不该让整场战役起不来。
fn apply_arrivals(world: &mut World, module: &LoadedModule) {
    use std::collections::HashMap;
    // 省名 → 下标（省名是内容层的东西，单位表里写的是名字）
    let by_name: HashMap<&str, u32> = world
        .provinces
        .iter()
        .enumerate()
        .map(|(i, p)| (p.name.as_str(), i as u32))
        .collect();
    // 单位 id → 下标（保持 oob 书写顺序，索引即下标）
    let mut list: Vec<(u32, usize, Option<u32>)> = Vec::new();
    for a in &module.scenario.arrivals {
        match module.oob.units.iter().position(|u| u.id == a.unit) {
            Some(idx) => match by_name.get(a.province.as_str()) {
                Some(&pid) => list.push((a.day, idx, Some(pid))),
                None => eprintln!(
                    "警告：迟到番号 {} 的到场省「{}」不存在，这条被忽略",
                    a.unit, a.province
                ),
            },
            None => eprintln!("警告：迟到番号里的单位 id「{}」在 oob.json 里找不到", a.unit),
        }
    }
    // 简写：单位自己写 arrives_on
    for (idx, u) in module.oob.units.iter().enumerate() {
        if let Some(d) = u.arrives_on {
            let pid = by_name.get(world.provinces.get(u.position as usize).map(|p| p.name.as_str()).unwrap_or("")).copied();
            list.push((d, idx, pid.or(Some(u.position))));
        }
    }
    if list.is_empty() {
        return;
    }
    world.has_arrivals = true;
    for (day, idx, pid) in list {
        if let Some(u) = world.units.get_mut(idx) {
            u.arrives_on = Some(day);
            u.arrive_at = pid;
            // 第 0 日到场 = 开局就在，不用等到 tick
            u.arrived = day == 0;
        }
    }
}

/// 只读 `module.json`（不校验整包），给"列模块"这种轻量场景用。
///
/// 为什么需要它：模块列表要在首页把每张战役卡的标题、规模、简介填出来。
/// 以前直接 `read_to_string(dir.join("module.json"))`——模块变成 `.imod` 压缩包
/// 之后那一行会静默读不到，首页显示一排"?"。
pub fn peek_meta_json(path: &Path) -> Option<serde_json::Value> {
    let src = source::Source::open(path).ok()?;
    let text = src.read_text("module.json").ok()?;
    serde_json::from_str(&text).ok()
}

/// 加载一个模块：**目录**（开发形态）或 `.imod` 压缩包（发布形态）。
///
/// 两种形态读出来的东西完全一样——`Src` 把"从哪读"这件事挡掉了，
/// 后面迁移、校验、建世界的代码一行都不用分情况。
pub fn load_module(path: &Path) -> Result<LoadedModule, LoadError> {
    let src = source::Source::open(path)?;
    // 先读成裸 JSON，再迁移，最后反序列化。
    //
    // 顺序很重要：迁移器改的是**结构**，它必须在 serde 之前跑。
    // 如果先反序列化，"旧字段名"已经被默认值顶掉了，迁移器连看都看不到。
    let mut raw_module: serde_json::Value = parse_json(&src, "module.json")?;
    let mut raw_scenario: serde_json::Value = parse_json(&src, "scenario.json")?;
    let mut raw_map: serde_json::Value = parse_json(&src, "map.json")?;
    let mut raw_oob: serde_json::Value = parse_json(&src, "oob.json")?;
    let migrated = {
        let mut raw = migrate::RawModule {
            module: &mut raw_module,
            scenario: &mut raw_scenario,
            map: &mut raw_map,
            oob: &mut raw_oob,
        };
        migrate::ensure_schema(&mut raw, model::CURRENT_CONTENT_SCHEMA).map_err(|message| {
            LoadError::Migration {
                module: raw_module
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?")
                    .to_string(),
                message,
            }
        })?
    };
    strip_comment_keys(&mut raw_module);
    strip_comment_keys(&mut raw_scenario);
    strip_comment_keys(&mut raw_map);
    strip_comment_keys(&mut raw_oob);
    let meta: ModuleMeta = from_value(raw_module).map_err(|e| LoadError::Parse {
        path: src.path_of("module.json"),
        message: e,
    })?;
    let scenario: ScenarioDef = from_value(raw_scenario).map_err(|e| LoadError::Parse {
        path: src.path_of("scenario.json"),
        message: e,
    })?;
    let map: MapDef = from_value(raw_map).map_err(|e| LoadError::Parse {
        path: src.path_of("map.json"),
        message: e,
    })?;
    let oob: OobDef = from_value(raw_oob).map_err(|e| LoadError::Parse {
        path: src.path_of("oob.json"),
        message: e,
    })?;
    if !migrated.is_empty() {
        println!(
            "  迁移：模块格式 schema {} → {}（{} 步）",
            migrated[0],
            model::CURRENT_CONTENT_SCHEMA,
            migrated.len()
        );
    }

    // 全部检查：元数据/地图/序列 + 场景（结局子句恒真之类）。
    let issues = validate::validate_all(&meta, &map, &oob, &scenario);
    let errors: Vec<Issue> = issues.iter().filter(|i| i.is_error()).cloned().collect();
    if !errors.is_empty() {
        return Err(LoadError::Validation {
            module: meta.id.clone(),
            issues: errors,
        });
    }

    let warnings = issues.into_iter().filter(|i| !i.is_error()).collect();

    Ok(LoadedModule {
        meta,
        scenario,
        map,
        oob,
        warnings,
        // 省界是可选的大文件：目录形态读旁边那个 json，压缩包形态读包里的成员。
        geo: src.read_optional("provinces_geo.json"),
    })
}

/// 从内存中的四个片段构建，不需要文件系统（测试用）。
pub fn from_parts(
    meta: ModuleMeta,
    scenario: ScenarioDef,
    map: MapDef,
    oob: OobDef,
) -> Result<LoadedModule, LoadError> {
    // 全部检查：元数据/地图/序列 + 场景（结局子句恒真之类）。
    let issues = validate::validate_all(&meta, &map, &oob, &scenario);
    let errors: Vec<Issue> = issues.iter().filter(|i| i.is_error()).cloned().collect();
    if !errors.is_empty() {
        return Err(LoadError::Validation {
            module: meta.id.clone(),
            issues: errors,
        });
    }
    let warnings = issues.into_iter().filter(|i| !i.is_error()).collect();
    Ok(LoadedModule {
        meta,
        scenario,
        map,
        oob,
        warnings,
        geo: None,
    })
}

fn terrain_of(t: TerrainDef) -> Terrain {
    match t {
        TerrainDef::Plain => Terrain::Plain,
        TerrainDef::Hill => Terrain::Hill,
        TerrainDef::Mountain => Terrain::Mountain,
        TerrainDef::Marsh => Terrain::Marsh,
        TerrainDef::Water => Terrain::Water,
        TerrainDef::Urban => Terrain::Urban,
    }
}

/// 模块的阵营表：id 列表，**顺序即 A/B**。
///
/// 空着时退回老模块的默认 `["cn","jp"]`——那些模块的数据里写的就是这两个 id，
/// 所以行为一字不变。引擎本身只认 A/B，不认识这些字符串。
pub fn side_ids(meta: &ModuleMeta) -> Vec<String> {
    let mut ids: Vec<String> = meta.sides.iter().map(|s| s.id.clone()).collect();
    if ids.is_empty() {
        ids = vec!["cn".to_string(), "jp".to_string()];
    }
    // 只取前两个：这一版的模拟是二元的，第三方阵营不在范围内。
    ids.truncate(2);
    ids
}

/// 阵营 id → 内部的 A/B。没声明过的 id 返回 None（由校验器报错）。
fn side_of(id: &SideDef, ids: &[String]) -> Option<Side> {
    let s = id.as_str();
    if ids.first().map(|x| x == s).unwrap_or(false) {
        Some(Side::A)
    } else if ids.get(1).map(|x| x == s).unwrap_or(false) {
        Some(Side::B)
    } else {
        None
    }
}

/// 内部 A/B → 阵营 id（写回 JSON 用）。
pub fn side_id(side: Side, ids: &[String]) -> String {
    let i = side.index() as usize;
    ids.get(i).cloned().unwrap_or_else(|| {
        if side == Side::A {
            "cn".into()
        } else {
            "jp".into()
        }
    })
}

/// 把加载好的模块构建成模拟核心的 `World`。
///
/// 这是浮点 → 定点的唯一转换点，之后全程定点。
pub fn build_world(module: &LoadedModule) -> World {
    // 阵营表：这一仗有哪两方（顺序即 A/B）。后面所有 id→Side 的换算都用它。
    let ids = side_ids(&module.meta);
    // 按 id 排序，保证索引与 id 一致
    let mut defs: Vec<&ProvinceDef> = module.map.provinces.iter().collect();
    defs.sort_by_key(|p| p.id);

    let mut provinces = Vec::with_capacity(defs.len());
    for d in defs {
        let owner = d
            .owner
            .as_ref()
            .and_then(|s| side_of(s, &ids))
            .unwrap_or(Side::A);
        let mut p = Province::new(&d.name, terrain_of(d.terrain), owner);
        p.fort = d.fort.min(5);
        p.elev = d.elev;
        p.relief = d.relief;
        p.supply_throughput = Fixed::from_f64_input(d.supply_throughput).clamp(Fixed::ZERO, Fixed::ONE);
        p.neighbors = d.neighbors.clone();
        p.is_supply_source = d.supply_source;
        p.rail = d.rail;
        p.lon = Fixed::from_f64_input(d.lon.unwrap_or(0.0));
        p.lat = Fixed::from_f64_input(d.lat.unwrap_or(0.0));
        // 阵地：没写就生成一个（省名 + 省地形 + 全宽度），老模块因此不用改。
        // 写了就按书写顺序编号——编号也是确定性的一部分。
        if d.positions.is_empty() {
            p.positions
                .push(sim_core::world::Position::new(&d.name, terrain_of(d.terrain)));
        } else {
            for pd in &d.positions {
                let terr = pd.terrain.map(terrain_of).unwrap_or(terrain_of(d.terrain));
                let mut node = sim_core::world::Position::new(&pd.name, terr);
                node.width = Fixed::from_f64_input(pd.width.clamp(0.2, 1.0));
                node.fort = pd.fort.min(5);
                p.positions.push(node);
            }
        }
        provinces.push(p);
    }

    let mut units = Vec::with_capacity(module.oob.units.len());
    // 保持 oob 中的书写顺序（也是确定性的一部分）
    for u in &module.oob.units {
        let strength_k = u.strength.round() as i32;
        let side = side_of(&u.side, &ids).unwrap_or(Side::A);
        let mut unit = Unit::new(&u.name, side, u.position, strength_k);
        // 支持小数兵力
        let frac = u.strength - u.strength.round();
        if frac.abs() > 1e-9 {
            let extra = Fixed::from_f64_input(frac);
            unit.strength += extra;
            unit.max_strength += extra;
        }
        unit.effectiveness = Fixed::from_f64_input(u.effectiveness.max(0.0));
        unit.supply_reach = Fixed::from_f64_input(u.supply_reach.clamp(0.0, 1.0));
        unit.kind = match u.kind {
            UnitKindDef::Infantry => sim_core::UnitKind::Infantry,
            UnitKindDef::Armor => sim_core::UnitKind::Armor,
            UnitKindDef::Air => sim_core::UnitKind::Air,
            UnitKindDef::Naval => sim_core::UnitKind::Naval,
        };
        unit.formation = u.parent.clone().unwrap_or_default();
        unit.recon_radius = u.recon_radius;
        unit.stance = match u.stance {
            StanceDef::Attack => sim_core::world::Stance::Attack,
            StanceDef::Defend => sim_core::world::Stance::Defend,
        };
        unit.strike_power = Fixed::from_f64_input(u.strike_power.max(0.0));
        unit.reform_days = u.reform_days;
        unit.reform_ratio = Fixed::from_f64_input(u.reform_ratio.clamp(0.0, 1.0));
        // 预览版弹药/油料：模块不写就是满的，而且完全不影响模拟（开关默认关）。
        unit.ammo = Fixed::from_f64_input(u.ammo.clamp(0.0, 1.0));
        unit.fuel = Fixed::from_f64_input(u.fuel.clamp(0.0, 1.0));
        unit.fuel_burn = match u.fuel_burn {
            Some(v) => Fixed::from_f64_input(v.clamp(0.0, 1.0)),
            // 没写就按兵种给个默认：步兵徒步不吃油，机械化才吃。
            None => match unit.kind {
                sim_core::UnitKind::Infantry => Fixed::ZERO,
                sim_core::UnitKind::Armor => Fixed::ratio(12, 100),
                sim_core::UnitKind::Air | sim_core::UnitKind::Naval => Fixed::ratio(15, 100),
            },
        };
        if let Some(c) = &u.commander {
            unit.commander = sim_core::world::Commander::new(
                &c.name,
                c.obedience,
                c.initiative,
                c.aggression,
                c.caution,
            )
            .with_persona(&c.title, &c.persona, &c.agenda)
            .with_faction(c.faction.as_deref());
        }
        units.push(unit);
    }

    let seed = if module.scenario.seed != 0 {
        module.scenario.seed
    } else {
        // 没写种子时，由模块 id 派生，保证同模块同结果
        simple_hash(&module.meta.id)
    };

    let mut world = World::new(seed, provinces, units);
    // 迟到番号：先在单位上记下"第几日、在哪到场"，再由 tick() 每天结算到场。
    apply_arrivals(&mut world, module);
    // 预览版开关：模块不声明 = 全关 = 第一版发行版的定稿行为。
    // 这一行是"定稿不被改"的总闸：下面所有新系统都先看这几个布尔值。
    world.fx = features_from(module.scenario.preview.as_ref(), module.meta.scale);
    world.comms = comms_from(module.scenario.comms.as_ref());
    world.campaign_days = module.scenario.days;
    // 编制实体：按 parent 归组。
    //
    // 兵团部 = 名字与番号相同的那支部队（第29军团 既是军团长、也是 187 师的上级）；
    // 找不到同名部队的番号（第1兵团）就是纸面编制，hq = None。
    {
        use sim_core::world::Formation;
        let mut forms: Vec<Formation> = Vec::new();
        for (i, u) in world.units.iter().enumerate() {
            if u.formation.is_empty() {
                continue;
            }
            match forms.iter_mut().find(|f| f.name == u.formation) {
                Some(f) => f.units.push(i),
                None => forms.push(Formation {
                    name: u.formation.clone(),
                    side: u.side,
                    hq: None,
                    units: vec![i],
                }),
            }
        }
        for f in forms.iter_mut() {
            f.hq = world
                .units
                .iter()
                .position(|u| u.name == f.name && u.side == f.side);
        }
        world.formations = forms;
    }
    world.endings = endings_from(&module.scenario.endings, &ids);

    // 派系（预览版）：把将领按 `faction` 归组，算好部队下标。
    if world.fx.politics {
        build_factions(&mut world, &module.oob.factions, &ids);
    }

    // 编译模块声明的规则
    let (rule_set, _problems) = rules_from(&module.scenario.rules, &ids);
    world.rules = std::sync::Arc::new(rule_set);

    world
}

/// 把模块声明 + **地图尺度**翻成引擎的 `Features`。
///
/// **1.0 起默认开启**：这几个机制已经并回主线，是正式基线。
/// 但"默认"不等于"一刀切"——`front_line`（连续正面模型）的默认值**跟着尺度走**：
///
/// | scale | `front_line` 默认 | 为什么 |
/// |---|---|---|
/// | `theater`（≈35 km/省） | **关** | 一个省是一个"区"，侧翼和后方是真实存在的；占领要求清空邻省才符合这类地图的时空粒度 |
/// | `naval` | 关 | 水区面积大，同理 |
/// | `front` / `battle` / `siege`（5–15 km/省） | **开** | 一个省是一个"阵地"，把它清空就等于拿下了；四邻皆敌是常态，不是被围 |
///
/// 这不是"给作者添麻烦"，而是尺度决定语义：同样一条"邻接有敌不准占领"，
/// 在 35 km 的战役图上是对的（防穿插），在 10 km 的正面图上会把战线冻住。
///
/// 想覆盖就显式写：`"preview": { "front_line": true/false }`（无论 scale 怎么写都听模块的）。
pub fn features_from(
    def: Option<&PreviewDef>,
    scale: model::ScaleDef,
) -> sim_core::Features {
    use model::ScaleDef;
    // 尺度决定的默认值（只有 front_line 分尺度；其余三项对所有尺度都该开）
    let dense_by_scale = matches!(
        scale,
        ScaleDef::Front | ScaleDef::Battle | ScaleDef::Siege
    );
    let defaults = sim_core::Features {
        front_line: dense_by_scale,
        ..sim_core::Features::default()
    };
    match def {
        None => defaults,
        Some(d) => sim_core::Features {
            ammo_fuel: d.ammo_fuel.unwrap_or(true),
            rail_damage: d.rail_damage.unwrap_or(true),
            politics: d.politics.unwrap_or(true),
            front_line: d.front_line.unwrap_or(defaults.front_line),
        },
    }
}

/// 把派系声明装上世界，并按将领的 `faction` 归组。
///
/// 忽略的引用不报错：内容层永远可能写错（引用了不存在的派系 id），
/// 那时候这支部队就算"无派系"，不 panic、也不影响确定性。
fn build_factions(world: &mut World, defs: &[FactionDef], ids: &[String]) {
    use sim_core::politics::Faction;
    let mut out: Vec<Faction> = Vec::new();
    for d in defs {
        let Some(side) = side_of(&d.side, ids) else {
            eprintln!("警告：派系 {} 的 side 没在 module.sides 里声明，已忽略", d.id);
            continue;
        };
        let f = Faction::new(
            &d.id,
            &d.name,
            side,
            Fixed::from_f64_input(d.weight.clamp(0.0, 1.0)),
        );
        out.push(f);
    }
    for (i, u) in world.units.iter().enumerate() {
        let Some(fid) = u.commander.faction.as_deref() else {
            continue;
        };
        if let Some(f) = out.iter_mut().find(|f| f.id == fid && f.side == u.side) {
            f.units.push(i);
        }
    }
    world.factions = out;
}

/// 从模块配置解析通信条件。///
/// 引擎不认识"年代"，只认识延迟天数与通信手段的名字。
/// 这样 1805 年和 1944 年用的是同一套代码，只是参数不同。
/// 把模块里"人话写的"结局编译成引擎认识的子句。
///
/// 指标名写错不 panic：整条子句被丢掉，并在 `problems` 里留一句。
/// 内容层永远可能写错，引擎不该因此崩掉整局。
pub fn endings_from(defs: &[EndingDef], ids: &[String]) -> Vec<sim_core::endings::Ending> {
    use sim_core::endings::{Clause, Cmp, Ending, Metric};

    let mut out = Vec::new();
    for d in defs {
        let mut clauses = Vec::new();
        // 有一条子句读不懂，就把**整条结局**丢掉。
        // 只丢子句是危险的：剩下的子句变少，条件变松，
        // 一条空子句的结局会永远成立（已经踩过：E7 在第 1 日就触发了）。
        let mut bad = false;
        for c in &d.require {
            let metric = match c.metric.as_str() {
                "day" => Metric::Day,
                "provinces" => Metric::Provinces,
                "strength" => Metric::Strength,
                "losses" => Metric::Losses,
                "units_alive" => Metric::UnitsAlive,
                "commanders_lost" => Metric::CommandersLost,
                "holds_province" => Metric::HoldsProvince,
                other => {
                    eprintln!("警告：结局 {} 的指标 {other} 不认识，整条结局被丢掉", d.id);
                    bad = true;
                    continue;
                }
            };
            let cmp = match c.cmp.as_deref() {
                None | Some("ge") => Cmp::Ge,
                Some("gt") => Cmp::Gt,
                Some("le") => Cmp::Le,
                Some("lt") => Cmp::Lt,
                Some(other) => {
                    eprintln!("警告：结局 {} 的比较符 {other} 不认识，整条结局被丢掉", d.id);
                    bad = true;
                    continue;
                }
            };
            clauses.push(Clause {
                metric,
                side: c.side.as_ref().and_then(|s| side_of(s, ids)),
                cmp,
                // 省略 value 时按 metric 给默认：holds_province 是 0/1 量纲，
                // 缺省 0.0 会让 `>= 0` 恒真（淞沪模块第 1 天就结束的原因）。
                value: clause_default_value(metric, c.value),
                province: c.province.clone(),
            });
        }
        if bad || clauses.is_empty() {
            eprintln!("警告：结局 {} 没有可用子句，已跳过", d.id);
            continue;
        }
        out.push(Ending {
            id: d.id.clone(),
            name: d.name.clone(),
            text: d.text.clone(),
            priority: d.priority,
            at_end_only: d.at_end_only,
            clauses,
        });
    }
    out
}
/// 子句省略 `value` 时的默认阈值。
///
/// `holds_province` 是 0/1 量纲：缺省 0.0 配上默认的 `ge` 就是 `x >= 0`，
/// **两个方向都恒真**——一条写错的结局会在第 1 天把战役判死。
/// 所以这里给它 1.0（"确实控制着"），其余指标沿用 0.0，
/// 并由 `validate::validate_scenario` 对"恒真子句"报 Warning。
pub fn clause_default_value(metric: sim_core::endings::Metric, raw: Option<f64>) -> f64 {
    use sim_core::endings::Metric;
    raw.unwrap_or(match metric {
        Metric::HoldsProvince => 1.0,
        _ => 0.0,
    })
}

pub fn comms_from(def: Option<&CommsDef>) -> sim_core::world::CommsConfig {
    use sim_core::world::{CommsConfig, CommsKind};

    let mut cfg = CommsConfig::telegraph_era();
    let Some(d) = def else {
        return cfg;
    };

    if let Some(preset) = &d.preset {
        cfg = match preset.as_str() {
            "courier" => CommsConfig::courier_era(),
            "radio" => CommsConfig::radio_era(),
            "wire_telegraph" | "telegraph" => CommsConfig::telegraph_era(),
            other => {
                // 未知预设：保持默认，不崩溃。向前兼容的关键。
                let _ = other;
                CommsConfig::telegraph_era()
            }
        };
    }

    if let Some(v) = d.transmission {
        cfg.transmission = v;
    }
    if let Some(v) = d.staff {
        cfg.staff = v;
    }
    if let Some(v) = d.order_delay {
        cfg.order_delay = v;
    }

    // 手段名只影响叙事，这里按预设校准一下
    if d.preset.is_none() {
        cfg.kind = CommsKind::WireTelegraph;
    }
    cfg
}

fn simple_hash(s: &str) -> u64 {
    let mut h: u64 = 0xCBF2_9CE4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01B3);
    }
    h
}

/// 引擎版本约束检查。只是提示，不阻断加载。
pub fn check_engine_compat(meta: &ModuleMeta, engine_version: &str) -> Vec<Issue> {
    let mut issues = Vec::new();
    if !version_ge(engine_version, &meta.engine.min) {
        issues.push(Issue::error(
            "engine.too_old",
            format!(
                "模块要求引擎 >= {}，当前 {}",
                meta.engine.min, engine_version
            ),
        ));
    }
    if let Some(max) = &meta.engine.max {
        if version_ge(engine_version, max) {
            issues.push(Issue::warn(
                "engine.too_new",
                format!("模块声明最高支持 {}，当前 {}，可能不兼容", max, engine_version),
            ));
        }
    }
    issues
}

/// 极简语义化版本比较（主.次.修订）。无法解析的段按 0 处理。
fn version_ge(a: &str, b: &str) -> bool {
    let pa = parse_version(a);
    let pb = parse_version(b);
    pa >= pb
}

fn parse_version(s: &str) -> (u32, u32, u32) {
    let core = s.split(['-', '+']).next().unwrap_or("");
    let mut it = core.split('.');
    let major = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    let minor = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    let patch = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    (major, minor, patch)
}

/// 模块文件的路径集合，便于工具打印。
pub fn module_files(dir: &Path) -> Vec<PathBuf> {
    ["module.json", "scenario.json", "map.json", "oob.json"]
        .iter()
        .map(|f| dir.join(f))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare() {
        assert!(version_ge("0.2.0", "0.1.0"));
        assert!(version_ge("0.1.0", "0.1.0"));
        assert!(!version_ge("0.0.9", "0.1.0"));
        assert!(version_ge("1.0.0", "0.9.9"));
    }

    #[test]
    fn hash_is_stable() {
        assert_eq!(simple_hash("a"), simple_hash("a"));
        assert_ne!(simple_hash("a"), simple_hash("b"));
    }
}


// ---------------------------------------------------------------- 规则

/// 把模块里的规则声明编译成引擎的 `RuleSet`。
///
/// 遇到写错的条目时**跳过并收集警告**，不整体失败——
/// 一条规则写错不该让整个战役装不进去。
pub fn rules_from(defs: &[RuleDef], ids: &[String]) -> (sim_core::RuleSet, Vec<String>) {
    use sim_core::{Phase, Rule};

    let mut rules = Vec::new();
    let mut problems = Vec::new();

    for d in defs {
        let phase = match d.phase {
            PhaseDef::PreSupply => Phase::PreSupply,
            PhaseDef::PostSupply => Phase::PostSupply,
            PhaseDef::PostMovement => Phase::PostMovement,
            PhaseDef::PreCombat => Phase::PreCombat,
            PhaseDef::PostCombat => Phase::PostCombat,
            PhaseDef::PostTick => Phase::PostTick,
        };

        let when = match condition_from(&d.when, ids) {
            Ok(c) => c,
            Err(e) => {
                problems.push(format!("规则 {} 的条件无效：{}", d.id, e));
                continue;
            }
        };

        let mut then = Vec::new();
        for p in &d.then {
            match primitive_from(p) {
                Ok(x) => then.push(x),
                Err(e) => {
                    problems.push(format!("规则 {} 的动作无效：{}", d.id, e));
                }
            }
        }
        if then.is_empty() {
            problems.push(format!("规则 {} 没有任何有效动作，已跳过", d.id));
            continue;
        }

        rules.push(Rule {
            id: d.id.clone(),
            phase,
            priority: d.priority,
            when,
            then,
        });
    }

    (sim_core::RuleSet::new(rules), problems)
}

fn terrain_from(s: &str) -> Result<sim_core::Terrain, String> {
    match s {
        "plain" => Ok(sim_core::Terrain::Plain),
        "hill" => Ok(sim_core::Terrain::Hill),
        "mountain" => Ok(sim_core::Terrain::Mountain),
        "marsh" => Ok(sim_core::Terrain::Marsh),
        "water" => Ok(sim_core::Terrain::Water),
        "urban" => Ok(sim_core::Terrain::Urban),
        other => Err(format!("未知地形 `{other}`")),
    }
}

fn kind_from(s: &str) -> Result<sim_core::UnitKind, String> {
    match s {
        "infantry" => Ok(sim_core::UnitKind::Infantry),
        "armor" => Ok(sim_core::UnitKind::Armor),
        "air" => Ok(sim_core::UnitKind::Air),
        "naval" => Ok(sim_core::UnitKind::Naval),
        other => Err(format!("未知兵种 `{other}`")),
    }
}

/// 阵营 id（字符串）→ 内部的 A/B，按模块声明的顺序。
fn side_from(s: &str, ids: &[String]) -> Result<sim_core::Side, String> {
    side_of(&SideDef::new(s), ids).ok_or_else(|| {
        format!(
            "未知阵营 `{s}`：这个模块声明的是 {}",
            ids.join(" / ")
        )
    })
}

fn condition_from(c: &ConditionDef, ids: &[String]) -> Result<sim_core::Condition, String> {
    use sim_core::Condition as C;
    Ok(match c {
        ConditionDef::Always => C::Always,
        ConditionDef::Never => C::Never,
        ConditionDef::All(v) => {
            let mut out = Vec::new();
            for x in v {
                out.push(condition_from(x, ids)?);
            }
            C::All(out)
        }
        ConditionDef::Any(v) => {
            let mut out = Vec::new();
            for x in v {
                out.push(condition_from(x, ids)?);
            }
            C::Any(out)
        }
        ConditionDef::Not(x) => C::Not(Box::new(condition_from(x, ids)?)),
        ConditionDef::Terrain(s) => C::ProvinceTerrain(terrain_from(s)?),
        ConditionDef::UnitKind(s) => C::UnitKindIs(kind_from(s)?),
        ConditionDef::StrengthBelow(r) => C::StrengthBelow(Fixed::from_f64_input(*r)),
        ConditionDef::ProvinceOwner(s) => C::ProvinceOwner(side_from(s, ids)?),
        ConditionDef::Side(s) => C::UnitSide(side_from(s, ids)?),
        ConditionDef::InCombat => C::InCombat,
    })
}

fn primitive_from(p: &PrimitiveDef) -> Result<sim_core::Primitive, String> {
    use sim_core::Primitive as P;
    Ok(match p {
        PrimitiveDef::ModifyStrengthPct(v) => P::ModifyStrengthPct(Fixed::from_f64_input(*v)),
        PrimitiveDef::ModifyMorale(v) => P::ModifyMorale(Fixed::from_f64_input(*v)),
        PrimitiveDef::ModifySupply(v) => P::ModifySupply(Fixed::from_f64_input(*v)),
        PrimitiveDef::Log(s) => P::Log(s.clone()),
    })
}



