/** Tauri v2 运行时探测：注入脚本会留下 __TAURI_INTERNALS__ */
export function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}
