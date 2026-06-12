import { invoke } from "@tauri-apps/api/core";

export function isDesktopRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export async function invokeDesktop<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  return invoke<T>(command, args);
}

export function asDesktopError(e: unknown, fallback: string): Error {
  if (typeof e === "string" && e.trim()) return new Error(e);
  if (e instanceof Error) return e;
  return new Error(fallback);
}

export async function openEditorUrl(url: string): Promise<void> {
  if (isDesktopRuntime()) {
    await invokeDesktop("open_editor_url", { url });
    return;
  }
  window.location.href = url;
}
