<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { Copy, Minus, Moon, Square, Sun, X } from "@lucide/vue";
import icon from "../icon.svg";
import { theme, syncWindowTheme, toggleTheme } from "./theme";

const emit = defineEmits<{ error: [message: string] }>();
const maximized = ref(false);
let unlistenResize: UnlistenFn | undefined;
let disposed = false;

async function run(action: () => Promise<unknown>) {
  try {
    await action();
  } catch (error) {
    emit("error", `窗口操作失败：${String(error)}`);
  }
}

async function updateMaximized() {
  maximized.value = await getCurrentWindow().isMaximized();
}

async function toggleMaximized() {
  await getCurrentWindow().toggleMaximize();
  await updateMaximized();
}

onMounted(async () => {
  if (!isTauri()) return;
  await run(async () => {
    try {
      await syncWindowTheme();
      await updateMaximized();
      const unlisten = await getCurrentWindow().onResized(() => {
        void run(updateMaximized);
      });
      if (disposed) unlisten();
      else unlistenResize = unlisten;
    } finally {
      await getCurrentWindow().show();
    }
  });
});

onUnmounted(() => {
  disposed = true;
  unlistenResize?.();
});
</script>

<template>
  <header class="window-titlebar" aria-label="窗口标题栏">
    <div class="window-drag-area" data-tauri-drag-region="deep">
      <img :src="icon" alt="" width="16" height="16" draggable="false" />
      <span>CloudDownloader · 云盘下载器</span>
    </div>
    <div class="window-controls">
      <button
        class="window-control theme-toggle"
        role="switch"
        aria-label="Night 主题"
        :aria-checked="theme === 'night'"
        :title="theme === 'night' ? '切换日间主题' : '切换 Night 主题'"
        @click="run(toggleTheme)"
      >
        <Sun v-if="theme === 'night'" :size="16" />
        <Moon v-else :size="16" />
      </button>
      <button
        class="window-control"
        aria-label="最小化"
        title="最小化"
        @click="run(() => getCurrentWindow().minimize())"
      >
        <Minus :size="15" />
      </button>
      <button
        class="window-control"
        :aria-label="maximized ? '还原' : '最大化'"
        :title="maximized ? '还原' : '最大化'"
        @click="run(toggleMaximized)"
      >
        <Copy v-if="maximized" :size="12" />
        <Square v-else :size="12" />
      </button>
      <button
        class="window-control window-close"
        aria-label="关闭窗口"
        title="关闭到托盘"
        @click="run(() => getCurrentWindow().close())"
      >
        <X :size="16" />
      </button>
    </div>
  </header>
</template>
