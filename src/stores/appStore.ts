import { create } from "zustand";
import { commands, formatCommandError } from "../lib/commands";
import type { AppInfo, ProfileSummary, SettingsSnapshot } from "../lib/types";

interface AppStore {
  appInfo: AppInfo | null;
  profiles: ProfileSummary[];
  settings: SettingsSnapshot | null;
  loading: boolean;
  error: string | null;
  bootstrap: () => Promise<void>;
}

export const useAppStore = create<AppStore>((set) => ({
  appInfo: null,
  profiles: [],
  settings: null,
  loading: false,
  error: null,
  bootstrap: async () => {
    set({ loading: true, error: null });
    try {
      const [appInfo, profiles, settings] = await Promise.all([
        commands.getAppInfo(),
        commands.listProfiles(),
        commands.getSettings(),
      ]);
      set({ appInfo, profiles, settings, loading: false });
    } catch (error) {
      set({ loading: false, error: formatCommandError(error) });
    }
  },
}));
