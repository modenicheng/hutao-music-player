export type ThemeMode = "auto" | "light" | "dark";
export type ResolvedTheme = Exclude<ThemeMode, "auto">;

export const THEME_STORAGE_KEY = "hmp.theme.mode";

export const nextThemeMode = (mode: ThemeMode): ThemeMode => {
  if (mode === "auto") return "light";
  if (mode === "light") return "dark";
  return "auto";
};

export const resolveTheme = (
  mode: ThemeMode,
  systemPrefersDark: boolean,
): ResolvedTheme => (mode === "auto" ? (systemPrefersDark ? "dark" : "light") : mode);

export const isThemeMode = (value: string | null): value is ThemeMode =>
  value === "auto" || value === "light" || value === "dark";
