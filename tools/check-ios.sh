#!/usr/bin/env bash
#
# check-ios.sh —— 出 iOS 包之前先跑这个：**缺什么，一句话说清楚**。
#
# 为什么要有：iOS 这一层最容易卡的从来不是代码，是"这台机器少一个部件"。
# 少 Xcode、少 SDK、少模拟器运行时、少 Rust target、没有签名身份——
# 每一种的报错都长得很不像人话（linker 的 sysroot 警告、dyld 版本不符、
# `xcodebuild: error: SDK "iphoneos" cannot be located`）。这个脚本把
# 它们逐个翻成"装什么、跑哪条命令"。
#
# 用法：
#   tools/check-ios.sh              # 只检查，缺了就报
#   tools/check-ios.sh --verbose    # 顺便把版本号、路径、身份都打出来
#
# 退出码：0 = 能出包；1 = 有硬缺口；2 = 只能出包、验不了 UI（缺模拟器运行时）。

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

VERBOSE=0
[ "${1:-}" = "--verbose" ] && VERBOSE=1

# 逐条记录：0 无关紧要 / 1 硬缺口 / 2 只是验不了 UI
WORST=0
note() { printf '  %s：%s\n' "$1" "$2"; }
ok()   { note "$1" "✅ $2"; }
# ⚠️ 只把 0 抬到 2：不能把已经记下的 1（硬缺口）降级成"只是看不见界面"
warn() { note "$1" "⚠️  $2"; if [ "$WORST" -eq 0 ]; then WORST=2; fi; }
bad()  { note "$1" "❌ $2"; WORST=1; }

echo "钢心 / Iron Mind —— iOS 出包前置检查"
echo "（每一行左边的缺口都对应一条能直接粘进终端的修法）"
echo

# ── 1. 机器本身 ────────────────────────────────────────────────────────────
echo "【机器】"
OS="$(sw_vers -productVersion 2>/dev/null || echo "?")"
ARCH="$(uname -m)"
note "macOS" "$OS（$ARCH）"
if [ "$ARCH" != "arm64" ]; then
  warn "架构" "Intel：能编、能出包；但新版 Xcode 的模拟器运行时不一定还给 x86_64"
else
  ok "架构" "Apple Silicon"
fi

# Xcode 装完 + 模拟器运行时之后，一颗 iOS 构建大约要 20–30 GB 临时空间
AVAIL_GB="$(df -g "$ROOT" 2>/dev/null | awk 'NR==2 {print $4}')"
AVAIL_GB="${AVAIL_GB:-0}"
if [ "$AVAIL_GB" -lt 15 ]; then
  bad "磁盘余量" "${AVAIL_GB} GB — 出包要 20 GB 以上，先腾地方"
elif [ "$AVAIL_GB" -lt 40 ]; then
  warn "磁盘余量" "${AVAIL_GB} GB — 出包够，装模拟器运行时（约 7 GB）就紧了"
else
  ok "磁盘余量" "${AVAIL_GB} GB"
fi

# ── 2. Xcode 与 SDK ────────────────────────────────────────────────────────
echo
echo "【Xcode】"
DEV_DIR="$(xcode-select -p 2>/dev/null || echo "")"
if [ -d /Applications/Xcode.app ]; then
  ok "Xcode.app" "$(defaults read /Applications/Xcode.app/Contents/Info CFBundleShortVersionString 2>/dev/null || echo 已装)"
else
  bad "Xcode.app" "没装 → App Store 搜 Xcode（约 12 GB 下载）"
fi
if [ -n "${DEVELOPER_DIR:-}" ]; then
  ok "xcode-select" "指向 $DEV_DIR（本次用 DEVELOPER_DIR=$DEVELOPER_DIR 覆盖，不用 sudo）"
elif [ "$DEV_DIR" = "/Applications/Xcode.app/Contents/Developer" ]; then
  ok "xcode-select" "$DEV_DIR"
else
  warn "xcode-select" "指向 $DEV_DIR → 要么 sudo xcode-select -s …，要么设 DEVELOPER_DIR（脚本会自动找 Xcode）"
fi
SDKS_OUT="$(xcodebuild -showsdks 2>&1)"
if printf '%s' "$SDKS_OUT" | grep -qi "agreed to the Xcode license"; then
  # 这一条最容易被误报成"没装 Xcode"：SDK 其实在，只是协议没同意，所有工具都被锁住
  bad "Xcode 许可协议" "还没同意 → sudo xcodebuild -license accept（要密码，只能你本人跑）"
  bad "iOS SDK" "被协议挡住，先同意协议"
elif printf '%s' "$SDKS_OUT" | grep -q iphoneos; then
  ok "iOS SDK" "$(printf '%s' "$SDKS_OUT" | awk '/iphoneos/ {print $NF; exit}')"
