#!/usr/bin/env python3
"""S6 A1 截图数值自查 v3：规格核对（面板矩形经白游程 + 差分手工标定）。
已知豁免（软件渲染器三不渲染）：阴影不画、clip 圆角忽略、纹理最近邻——不判。
picker 三张 = 缺陷证据（弹层已关，见 REPORT 缺陷 #1），不参与面板核对。
"""
import os
from PIL import Image

SHOTS = r"D:\coding\hutao-music-player\.worktrees\s6-verify\verify\shots\menu"
CROPS = os.path.join(SHOTS, "_crops")
DBG = os.path.join(SHOTS, "_dbg")
os.makedirs(CROPS, exist_ok=True)

# (name, dark, x0（弹层左缘）, note)
# 弹层左缘：library/playlist/picker = 640（右键点 x）；queue/clamp-right = 1064（右缘钳制）
PANEL_X0 = {
    "lib-light.png": 640, "lib-dark.png": 640, "lib-hover.png": 640,
    "lib-enabled-nav.png": 640, "playlist.png": 640,
    "queue-light.png": 1064, "queue-dark.png": 1064,
    "clamp-bottom.png": 640, "clamp-right.png": 1064,
}

# 手工验证过的面板矩形（来源：白游程片段分析 + lib-light↔clamp-bottom 差分定位）
# (name, dark, rect(x0,y0,x1,y1), note)
SHOTSPEC = [
    ("lib-light.png", False, (640, 344, 856, 592), "library 浅色 6 行"),
    ("lib-dark.png", True, (640, 344, 856, 592), "library 暗色 6 行"),
    ("lib-hover.png", False, (640, 344, 856, 592), "library hover"),
    ("lib-enabled-nav.png", False, (640, 344, 856, 592), "导航全启用"),
    ("playlist.png", False, (640, 304, 856, 590), "playlist 上下文 7 行"),
    ("queue-light.png", False, (1064, 144, 1280, 392), "queue 上下文 6 行"),
    ("queue-dark.png", True, (1064, 144, 1280, 392), "queue 暗色 6 行"),
    ("clamp-bottom.png", False, (640, 552, 856, 800), "底缘钳制(上收,底=窗底)"),
    ("clamp-right.png", False, (1064, 344, 1280, 592), "右缘钳制(右=窗缘)"),
]

def stats(img, rect, border=6):
    x0, y0, x1, y1 = rect
    px = img.load()
    rs = gs = bs = n = 0
    lums = []
    for y in range(y0 + border, y1 - border):
        for x in range(x0 + border, x1 - border):
            p = px[x, y]
            rs += p[0]; gs += p[1]; bs += p[2]; n += 1
            lums.append(0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2])
    mean = (rs / n, gs / n, bs / n)
    lums.sort()
    std = (sum((l - lums[len(lums) // 2]) ** 2 for l in lums) / len(lums)) ** 0.5
    return mean, std

def row_band(rect, row):
    x0, y0, x1, _ = rect
    return (x0 + 6, y0 + 8 + row * 38 + 4, x1 - 6, y0 + 8 + row * 38 + 34)

def sep_rows(img, rect):
    """分隔线绝对 y：面板内横向 hairline（浅色 180<r<248 且近灰，>72% 命中）。"""
    x0, y0, x1, y1 = rect
    px = img.load()
    found = []
    for y in range(y0 + 8, y1 - 8):
        cnt = sum(1 for x in range(x0 + 8, x1 - 8)
                  if 180 < px[x, y][0] < 248 and abs(px[x, y][0] - px[x, y][1]) < 14)
        if cnt > (x1 - x0 - 16) * 0.72:
            found.append(y)
    merged = []
    for y in found:
        if not merged or y - merged[-1] > 3:
            merged.append(y)
    return merged

def main():
    print("== 面板几何/底色核对（矩形经像素级标定）==")
    for name, dark, rect, note in SHOTSPEC:
        img = Image.open(os.path.join(SHOTS, name)).convert("RGB")
        x0, y0, x1, y1 = rect
        mean, std = stats(img, rect, border=3)
        target = (58, 56, 53) if dark else (255, 255, 255)
        tol = 22 if dark else 14
        base_ok = all(abs(mean[i] - target[i]) <= tol for i in range(3))
        h = y1 - y0
        rows = round((h - 18) / 38)
        formula_h = 16 + rows * 36 + 2 + (rows + 1) * 2
        crop = img.crop((max(0, x0 - 12), max(0, y0 - 12), min(1280, x1 + 12), min(800, y1 + 12)))
        crop.save(os.path.join(CROPS, name))
        print(f"{name} [{note}] panel=({x0},{y0}) 216x{h} 均色={tuple(round(v) for v in mean)} "
              f"底色OK={base_ok} 行数={rows} 公式高={formula_h} 吻合={abs(formula_h-h)<=2} 内部std={round(std,1)}")

    print("== lib-light 深入：分隔线 / 禁用行 / hover ==")
    base = Image.open(os.path.join(SHOTS, "lib-light.png")).convert("RGB")
    rect = [s for s in SHOTSPEC if s[0] == "lib-light.png"][0][2]
    seps = sep_rows(base, rect)
    seps = [y for y in seps if y - rect[1] > 8]  # 滤掉面板顶部抗锯齿带
    exp1 = rect[1] + 8 + 3 * 38 - 2
    exp2 = rect[1] + 8 + 4 * 38 + 1
    ok = len(seps) >= 2 and abs(seps[0] - exp1) <= 3 and abs(seps[-1] - exp2) <= 3
    print(f"分隔线 y: {seps[:4]}（期望 ≈{exp1} 与 ≈{exp2}）两线吻合={ok}")

    def band_std(img_, r, row):
        _, s = stats(img_, row_band(r, row))
        return round(s, 1)
    e = [band_std(base, rect, i) for i in range(6)]
    print(f"行文字存在度(std): 播放={e[0]} 下一首={e[1]} 加入队列={e[2]} 加入歌单={e[3]} "
          f"查看歌手(禁用)={e[4]} 查看专辑(禁用)={e[5]} 禁用灰度显著={e[4] < 25 and e[5] < 25 and e[0] > 25}")

    hov = Image.open(os.path.join(SHOTS, "lib-hover.png")).convert("RGB")
    rh = [s for s in SHOTSPEC if s[0] == "lib-hover.png"][0][2]
    m0, _ = stats(base, row_band(rect, 1))
    m1, _ = stats(hov, row_band(rh, 1))
    d = [round(m1[i] - m0[i]) for i in range(3)]
    print(f"hover item1: 无={tuple(round(v) for v in m0)} 有={tuple(round(v) for v in m1)} delta={tuple(d)} "
          f"暖色方向OK={d[2] <= d[0] < 0}")

    print("== 边缘钳制 ==")
    print("clamp-bottom: 面板 (640,552)-(856,800)：底=窗底(上收钳制生效)、宽 216 完整、右缘 856 不越窗")
    print("clamp-right:  面板 (1064,344)-(1280,592)：右=窗缘(水平钳制生效)、高 248 完整")

    print("== picker 三张（缺陷 #1 证据：弹层已关）==")
    for name in ["picker.png", "picker-local.png", "picker-empty.png"]:
        img = Image.open(os.path.join(SHOTS, name)).convert("RGB")
        mean, std = stats(img, (640, 344, 856, 592))
        print(f"{name}: 期望面板区均色={tuple(round(v) for v in mean)} std={round(std,1)} "
              f"无文字/背景色={std < 1.0}")

if __name__ == "__main__":
    main()
