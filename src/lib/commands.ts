import { invoke } from "@tauri-apps/api/core";
import type {
  AppInfo,
  ProfileSummary,
  PublicError,
  SettingsSnapshot,
} from "./types";

export function isPublicError(value: unknown): value is PublicError {
  return (
    typeof value === "object" &&
    value !== null &&
    "code" in value &&
    "message" in value &&
    typeof value.message === "string"
  );
}

export function formatCommandError(value: unknown): string {
  if (isPublicError(value)) return value.message;
  if (value instanceof Error) return value.message;
  return "The operation could not be completed.";
}

export const commands = {
  getAppInfo: () => invoke<AppInfo>("get_app_info"),
  listProfiles: () => invoke<ProfileSummary[]>("list_profiles"),
  getSettings: () => invoke<SettingsSnapshot>("get_settings"),
};
