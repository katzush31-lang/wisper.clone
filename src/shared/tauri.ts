// Thin wrappers around the Tauri API that also work in a plain browser (dev / tests),
// where they fall back to harmless no-ops.
import type { EventCallback, UnlistenFn } from "@tauri-apps/api/event";

export const isTauri = (): boolean =>
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) throw new Error(`Not running inside the app (${cmd})`);
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args);
}

export async function listen<T>(event: string, cb: EventCallback<T>): Promise<UnlistenFn> {
  if (!isTauri()) return () => {};
  const mod = await import("@tauri-apps/api/event");
  return mod.listen<T>(event, cb);
}

export async function currentWindow() {
  if (!isTauri()) return null;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  return getCurrentWindow();
}

export async function openUrl(url: string) {
  if (!isTauri()) {
    window.open(url, "_blank");
    return;
  }
  const { openUrl } = await import("@tauri-apps/plugin-opener");
  await openUrl(url);
}
