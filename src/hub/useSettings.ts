import { createContext, useContext } from "react";
import type { Settings } from "../shared/api";

export interface SettingsCtx {
  settings: Settings | null;
  /** Saves the whole settings object (optimistic). */
  update: (next: Settings) => Promise<void>;
}

export const SettingsContext = createContext<SettingsCtx>({
  settings: null,
  update: async () => {},
});

export function useSettings() {
  return useContext(SettingsContext);
}
