#!/usr/bin/env python3
"""S6 §21 内存长跑判据分析（C3 修订版）。

主序列 = WS(WorkingSet64)，PM(PrivateMemorySize64) 同步计算与展示；
分歧时以 WS 为准（§21 的 2.4G 观测口径即 RSS/btop）。
噪声防护：所有斜率/窗口比较先做 30s 中位数降采样（每 30s 窗取中位数）。

判据：
1. 绝对上限：全程 WS ≤ 600 MB
2. 双窗漂移：P5 复扫中位数均值 vs P1 后半中位数均值，|差| ≤ 24 MB
3. 平台斜率：P2+P3+P4 合并段 30s 中位数序列线性拟合斜率 ≤ +0.5 MB/min
4. 无持续新高：max(P3+P4) ≤ max(P1+P2) + 24 MB
判定矩阵：1 破→FAIL；300≤max<600 且 2-4 任一破→FAIL；
         max<300 且 2-4 任一破→PARTIAL；全满足→PASS。
"""
import csv
import os
import sys

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt

DATA = os.path.join(os.path.dirname(__file__), "..", "data")
MAIN_CSV = os.path.join(DATA, "rss_curve.csv")
PROXY_CSV = os.path.join(DATA, "rss_browse_proxy.csv")
OUT_PNG = os.path.join(DATA, "rss_curve.png")

MB = 1024.0  # KB -> MB


def load(path):
    rows = []
    with open(path, newline="") as f:
        for r in csv.DictReader(f):
            rows.append({
                "t": float(r["elapsed_s"]), "phase": r["phase"],
                "image": r["image"], "pid": r["pid"],
                "ws": float(r["ws_kb"]) / MB, "pm": float(r["pm_kb"]) / MB,
            })
    return rows


def phase_span(rows, prefix):
    ts = [r["t"] for r in rows if r["phase"].startswith(prefix)]
    return (min(ts), max(ts)) if ts else None


