#!/usr/bin/env python3
"""S6 长跑素材生成：近静音 WAV（16bit/mono/8kHz/~10s，幅度±100 ≈ -50dBFS）
+ 每文件互异 1200x1200 JPEG 内嵌封面（ID3v2.3 APIC CoverFront，藏于 WAV `id3 ` chunk）。

提取链路依据：crates/hmp-storage/src/local.rs::read_meta（lofty Probe 读 primary tag
的 CoverFront/首图）；hmp_storage::scan::persist_cover 按内容哈希落盘——互异字节必得互异文件，
从而让桌面端 cover_cache（(路径,桶) 键、64MB CAP）在 80 曲换曲中打满并触发 LRU 逐出。

用法：
  python gen_media.py --outdir <dir> --count 80          # 批量
  python gen_media.py --outdir <dir> --count 1 --album-prefix smoke   # 冒烟单文件
"""
import argparse
import io
import os
import random
import struct
import wave


def make_cover_jpeg(seed: int, size: int = 1200) -> bytes:
    """互异封面：双色径向渐变 + 伪随机噪声块，JPEG q=70。"""
    from PIL import Image

    rng = random.Random(seed)
    hue_a = (seed * 37) % 256
    hue_b = (seed * 91 + 77) % 256
    hue_c = (seed * 53 + 160) % 256
    cx, cy = rng.randint(0, size), rng.randint(0, size)

    img = Image.new("RGB", (size, size))
    px = img.load()
    # 网格步进生成（1200x1200 逐像素纯 Python 太慢）：小图渐变 + resize 平滑
    small = size // 8
    simg = Image.new("RGB", (small, small))
    spx = simg.load()
    for y in range(small):
        for x in range(small):
            dx = (x * 8 - cx) / size
            dy = (y * 8 - cy) / size
            d = min(1.0, (dx * dx + dy * dy) ** 0.5)
            r = int(hue_a * (1 - d) + hue_b * d)
            g = int(hue_b * (1 - d) + hue_c * d)
            b = int(hue_c * (1 - d) + hue_a * d)
            spx[x, y] = (r, g, b)
    img = simg.resize((size, size), Image.BILINEAR)
    # 伪随机半透明噪声块（互异性保证 + 视觉可分辨；PIL 矩形绘制，快）
    from PIL import ImageDraw

    overlay = Image.new("RGB", (size, size), (0, 0, 0))
    d_overlay = ImageDraw.Draw(overlay)
    rng2 = random.Random(seed * 7 + 1)
    for _ in range(24):
        x0, y0 = rng2.randint(0, size - 10), rng2.randint(0, size - 10)
        w, h = rng2.randint(10, 200), rng2.randint(10, 200)
        c = (rng2.randint(0, 255), rng2.randint(0, 255), rng2.randint(0, 255))
        d_overlay.rectangle([x0, y0, min(size - 1, x0 + w), min(size - 1, y0 + h)], fill=c)
    img = Image.blend(img, overlay, 0.35)
    # 角标序号（大块互异 + 人眼可辨识）
    draw = ImageDraw.Draw(img)
    draw.rectangle([size - 320, size - 140, size, size], fill=(16, 16, 18))
    draw.text((size - 300, size - 110), f"S6#{seed:03d}", fill=(240, 230, 210))
    buf = io.BytesIO()
    img.save(buf, format="JPEG", quality=70)
    return buf.getvalue()


def syncsafe_bytes(size: int) -> bytes:
    """同步安全整数：每字节仅低 7 位有效、大端拼接（非恒等变换！）。"""
    return struct.pack(
        ">BBBB",
        (size >> 21) & 0x7F,
        (size >> 14) & 0x7F,
        (size >> 7) & 0x7F,
        size & 0x7F,
    )


def id3v2_frame(frame_id: bytes, payload: bytes) -> bytes:
    # ID3v2.4：帧长为 syncsafe 32-bit（实测 lofty 按 syncsafe 语义读帧长；
    # 此前把 OR 组合写回原位=恒等错误，帧长被误读成 ~1/28 → 封面截断 ~1KB）
    return frame_id + syncsafe_bytes(len(payload)) + b"\x00\x00" + payload


def build_id3_tag(title: str, artist: str, album: str, cover_jpeg: bytes) -> bytes:
    frames = b""
    frames += id3v2_frame(b"TIT2", b"\x00" + title.encode("ascii"))
    frames += id3v2_frame(b"TPE1", b"\x00" + artist.encode("ascii"))
    frames += id3v2_frame(b"TALB", b"\x00" + album.encode("ascii"))
    # APIC：编码0(ISO-8859-1) + MIME\0 + 类型0x03(CoverFront) + 描述\0 + JPEG 字节
    frames += id3v2_frame(
        b"APIC", b"\x00image/jpeg\x00\x03\x00" + cover_jpeg
    )
    header = b"ID3" + bytes([4, 0, 0]) + syncsafe_bytes(len(frames))
    return header + frames


def write_wav_with_id3(path: str, seconds: float, tag: bytes) -> None:
    rate, amp = 8000, 100
    n_samples = int(rate * seconds)
    samples = b""
    # 近静音：低幅正弦（-50dBFS 量级），双重声音纪律之一（另一重是 CLI 音量 0）
    import math

    for i in range(n_samples):
        v = int(amp * math.sin(2 * math.pi * 220 * i / rate))
        samples += struct.pack("<h", v)
    fmt = struct.pack("<HHIIHH", 1, 1, rate, rate * 2, 2, 16)
    chunks = b""
    chunks += b"fmt " + struct.pack("<I", len(fmt)) + fmt
    # RIFF 规范：奇数长度 chunk 后补 1 字节（不计入 size 字段）——不补则后续
    # chunk 头错位 1 字节，lofty 标签解析失败（实测 39/80 曲目 album NULL 的根因）
    pad = tag if len(tag) % 2 == 0 else tag + b"\x00"
    chunks += b"id3 " + struct.pack("<I", len(tag)) + pad
    chunks += b"data" + struct.pack("<I", len(samples)) + samples
    riff_size = 4 + len(chunks)
    with open(path, "wb") as f:
        f.write(b"RIFF" + struct.pack("<I", riff_size) + b"WAVE" + chunks)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--outdir", required=True)
    ap.add_argument("--count", type=int, default=80)
    ap.add_argument("--album-prefix", default="s6album")
    ap.add_argument("--seconds", type=float, default=10.0)
    ap.add_argument("--cover-size", type=int, default=1200)
    args = ap.parse_args()

    os.makedirs(args.outdir, exist_ok=True)
    for i in range(1, args.count + 1):
        idx = (i - 1) % 80 + 1
        cover = make_cover_jpeg(idx, args.cover_size)
        tag = build_id3_tag(
            f"s6 track {idx:03d}",
            f"s6 artist {idx % 7 + 1}",
            f"{args.album_prefix} {idx:03d}",
            cover,
        )
        name = f"s6_{args.album_prefix}_{idx:03d}.wav"
        write_wav_with_id3(os.path.join(args.outdir, name), args.seconds, tag)
        print(f"wrote {name} ({os.path.getsize(os.path.join(args.outdir, name))} B, cover {len(cover)} B)")


if __name__ == "__main__":
    main()
