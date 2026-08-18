import { describe, expect, it } from "vitest";
import {
  nextThemeMode,
  resolveTheme,
  type ThemeMode,
} from "./theme";

describe("theme mode", () => {
  it("cycles through auto, light, and dark", () => {
    const modes: ThemeMode[] = ["auto", "light", "dark"];

    expect(modes.map(nextThemeMode)).toEqual(["light", "dark", "auto"]);
  });

  it("resolves auto mode from the system preference", () => {
    expect(resolveTheme("auto", true)).toBe("dark");
    expect(resolveTheme("auto", false)).toBe("light");
    expect(resolveTheme("light", true)).toBe("light");
    expect(resolveTheme("dark", false)).toBe("dark");
  });
});
