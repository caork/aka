export type ThemeMode = "light" | "dark" | "auto";
export type ResolvedTheme = "light" | "dark";

export const THEME_STORAGE_KEY = "aka.themeMode";
export const DEFAULT_THEME_MODE: ThemeMode = "light";

const THEME_MODES: readonly ThemeMode[] = ["light", "dark", "auto"];

export function isThemeMode(value: unknown): value is ThemeMode {
  return typeof value === "string" && THEME_MODES.includes(value as ThemeMode);
}

export function systemTheme(): ResolvedTheme {
  if (
    typeof window !== "undefined" &&
    window.matchMedia?.("(prefers-color-scheme: dark)").matches
  ) {
    return "dark";
  }
  return "light";
}

export function resolveThemeMode(mode: ThemeMode): ResolvedTheme {
  return mode === "auto" ? systemTheme() : mode;
}

export function readThemeMode(): ThemeMode {
  try {
    const stored = window.localStorage.getItem(THEME_STORAGE_KEY);
    return isThemeMode(stored) ? stored : DEFAULT_THEME_MODE;
  } catch {
    return DEFAULT_THEME_MODE;
  }
}

export function persistThemeMode(mode: ThemeMode): void {
  try {
    window.localStorage.setItem(THEME_STORAGE_KEY, mode);
  } catch {
    /* private mode 等场景忽略 */
  }
}

export function applyTheme(mode: ThemeMode, resolved = resolveThemeMode(mode)): void {
  const root = document.documentElement;
  root.dataset.theme = resolved;
  root.dataset.themeMode = mode;
  root.style.setProperty("color-scheme", resolved);
}
