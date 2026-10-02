<script setup lang="ts">
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { Copy, Link, Share2, X } from "@lucide/vue";
import {
  type CreatedShare,
  formatDate,
  formatSharePeriod,
  isShareExpired as expired,
  shareText,
} from "./cloud";
defineProps<{ shares: CreatedShare[] }>();
const emit = defineEmits<{
  changed: [];
  error: [message: string];
  notice: [message: string];
  browse: [];
}>();
const target = ref<CreatedShare | null>(null);
const busy = ref(false);
const cancelError = ref("");
async function copy(s: CreatedShare) {
  try {
    await navigator.clipboard.writeText(shareText(s));
    emit("notice", "分享链接和提取码已复制");
  } catch {
    emit("error", "复制失败，请选中链接手动复制");
  }
}
async function cancel() {
  if (!target.value) return;
  busy.value = true;
  cancelError.value = "";
  try {
    await invoke("drive_cancel_share", { id: target.value.id });
    target.value = null;
    emit("changed");
    emit("notice", "分享已取消");
  } catch (e) {
    cancelError.value = String(e);
  } finally {
    busy.value = false;
  }
}
</script>
<template>
  <div class="cloud-page-toolbar">
    <p class="account-intro">显示通过本程序创建的分享。</p>
    <button class="primary" @click="emit('browse')">
      <Share2 :size="16" />选择文件分享
    </button>
  </div>
  <section class="card task-list">
    <div v-if="!shares.length" class="empty-state">
      <span class="empty-illustration"><Link :size="40" /></span>
      <h3>还没有分享记录</h3>
      <p>在「我的网盘」中选择文件，设置有效期和提取码。</p>
    </div>
    <article v-for="s in shares" :key="s.id" class="share-record">
      <div class="task-name-row">
        <strong :title="s.names.join('、')">{{ s.names.join("、") }}</strong
        ><span
          class="status-pill"
          :class="{ complete: !s.cancelled && !expired(s) }"
          >{{ s.cancelled ? "已取消" : expired(s) ? "已过期" : "分享中" }}</span
        >
      </div>
      <div class="share-link-text">
        <Link :size="15" /><span>{{ s.url }}</span
        ><span>提取码：{{ s.password }}</span>
      </div>
      <div class="share-record-bottom">
        <span
          >创建于 {{ formatDate(s.createdAt) }} ·
          {{ formatSharePeriod(s.period) }}</span
        ><button
          v-if="!s.cancelled && !expired(s)"
          class="text-button"
          @click="copy(s)"
        >
          <Copy :size="14" />复制分享</button
        ><button
          v-if="!s.cancelled && !expired(s)"
          class="text-button cancel-share"
          @click="
            cancelError = '';
            target = s;
          "
        >
          <X :size="14" />取消分享
        </button>
      </div>
    </article>
  </section>
  <Teleport to="body"
    ><div v-if="target" class="modal-backdrop">
      <div class="confirm-dialog">
        <h2>取消这个分享？</h2>
        <div v-if="cancelError" class="error-banner" role="alert">
          {{ cancelError }}
        </div>
        <p>
          {{ target.names.join("、")
          }}<br />取消后原链接将无法访问，网盘文件会保留。
        </p>
        <div class="button-row">
          <button class="secondary" :disabled="busy" @click="target = null">
            返回</button
          ><button class="danger" :disabled="busy" @click="cancel">
            {{ busy ? "正在取消…" : "取消分享" }}
          </button>
        </div>
      </div>
    </div></Teleport
  >
</template>
