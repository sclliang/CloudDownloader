import { ref } from "vue";
import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

export type AppTheme = "light" | "night";
const storageKey = "clouddownloader.theme";

function savedTheme(): AppTheme {
  try {
    return localStorage.getItem(storageKey) === "night" ? "night" : "light";
  } catch {
    return "light";
  }
}

export const theme = ref<AppTheme>(savedTheme());

export function applyTheme() {
  document.documentElement.dataset.theme = theme.value;
  document.documentElement.style.colorScheme =
    theme.value === "night" ? "dark" : "light";
}

export async function syncWindowTheme() {
  if (!isTauri()) return;
  const window = getCurrentWindow();
  await window.setTheme(theme.value === "night" ? "dark" : "light");
  await window.setBackgroundColor(
    theme.value === "night" ? "#171c24" : "#eaf5fc",
  );
}

export async function toggleTheme() {
  theme.value = theme.value === "night" ? "light" : "night";
  applyTheme();
  try {
    localStorage.setItem(storageKey, theme.value);
  } catch {
    // Restricted storage must not prevent changing the current theme.
  }
  await syncWindowTheme();
}
