#!/usr/bin/env bash
#
# build-ios-rust.sh —— 把 Rust 内核编成 iOS 静态库，落到 target/ios/libsim_ios.a。
#
# 谁调它：
#   · Xcode 的 preBuildScript（ios/project.yml 里那条），每次构建都跑一次。
#     cargo 自己是增量的，没改就是 0.1 秒返回，所以"每次都跑"最省心。
#   · 或者手动：tools/build-ios-rust.sh [--simulator|--device] [--debug|--release]
#
# 为什么不用 XCFramework：那个要多一步 `xcodebuild -create-xcframework`，
# 而且真机/模拟器两份切片还得分开维护。这里固定写一个路径、按当前平台重编，
# 换平台时重编一次就行——cargo 的产物本来就在 target/ 里按 target 分好了。
#
# 为什么输出到 target/ios/ 而不是 target/<triple>/release/：LIBRARY_SEARCH_PATHS
# 是写死在工程里的，不能随平台变；拷贝一次比在工程里写三个条件分支清楚。

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# ── 平台与配置：优先听 Xcode 的，其次看命令行 ──────────────────────────────
PLATFORM="${PLATFORM_NAME:-}"
CONFIG="${CONFIGURATION:-}"
ARCHS_="${ARCHS:-}"

while [ $# -gt 0 ]; do
  case "$1" in
    --simulator) PLATFORM="iphonesimulator" ;;
    --device)    PLATFORM="iphoneos" ;;
    --debug)     CONFIG="Debug" ;;
    --release)   CONFIG="Release" ;;
    *) echo "用法：$0 [--device|--simulator] [--debug|--release]" >&2; exit 2 ;;
  esac
  shift
done

[ -n "$PLATFORM" ] || PLATFORM="iphoneos"
[ -n "$CONFIG" ] || CONFIG="Release"

# 要编哪些 triple。真机只要一个；模拟器可能两个都要（见下）。
TRIPLES=()
FAT=0
case "$PLATFORM" in
  iphoneos)
    TRIPLES=(aarch64-apple-ios)
    ;;
  iphonesimulator)
    # `-destination generic/platform=iOS Simulator` 会同时要 arm64 和 x86_64，
    # 只给一个切片的结果就是几百行
    #   "found architecture 'arm64', required architecture 'x86_64'"
    # 然后缺符号。所以 ARCHS 里有两个就都编，最后 lipo 成一份 fat 库。
    WANT_ARM=0; WANT_X86=0
    for a in ${ARCHS_:-$(uname -m)}; do
      case "$a" in
        arm64*)  WANT_ARM=1 ;;
        x86_64)  WANT_X86=1 ;;
      esac
    done
    [ "$WANT_ARM" = 1 ] && TRIPLES+=(aarch64-apple-ios-sim)
    [ "$WANT_X86" = 1 ] && TRIPLES+=(x86_64-apple-ios)
    [ ${#TRIPLES[@]} -gt 1 ] && FAT=1
    if [ ${#TRIPLES[@]} -eq 0 ]; then
      echo "ARCHS 里没有认识的架构：$ARCHS_" >&2
      exit 2
    fi
    ;;
  *)
    echo "不认识的 PLATFORM_NAME=$PLATFORM（只做 iphoneos / iphonesimulator）" >&2
    exit 2
    ;;
esac

case "$CONFIG" in
  Debug)   PROFILE_FLAG="" ; PROFILE_DIR="debug" ;;
  *)       PROFILE_FLAG="--release" ; PROFILE_DIR="release" ;;
esac

# ── cargo 在哪：Xcode 的 PATH 里没有 ~/.cargo/bin ──────────────────────────
CARGO_BIN="${CARGO:-}"
if [ -z "$CARGO_BIN" ] || [ ! -x "$CARGO_BIN" ]; then
  if command -v cargo >/dev/null 2>&1; then
    CARGO_BIN="$(command -v cargo)"
  else
    for cand in "$HOME/.cargo/bin/cargo" /opt/homebrew/bin/cargo /usr/local/bin/cargo; do
      if [ -x "$cand" ]; then CARGO_BIN="$cand"; break; fi
    done
  fi
fi
if [ -z "${CARGO_BIN:-}" ] || [ ! -x "$CARGO_BIN" ]; then
  echo "找不到 cargo。先装 Rust（https://rustup.rs），或者用 CARGO=/路径 cargo 指过来。" >&2
  exit 1
fi

# ── 目标得先装上：少了这一步报的是 linker 的一堆天书 ──────────────────────
if ! "$CARGO_BIN" -V >/dev/null 2>&1; then
  echo "cargo 跑不起来：$CARGO_BIN" >&2
  exit 1
fi
RUSTUP_BIN="$(dirname "$CARGO_BIN")/rustup"
if [ -x "$RUSTUP_BIN" ]; then
  for t in "${TRIPLES[@]}"; do
    if ! "$RUSTUP_BIN" target list --installed 2>/dev/null | grep -qx "$t"; then
      echo "缺少 Rust target: $t" >&2
      echo "  $RUSTUP_BIN target add $t" >&2
      exit 1
    fi
  done
fi

# 部署版本要跟着工程走：rustc 会把 min-version 写进目标文件，
# 比工程低没事，比工程高会在真机上直接起不来（dyld 报版本不符）。
export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-15.4}"

# Xcode 会把 DEVELOPER_DIR 设成"当前 xcode-select 的那个"——如果那还是 Command
# Line Tools，rustc 内部的 `xcrun --sdk iphoneos` 就会失败（sim-core 的 cdylib
# 目标会去链一次，报一串 "SDK iphoneos cannot be located"）。这里按 Xcode 的实际
# 位置覆盖一次，绕开"忘了跑 sudo xcode-select -s"。
if [ -d /Applications/Xcode.app/Contents/Developer ]; then
  export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
fi

# 构建中间产物（target/，几 GB）可以指到别的卷上——内部盘只剩 30 GB 时很有用：
#   CARGO_TARGET_DIR=/Volumes/Dev/ironmind-target tools/build-ios.sh
# 静态库最后还是拷回 target/ios/（26 MB，Xcode 按固定路径找它）。
CARGO_TARGET_ARGS=()
TARGET_ROOT="${CARGO_TARGET_DIR:-$ROOT/target}"
if [ -n "${CARGO_TARGET_DIR:-}" ]; then
  CARGO_TARGET_ARGS=(--target-dir "$CARGO_TARGET_DIR")
fi

OUT_DIR="$ROOT/target/ios"
mkdir -p "$OUT_DIR"
SLICES=()
for t in "${TRIPLES[@]}"; do
  echo "▸ Rust → $t（$PROFILE_DIR）"
  # `--lib`：只编库，不尝试编 tests/ 里的集成测试（那要在宿主上跑）
  "$CARGO_BIN" build --lib -p sim-ios --target "$t" $PROFILE_FLAG ${CARGO_TARGET_ARGS[@]+"${CARGO_TARGET_ARGS[@]}"}
  SLICES+=("$TARGET_ROOT/$t/$PROFILE_DIR/libsim_ios.a")
done

if [ "$FAT" = 1 ]; then
  lipo -create -output "$OUT_DIR/libsim_ios.a" "${SLICES[@]}"
  echo "▸ fat: $(lipo -archs "$OUT_DIR/libsim_ios.a" 2>/dev/null)"
else
  cp -f "${SLICES[0]}" "$OUT_DIR/libsim_ios.a"
fi
echo "▸ $OUT_DIR/libsim_ios.a（$(du -h "$OUT_DIR/libsim_ios.a" | cut -f1)）"