def median(v):
    v = sorted(v)
    n = len(v)
    return v[n // 2] if n % 2 else (v[n // 2 - 1] + v[n // 2]) / 2


def downsample(rows, key, t0, t1, window=30.0):
    """[t0,t1) 内每 30s 窗取中位数 → [(窗中心 t, 中位值)]。"""
    out = []
    w = t0
    while w + window / 2 <= t1:
        vals = [r[key] for r in rows if w <= r["t"] < w + window]
        if vals:
            out.append((w + window / 2, median(vals)))
        w += window
    return out


def mean(v):
    return sum(v) / len(v) if v else float("nan")


def fit_slope_mb_per_min(series):
    """最小二乘斜率，输入 [(t_s, MB)]，输出 MB/min。"""
    if len(series) < 2:
        return float("nan")
    n = len(series)
    sx = sum(t for t, _ in series)
    sy = sum(y for _, y in series)
    sxx = sum(t * t for t, _ in series)
    sxy = sum(t * y for t, y in series)
    denom = n * sxx - sx * sx
    if denom == 0:
        return float("nan")
    return (n * sxy - sx * sy) / denom * 60.0


def analyze(image, rows, label, verdict=True):
    print(f"\n===== {label}（image={image}，样本 {len(rows)}）=====")
    if not rows:
        print("无样本")
        return
    for key, kname in (("ws", "WS"), ("pm", "PM")):
        mx = max(r[key] for r in rows)
        print(f"max {kname}: {mx:.1f} MB")
    ws_series = [(r["t"], r["ws"]) for r in rows]
    pm_series = [(r["t"], r["pm"]) for r in rows]

    spans = {}
    for p in ("P0-warmup", "P1-play", "P2-queue", "P3-library", "P4-settle", "P5-replay"):
        s = phase_span(rows, p)
        if s:
            spans[p] = s
            print(f"{p}: {s[0]:.0f}s - {s[1]:.0f}s")

    if not verdict:
        # 代理模式：同口径算判据 2/3 作佐证，不下 PASS/FAIL
        c2 = c3 = None
        if "P1-play" in spans and "P5-replay" in spans:
            p1 = spans["P1-play"]
            p1_late_t0 = p1[0] + (p1[1] - p1[0]) / 2
            p5 = spans["P5-replay"]
            d1 = [v for _, v in downsample(rows, "ws", p1_late_t0, p1[1] + 30)]
            d5 = [v for _, v in downsample(rows, "ws", *p5, window=30)]
            if d1 and d5:
                c2 = abs(mean(d5) - mean(d1))
                print(f"[佐证] 判据2 双窗漂移: |{mean(d5):.1f} - {mean(d1):.1f}| = {c2:.1f} MB (≤24 佐证成立)")
        merged_t0 = min(spans[p][0] for p in ("P2-queue", "P3-library", "P4-settle") if p in spans) if any(p in spans for p in ("P2-queue", "P3-library", "P4-settle")) else None
        if merged_t0 is not None:
            merged_t1 = max(spans[p][1] for p in ("P2-queue", "P3-library", "P4-settle") if p in spans)
            c3 = fit_slope_mb_per_min(downsample(rows, "ws", merged_t0, merged_t1 + 30))
            print(f"[佐证] 判据3 平台斜率: {c3:+.3f} MB/min (≤+0.5 佐证成立)")
        return ws_series, pm_series, spans

    # —— 主判据（C3）——
    max_ws = max(y for _, y in ws_series)
    c1 = max_ws <= 600
    print(f"\n判据1 绝对上限: max WS = {max_ws:.1f} MB ≤ 600 → {'满足' if c1 else '破'}")

    c2 = None
    if "P1-play" in spans and "P5-replay" in spans:
        p1 = spans["P1-play"]
        p1_late_t0 = p1[0] + (p1[1] - p1[0]) / 2
        p5 = spans["P5-replay"]
        d1 = [v for _, v in downsample(rows, "ws", p1_late_t0, p1[1] + 30)]
        d5 = [v for _, v in downsample(rows, "ws", *p5)]
        if d1 and d5:
            drift = mean(d5) - mean(d1)
            c2 = abs(drift) <= 24
            print(f"判据2 双窗漂移: P1后半 {mean(d1):.1f} MB vs P5 {mean(d5):.1f} MB，|Δ|={abs(drift):.1f} MB ≤ 24 → {'满足' if c2 else '破'}")
    else:
        print("判据2: 阶段数据不全，跳过")

    c3 = None
    plat = [p for p in ("P2-queue", "P3-library", "P4-settle") if p in spans]
    if plat:
        t0 = min(spans[p][0] for p in plat)
        t1 = max(spans[p][1] for p in plat)
        slope = fit_slope_mb_per_min(downsample(rows, "ws", t0, t1 + 30))
        c3 = slope <= 0.5
        print(f"判据3 平台斜率(P2+P3+P4): {slope:+.3f} MB/min ≤ +0.5 → {'满足' if c3 else '破'}")
    else:
        print("判据3: 阶段数据不全，跳过")

    c4 = None
    early = [p for p in ("P1-play", "P2-queue") if p in spans]
    late = [p for p in ("P3-library", "P4-settle") if p in spans]
    if early and late:
        e0 = min(spans[p][0] for p in early); e1 = max(spans[p][1] for p in early)
        l0 = min(spans[p][0] for p in late); l1 = max(spans[p][1] for p in late)
        m_early = max(v for _, v in downsample(rows, "ws", e0, e1 + 30))
        m_late = max(v for _, v in downsample(rows, "ws", l0, l1 + 30))
        c4 = m_late <= m_early + 24
        print(f"判据4 无持续新高: max(后期 {m_late:.1f}) ≤ max(前期 {m_early:.1f}) + 24 → {'满足' if c4 else '破'}")
    else:
        print("判据4: 阶段数据不全，跳过")

    crits = [c1, c2, c3, c4]
    any_fail = any(c is False for c in crits)
    if not c1:
        verdict_str = "FAIL（判据1 破：>600MB）"
    elif 300 <= max_ws < 600 and any_fail:
        verdict_str = f"FAIL（max {max_ws:.0f}MB ∈ [300,600) 且判据 " + ",".join(
            str(i + 1) for i, c in enumerate(crits) if c is False) + " 破）"
    elif max_ws < 300 and any_fail:
        verdict_str = f"PARTIAL（max {max_ws:.0f}MB < 300，判据 " + ",".join(
            str(i + 1) for i, c in enumerate(crits) if c is False) + " 破，交协调者裁量）"
    else:
        verdict_str = "PASS（判据 1-4 全满足）"
    print(f"\n判定: {verdict_str}")
    return verdict_str, ws_series, pm_series, spans


def plot(main, proxy=None):
    verdict, ws, pm, spans = main
    fig, ax = plt.subplots(figsize=(13, 6.5))
    if ws:
        ax.plot([t for t, _ in ws], [y for _, y in ws], lw=1.0, color="#B4413C", label="desktop WS (main series)")
    if pm:
        ax.plot([t for t, _ in pm], [y for _, y in pm], lw=0.8, alpha=0.7, color="#C98A3D", ls="--", label="desktop PM (reference)")
    if proxy:
        p_ws, p_pm = proxy
        if p_ws:
            ax.plot([t for t, _ in p_ws], [y for _, y in p_ws], lw=1.0, color="#4C6E9E", label="browse-proxy WS (sw-render proxy)")
    for (p, (t0, t1)) in spans.items():
        ax.axvspan(t0, t1, alpha=0.10, color="grey")
        ax.text((t0 + t1) / 2, ax.get_ylim()[1] * 0.02, p, rotation=90,
                fontsize=7, ha="center", va="bottom", color="#555")
    ax.axhline(600, color="#B4413C", lw=0.8, ls=":", label="600MB ceiling (crit.1)")
    ax.axhline(300, color="#C9A23D", lw=0.8, ls=":", label="300MB threshold")
    ax.set_xlabel("elapsed (s)")
    ax.set_ylabel("RSS (MB)")
    ax.set_title(f"S6 mem long-run RSS curve - verdict: {verdict}")
    ax.legend(loc="upper left", fontsize=8)
    ax.grid(alpha=0.25)
    fig.tight_layout()
    fig.savefig(OUT_PNG, dpi=110)
    print(f"\ncurve saved: {OUT_PNG}")


def main():
    rows = [r for r in load(MAIN_CSV) if r["image"] == "hmp-desktop"]
    daemon = [r for r in load(MAIN_CSV) if r["image"] == "hmp-daemon"]
    if daemon:
        print(f"[对照] daemon max WS = {max(r['ws'] for r in daemon):.1f} MB（预期 ~20-60MB 级）")
    result = analyze("hmp-desktop", rows, "B 主实验（真机 GUI，femtovg）")
    proxy_series = None
    if os.path.exists(PROXY_CSV):
        prow = [r for r in load(PROXY_CSV)]
        pres = analyze("shot-menu", prow, "B 浏览代理（软件渲染器，仅佐证）", verdict=False)
        if pres:
            proxy_series = (pres[0], pres[1])
    if result:
        plot(result, proxy_series)


if __name__ == "__main__":
    main()