else
  bad "iOS SDK" "没有 → 装完整 Xcode（不是 Command Line Tools）"
fi
if xcrun --find actool >/dev/null 2>&1 && xcrun --find actool 2>/dev/null | grep -q "Xcode.app"; then
  ok "actool（编译图标）" "有"
else
  bad "actool（编译图标）" "没有 → 缺 Xcode，AppIcon 不会被校验"
fi

echo
echo "【模拟器（只影响"能不能看见界面"，不影响出包）】"
RUNTIMES="$(xcrun simctl list runtimes 2>/dev/null | grep -c '^iOS ' || true)"
if [ "${RUNTIMES:-0}" -gt 0 ]; then
  ok "iOS 运行时" "${RUNTIMES} 个"
else
  warn "iOS 运行时" "没有 → xcodebuild -downloadPlatform iOS（约 7 GB）"
fi
if xcrun simctl list devices available 2>/dev/null | grep -q iPhone; then
  ok "可用模拟器" "有"
else
  warn "可用模拟器" "没有可用设备 → xcrun simctl create 'iPhone 15' <runtime-id>"
fi

# ── 3. 签名（只在装真机时需要）──────────────────────────────────────────────
echo
echo "【签名（装真机才要；模拟器出包不需要）】"
ID_COUNT="$(security find-identity -v -p codesigning 2>/dev/null | grep -c 'iPhone\|Apple Development' || true)"
if [ "${ID_COUNT:-0}" -gt 0 ]; then
  ok "开发证书" "${ID_COUNT} 个"
  [ "$VERBOSE" = 1 ] && security find-identity -v -p codesigning | sed -n '2,5p'
else
  warn "开发证书" "没有 → Xcode → Settings → Accounts 登一个 Apple ID（免费账号够用，7 天有效期）"
fi

# ── 4. Rust 侧 ────────────────────────────────────────────────────────────
echo
echo "【Rust】"
CARGO_BIN="${CARGO:-}"
if [ -z "$CARGO_BIN" ] || [ ! -x "$CARGO_BIN" ]; then
  if command -v cargo >/dev/null 2>&1; then
    CARGO_BIN="$(command -v cargo)"
  else
    for cand in "$HOME/.cargo/bin/cargo" /opt/homebrew/bin/cargo /usr/local/bin/cargo; do
      [ -x "$cand" ] && { CARGO_BIN="$cand"; break; }
    done
  fi
fi
if [ -n "${CARGO_BIN:-}" ] && "$CARGO_BIN" -V >/dev/null 2>&1; then
  ok "cargo" "$("$CARGO_BIN" -V 2>/dev/null | cut -d' ' -f2)"
else
  bad "cargo" "没有 → curl https://sh.rustup.rs -sSf | sh（找过 PATH、~/.cargo/bin、/opt/homebrew/bin、/usr/local/bin；也可用 CARGO=/路径 指过来）"
fi
for t in aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios; do
  if [ -n "${CARGO_BIN:-}" ] && rustup target list --installed 2>/dev/null | grep -qx "$t"; then
    ok "target $t" "有"
  else
    # 模拟器那个只在 Intel Mac 上非要不可，缺了先提醒不算硬缺口
    if [ "$t" = "aarch64-apple-ios" ]; then
      bad "target $t" "没有 → rustup target add $t"
    else
      warn "target $t" "没有 → rustup target add $t"
    fi
  fi
done
if [ -f "$ROOT/target/ios/libsim_ios.a" ]; then
  ok "已编好的静态库" "$(du -h "$ROOT/target/ios/libsim_ios.a" | cut -f1)"
else
  warn "已编好的静态库" "还没有（Xcode 构建时会自动编；也可手动 tools/build-ios-rust.sh）"
fi

# ── 5. 工程生成器（可选）────────────────────────────────────────────────────
echo
echo "【可选工具】"
if command -v xcodegen >/dev/null 2>&1; then
  ok "xcodegen" "$(xcodegen --version 2>/dev/null | tr -d '\n')"
else
  note "xcodegen" "没装 —— 不影响出包（IronMind.xcodeproj 已提交）；改了 project.yml 才需要"
fi

# ── 结论 ──────────────────────────────────────────────────────────────────
echo
case "$WORST" in
  0) echo "结论：齐了。tools/build-ios.sh（真机包）与 --simulator（模拟器里看界面）都能跑。" ;;
  2) echo "结论：能出包，但看不见界面。要验 UI 再补模拟器运行时：xcodebuild -downloadPlatform iOS" ;;
  *) echo "结论：还出不了包。先补上面 ❌ 的那几条。" ;;
esac
exit "$WORST"
