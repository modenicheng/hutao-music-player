// 展示层格式化助手：文件大小 / 长时长 / 金额。
// 纯函数、确定性输出；展示时数字一律配合 tabular-nums 使用（DESIGN.md §3.0）。

function trimOne(value: number): string {
  const fixed = value.toFixed(1);
  return fixed.endsWith(".0") ? fixed.slice(0, -2) : fixed;
}

/** 字节数 → 人读大小：≥1 GB 以 GB 计，否则以 MB 计；一位小数，整值去尾 ".0" */
export function formatBytes(bytes: number): string {
  const gb = bytes / 1024 ** 3;
  if (gb >= 1) return `${trimOne(gb)} GB`;
  return `${trimOne(bytes / 1024 ** 2)} MB`;
}

/** 毫秒 → 长时长：一小时内 "46 分钟"，跨小时 "3 小时 42 分钟"（与队列抽屉同文案） */
export function formatLongDuration(ms: number): string {
  const minutes = Math.round(ms / 60_000);
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return hours > 0 ? `${hours} 小时 ${rest} 分钟` : `${rest} 分钟`;
}

/** 分 → 金额文案："¥36.00"（恒两位小数） */
export function formatCny(fen: number): string {
  return `¥${(fen / 100).toFixed(2)}`;
}
