#!/usr/bin/env python3
# make-icon-ios.py —— 生成 iOS 的 App 图标（全套尺寸 + Contents.json）。
#
# 为什么要有脚本而不是塞一堆 png：同一个徽记要在 18 个尺寸上出现（iPhone / iPad / App Store），
# 手导出一次就会有一两张对不上；配色改了更没人愿意重导一遍。
# 这里的画法**照抄桌面版**（tools/make-icon.ps1）：深蓝渐变底 + 金边 + 一个「钢」，
# 桌面窗口、安装包快捷方式、手机图标因此是同一个东西。
#
# 和桌面版唯一的区别：iOS 的图标必须是一块**不透明正方形**，圆角由系统裁。
# 所以这里铺满整个画布，只把金边画成圆角——不是画一个圆角矩形再留透明角。
#
# 用法：
#   python3 tools/make-icon-ios.py                 # 写到 ios/IronMind/Assets.xcassets/AppIcon.appiconset
#   python3 tools/make-icon-ios.py --out 某个目录
#
# 依赖 Pillow（`pip install pillow`）。字体用系统自带的黑体，找不到就换一个，
# 再找不到就退到默认字体——**绝不能因为缺字体而失败**，那样改一行配色都画不出图。

import argparse
import json
import os
import sys

from PIL import Image, ImageDraw, ImageFont

# 和桌面版、网页同一套颜色
BG_TOP = (0x20, 0x30, 0x3F)
BG_BOTTOM = (0x0B, 0x0E, 0x12)
GOLD = (0xE3, 0xB0, 0x4B)
GLYPH = "钢"

FONT_CANDIDATES = [
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
    "/System/Library/Fonts/STHeiti Medium.ttc",
    "/Library/Fonts/Arial Unicode.ttf",
    "/System/Library/Fonts/Supplemental/Songti.ttc",
    "C:/Windows/Fonts/msyhbd.ttc",
    "C:/Windows/Fonts/simhei.ttf",
]

# (idiom, size_in_points, scale) —— 经典全套，任何 Xcode 版本都认
ICON_SET = [
    ("iphone", 20, 2), ("iphone", 20, 3),
    ("iphone", 29, 2), ("iphone", 29, 3),
    ("iphone", 40, 2), ("iphone", 40, 3),
    ("iphone", 60, 2), ("iphone", 60, 3),
    ("ipad", 20, 1), ("ipad", 20, 2),
    ("ipad", 29, 1), ("ipad", 29, 2),
    ("ipad", 40, 1), ("ipad", 40, 2),
    # 76@1x 只对 iOS 10 之前的 iPad 有意义，actool 会为此发一条 notice，去掉
    ("ipad", 76, 2),
    ("ipad", 83.5, 2),
    ("ios-marketing", 1024, 1),
]


def load_font(px: int) -> ImageFont.FreeTypeFont:
    for path in FONT_CANDIDATES:
        if not os.path.exists(path):
            continue
        try:
            return ImageFont.truetype(path, px, index=0)
        except Exception:
            continue
    print("警告：没找到中文字体，图标上的字可能缺字形", file=sys.stderr)
    return ImageFont.load_default()


def draw_icon(size: int) -> Image.Image:
    img = Image.new("RGB", (size, size), BG_BOTTOM)
    d = ImageDraw.Draw(img)

    # 底：竖向渐变（逐行填，够快也够准；小图标上不会出现色带）
    for y in range(size):
        t = y / max(1, size - 1)
        color = tuple(round(BG_TOP[i] + (BG_BOTTOM[i] - BG_TOP[i]) * t) for i in range(3))
        d.line([(0, y), (size, y)], fill=color)

    # 金边：圆角矩形，线宽按尺寸缩放（20px 的图标上也得看得见）
    width = max(1, round(size * 0.035))
    inset = round(size * 0.06) + width / 2
    radius = max(1, round(size * 0.15))
    d.rounded_rectangle(
        [inset, inset, size - inset, size - inset],
        radius=radius,
        outline=GOLD,
        width=width,
    )

    # 字：按实际字形框居中（用 getbbox 而不是"字号一半"那种估算，
    # 汉字和拉丁字母的基线不一样，估出来的会偏下）
    px = round(size * 0.46)
    font = load_font(px)
    box = d.textbbox((0, 0), GLYPH, font=font)
    x = (size - (box[2] - box[0])) / 2 - box[0]
    y = (size - (box[3] - box[1])) / 2 - box[1]
    d.text((x, y), GLYPH, font=font, fill=GOLD)
    return img


def icon_name(idiom: str, pts: float, scale: int) -> str:
    """文件名带上 idiom：iPhone 的 20@2x 和 iPad 的 20@2x 是同一张 40px 图，
    但**不能**共用一个文件名——actool 见到两个 entry 指同一个文件会挑刺。"""
    return f"icon-{idiom}-{pts:g}@{scale}x.png"


def contents_json() -> dict:
    images = []
    for idiom, pts, scale in ICON_SET:
        images.append({
            "filename": icon_name(idiom, pts, scale),
            "idiom": idiom,
            "scale": f"{scale}x",
            "size": f"{pts:g}x{pts:g}",
        })
    return {"images": images, "info": {"author": "xcode", "version": 1}}


def main() -> int:
    here = os.path.dirname(os.path.abspath(__file__))
    default_out = os.path.join(here, "..", "ios", "IronMind", "Assets.xcassets", "AppIcon.appiconset")
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=default_out)
    args = ap.parse_args()

    out = os.path.abspath(args.out)
    os.makedirs(out, exist_ok=True)

    for idiom, pts, scale in ICON_SET:
        draw_icon(round(pts * scale)).save(os.path.join(out, icon_name(idiom, pts, scale)),
                                           "PNG", optimize=True)

    with open(os.path.join(out, "Contents.json"), "w", encoding="utf-8") as f:
        json.dump(contents_json(), f, ensure_ascii=False, indent=2)
        f.write("\n")

    print(f"图标：{len(ICON_SET)} 个 → {out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
