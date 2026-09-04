// DOM 适配层：封面 URL → 64×64 降采样像素 → extractPalette。
// 这是模块里唯一允许触碰 DOM/Canvas 的文件，纯算法部分保持可单测、可替换。
import { extractPalette, type ExtractOptions, type TrackPalette } from "./index";

const SAMPLE_SIZE = 64;

export const extractPaletteFromUrl = async (
  url: string,
  options: ExtractOptions = {},
): Promise<TrackPalette> => {
  try {
    const image = await loadImage(url);
    return extractPalette(samplePixels(image), options);
  } catch {
    // 加载/绘制失败不抛错：调用方（UI 层）永远拿得到可用的回退调色板。
    // 跨域图片不在处理范围（约定同源或 data: URL）。
    return extractPalette(new Uint8ClampedArray(0), options);
  }
};

const loadImage = (url: string): Promise<HTMLImageElement> =>
  new Promise((resolve, reject) => {
    const image = new Image();
    image.onload = () => resolve(image);
    image.onerror = () => reject(new Error(`封面图片加载失败：${url}`));
    // data: URL（程序化 SVG 封面）与同源资源均可直接绘制，SVG 不含外部资源
    image.src = url;
  });

const samplePixels = (image: HTMLImageElement): Uint8ClampedArray => {
  // 64×64 足以保住主色结构，同时把量化成本压到常数级
  if (typeof OffscreenCanvas !== "undefined") {
    const canvas = new OffscreenCanvas(SAMPLE_SIZE, SAMPLE_SIZE);
    const context = canvas.getContext("2d");
    if (!context) throw new Error("OffscreenCanvas 2D 上下文不可用");
    context.drawImage(image, 0, 0, SAMPLE_SIZE, SAMPLE_SIZE);
    return context.getImageData(0, 0, SAMPLE_SIZE, SAMPLE_SIZE).data;
  }
  const canvas = document.createElement("canvas");
  canvas.width = SAMPLE_SIZE;
  canvas.height = SAMPLE_SIZE;
  const context = canvas.getContext("2d");
  if (!context) throw new Error("Canvas 2D 上下文不可用");
  context.drawImage(image, 0, 0, SAMPLE_SIZE, SAMPLE_SIZE);
  return context.getImageData(0, 0, SAMPLE_SIZE, SAMPLE_SIZE).data;
};
