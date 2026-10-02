<script setup lang="ts">
import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import {
  ArrowUpFromLine,
  File,
  Folder,
  Pause,
  Play,
  RefreshCw,
  Trash2,
} from "@lucide/vue";
import { type UploadTask, formatBytes } from "./cloud";
const props = defineProps<{ tasks: UploadTask[] }>();
const emit = defineEmits<{
  changed: [];
  error: [message: string];
  browse: [];
}>();
const busy = ref("");
const filter = ref("all");
const removeTarget = ref<UploadTask | null>(null);
const rows = computed(() =>
  props.tasks.filter(
    (t) => filter.value === "all" || t.status === filter.value,
  ),
);
const labels: Record<string, string> = {
  waiting: "排队中",
  hashing: "校验本地文件",
  active: "上传中",
  merging: "云端合并中",
  paused: "已暂停",
  complete: "已完成",
  error: "上传失败",
};
const running = (t: UploadTask) =>
  ["waiting", "hashing", "active", "merging"].includes(t.status);
const speed = computed(() => props.tasks.reduce((n, t) => n + t.speed, 0));
const remaining = (t: UploadTask) => {
  const seconds = Math.max(1, Math.ceil((t.size - t.completed) / t.speed));
  return seconds < 60
    ? `约 ${seconds} 秒`
    : seconds < 3600
      ? `约 ${Math.ceil(seconds / 60)} 分钟`
      : `约 ${(seconds / 3600).toFixed(1)} 小时`;
};
async function action(t: UploadTask, action: string) {
  busy.value = t.id;
  try {
    await invoke("upload_action", { id: t.id, action });
    emit("changed");
  } catch (e) {
    emit("error", String(e));
  } finally {
    busy.value = "";
  }
}
async function remove() {
  const task = removeTarget.value;
  removeTarget.value = null;
  if (task) await action(task, "remove");
}
</script>
<template>
  <div class="cloud-page-toolbar">
    <div class="segmented">
      <button
        v-for="[key, label] in [
          ['all', '全部'],
          ['complete', '已完成'],
          ['paused', '已暂停'],
          ['error', '失败'],
        ]"
        :key="key"
        :class="{ selected: filter === key }"
        @click="filter = key!"
      >
        {{ label }}
      </button>
    </div>
    <span v-if="speed" class="speed-label"
      ><ArrowUpFromLine :size="15" />{{ formatBytes(speed) }}/s</span
    ><button class="primary" @click="emit('browse')">
      <Folder :size="16" />选择上传位置
    </button>
  </div>
  <section class="task-list card">
    <div v-if="!rows.length" class="empty-state">
      <span class="empty-illustration"><ArrowUpFromLine :size="40" /></span>
      <h3>{{ tasks.length ? "这里没有匹配的任务" : "把本地文件带到云端" }}</h3>
      <p>在「我的网盘」中打开目标文件夹，然后选择上传文件。</p>
    </div>
    <article v-for="task in rows" :key="task.id" class="task-row">
      <div class="task-file-icon"><File :size="25" /></div>
      <div class="task-details">
        <div class="task-name-row">
          <strong :title="task.localPath">{{ task.name }}</strong
          ><span class="status-pill" :class="task.status"
            ><i />{{ labels[task.status] || task.status }}</span
          >
        </div>
        <div class="task-progress" :class="task.status">
          <div
            :style="{
              width: `${task.status === 'complete' ? 100 : task.size ? Math.min(99.9, (task.completed / task.size) * 100) : 0}%`,
            }"
          />
        </div>
        <div class="task-meta">
          <span class="provider-tag">百度网盘</span
          ><span
            >{{ formatBytes(task.completed) }} /
            {{ formatBytes(task.size) }}</span
          ><span>{{ task.concurrency }} 路并发</span
          ><span v-if="task.totalParts"
            >分片 {{ task.completedParts }} / {{ task.totalParts }}</span
          ><span v-if="task.speed"
            >{{ formatBytes(task.speed) }}/s · {{ remaining(task) }}</span
          >
        </div>
        <p class="upload-destination" :title="task.remotePath">
          上传到 {{ task.remotePath }}
        </p>
        <p v-if="task.error" class="task-error">{{ task.error }}</p>
      </div>
      <div class="task-actions">
        <button
          v-if="running(task)"
          class="icon-button"
          :disabled="busy === task.id || task.status === 'merging'"
          :title="
            task.status === 'merging' ? '正在提交云端文件，请稍候' : '暂停上传'
          "
          @click="action(task, 'pause')"
        >
          <Pause :size="18" /></button
        ><button
          v-if="['paused', 'error'].includes(task.status)"
          class="icon-button"
          :disabled="busy === task.id"
          :title="task.status === 'error' ? '重试上传' : '继续上传'"
          @click="action(task, 'resume')"
        >
          <RefreshCw v-if="task.status === 'error'" :size="18" /><Play
            v-else
            :size="18"
          /></button
        ><button
          v-if="!running(task)"
          class="icon-button"
          title="移除上传记录"
          :disabled="busy === task.id"
          @click="removeTarget = task"
        >
          <Trash2 :size="18" />
        </button>
      </div>
    </article>
  </section>
  <p class="bottom-note">
    进度按已确认的分片计算。重新启动后，未完成的上传保持暂停。
  </p>
  <Teleport to="body"
    ><div v-if="removeTarget" class="modal-backdrop">
      <div class="confirm-dialog">
        <h2>移除上传记录？</h2>
        <p>{{ removeTarget.name }}<br />已经上传到网盘的文件会保留。</p>
        <div class="button-row">
          <button class="secondary" @click="removeTarget = null">取消</button
          ><button class="danger" @click="remove">移除记录</button>
        </div>
      </div>
    </div></Teleport
  >
</template>
