//! 测试场景。
//!
//! 这是给核心跑通用的**假数据**，不是史实。真实数据将来由 `sim-content`
//! 从模块文件加载。这里只求：能跑、能打、能验证确定性。
//!
//! 场景形状：一条南北走廊，日军在北（进攻），国军在南（防守），
//! 中间有一个隘口和一个要塞。这是南浔线的极简抽象。

use crate::world::{Province, Side, Terrain, Unit, World};

/// 简化的走廊场景。兵力单位见 `Unit`：千人。
pub fn nanxun_line(seed: u64) -> World {
    let spec: &[(&str, Terrain, Side)] = &[
        ("九江", Terrain::Plain, Side::Jp),
        ("姑塘", Terrain::Marsh, Side::Jp),
        ("星子", Terrain::Hill, Side::Jp),
        ("隘口", Terrain::Mountain, Side::Jp),
        ("金官桥", Terrain::Hill, Side::Cn),
        ("德安", Terrain::Urban, Side::Cn),
        ("万家岭", Terrain::Mountain, Side::Cn),
        ("南昌", Terrain::Plain, Side::Cn),
    ];

    let n = spec.len();
    let mut provinces = Vec::with_capacity(n);

    for (i, (name, terrain, owner)) in spec.iter().enumerate() {
        let mut p = Province::new(name, *terrain, *owner);
        let mut nb = Vec::new();
        if i > 0 {
            nb.push((i - 1) as u32);
        }
        if i + 1 < n {
            nb.push((i + 1) as u32);
        }
        p.neighbors = nb;
        if i == 0 || i == n - 1 {
            p.is_supply_source = true;
        }
        provinces.push(p);
    }

    // 隘口设防，万家岭也有工事
    provinces[3].fort = 3;
    provinces[6].fort = 2;

    let units = vec![
        Unit::new("第106师团", Side::Jp, 0, 16),
        Unit::new("第101师团", Side::Jp, 1, 12),
        Unit::new("第74军", Side::Cn, 4, 15),
        Unit::new("第4军", Side::Cn, 5, 12),
        Unit::new("第66军", Side::Cn, 6, 9),
    ];

    World::new(seed, provinces, units)
}

/// 一条直线，用于最小回归测试。
pub fn straight_line(seed: u64) -> World {
    let provinces = vec![
        Province::new("A", Terrain::Plain, Side::Cn)
            .with_neighbors(&[1])
            .as_source(),
        Province::new("B", Terrain::Plain, Side::Cn).with_neighbors(&[0, 2]),
        Province::new("C", Terrain::Plain, Side::Jp)
            .with_neighbors(&[1])
            .as_source(),
    ];
    let units = vec![
        Unit::new("cn-1", Side::Cn, 0, 10),
        Unit::new("jp-1", Side::Jp, 2, 10),
    ];
    World::new(seed, provinces, units)
}
