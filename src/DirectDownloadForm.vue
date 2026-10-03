<script setup lang="ts">
import { onBeforeUnmount, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { ArrowDownToLine, LoaderCircle, Zap } from "@lucide/vue";

const emit = defineEmits<{
  created: [task: { status: string; error: string }];
  error: [message: string];
  busy: [value: boolean];
}>();
const url = ref("");
const filename = ref("");
const connections = ref(8);
const busy = ref(false);
const input = ref<HTMLInputElement | null>(null);
defineExpose({ focus: () => input.value?.focus() });
onBeforeUnmount(() => emit("busy", false));

async function download() {
  if (busy.value || !url.value.trim()) return;
  busy.value = true;
  emit("busy", true);
  emit("error", "");
  try {
    const task = await invoke<{ status: string; error: string }>(
      "enqueue_direct",
      {
        url: url.value.trim(),
        filename: filename.value.trim(),
        connections: connections.value,
      },
    );
    emit("created", task);
    if (task.status !== "error") {
      url.value = "";
      filename.value = "";
    }
  } catch (error) {
    emit("error", String(error));
  } finally {
    busy.value = false;
    emit("busy", false);
  }
}
</script>

<template>
  <form class="direct-download-form" @submit.prevent="download">
    <label for="direct-url">文件链接</label>
    <input
      id="direct-url"
      ref="input"
      v-model="url"
      type="url"
      required
      placeholder="https://example.com/file.zip"
      spellcheck="false"
      autocomplete="off"
      :disabled="busy"
      aria-label="普通下载链接"
    />
    <div class="direct-options">
      <label class="direct-filename"
        >文件名（可选）
        <input
          v-model="filename"
          maxlength="180"
          placeholder="自动识别文件名"
          :disabled="busy"
        />
      </label>
      <label
        >加速连接
        <select
          v-model.number="connections"
          :disabled="busy"
          aria-label="下载连接数"
        >
          <option :value="1">1 路 · 单连接</option>
          <option :value="4">最多 4 路</option>
          <option :value="8">最多 8 路（推荐）</option>
          <option :value="16">最多 16 路</option>
        </select>
      </label>
      <button class="primary" type="submit" :disabled="busy || !url.trim()">
        <LoaderCircle v-if="busy" :size="16" class="spinning" />
        <ArrowDownToLine v-else :size="16" />{{
          busy ? "正在识别链接…" : "开始下载"
        }}
      </button>
    </div>
    <p class="direct-download-hint">
      <Zap :size="13" />支持分段时多路连接加速，实际速度取决于服务器和网络。
    </p>
  </form>
</template>
