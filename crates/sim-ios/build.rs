//! 把选定的战役模块打进静态库 —— 逻辑在 `sim-bundle`，安卓壳用的是同一份。

fn main() {
    sim_bundle::embed_module();
}
