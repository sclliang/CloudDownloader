<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import {
  ArrowDownToLine,
  ChevronRight,
  File,
  Folder,
  Home,
  LoaderCircle,
  RefreshCw,
  Search,
  UserRound,
} from "@lucide/vue";
import { type DriveFile, formatBytes, formatDate } from "./cloud";
import { type CloudProvider, providerName } from "./providers";

const props = defineProps<{
  provider: CloudProvider;
  downloadDir: string;
  connected: boolean;
}>();
const emit = defineEmits<{
  login: [];
  downloaded: [];
  directory: [];
  changed: [];
  notice: [message: string];
  busy: [value: boolean];
}>();
type Listing = {
  accountId: string;
  path: string;
  files: DriveFile[];
  nextCursor: string | null;
};
const listing = ref<Listing | null>(null);
const crumbs = ref([{ path: "/", name: "全部文件" }]);
const selected = ref<string[]>([]);
const keyword = ref("");
const loading = ref(false);
const adding = ref(false);
const error = ref("");
const page = ref(1);
let disposed = false;
let reloadAfterLogin = false;
const busy = computed(() => loading.value || adding.value);
const files = computed(() =>
  (listing.value?.files || []).filter((file) =>
    file.name.toLowerCase().includes(keyword.value.toLowerCase()),
  ),
);
const allSelected = computed(
  () =>
    files.value.length > 0 &&
    files.value.slice(0, 100).every((file) => selected.value.includes(file.id)),
);
watch(
  busy,
  (value) => {
    emit("busy", value);
    if (!value && reloadAfterLogin) {
      reloadAfterLogin = false;
      queueMicrotask(() => {
        if (!disposed) void load();
      });
    }
  },
  { flush: "sync" },
);
watch(
  () => props.connected,
  (connected, previous) => {
    if (connected && !previous) {
      if (busy.value) reloadAfterLogin = true;
      else void load();
    }
  },
);
onUnmounted(() => {
  disposed = true;
  emit("busy", false);
});

async function load(target = crumbs.value, more = false) {
  if (busy.value) return;
  loading.value = true;
  error.value = "";
  try {
    const nextPage = more ? page.value + 1 : 1;
    const result = await invoke<Listing>("cloud_list", {
      provider: props.provider,
      path: target[target.length - 1]!.path,
      page: nextPage,
      cursor: more ? listing.value?.nextCursor : null,
    });
    if (disposed) return;
    if (more && listing.value) {
      if (listing.value.accountId !== result.accountId)
        throw new Error("网盘账号已切换，请刷新文件列表");
      const known = new Set(listing.value.files.map((file) => file.id));
      result.files = [
        ...listing.value.files,
        ...result.files.filter((file) => !known.has(file.id)),
      ];
    } else {
      selected.value = [];
      keyword.value = "";
    }
    listing.value = result;
    crumbs.value = target;
    page.value = nextPage;
  } catch (e) {
    if (!disposed) error.value = String(e);
  } finally {
    if (!disposed) loading.value = false;
  }
}
function enter(file: DriveFile) {
  if (file.isDir)
    void load([...crumbs.value, { path: file.path, name: file.name }]);
}
function selectAll() {
  selected.value = allSelected.value
    ? []
    : files.value.slice(0, 100).map((file) => file.id);
}
async function download() {
  if (busy.value || !listing.value || !selected.value.length) return;
  adding.value = true;
  error.value = "";
  try {
    const result = await invoke<unknown[]>("cloud_enqueue", {
      provider: props.provider,
      accountId: listing.value.accountId,
      files: listing.value.files.filter((file) =>
        selected.value.includes(file.id),
      ),
    });
    if (!disposed) {
      selected.value = [];
      emit("notice", `已添加 ${result.length} 个下载任务`);
      emit("changed");
      emit("downloaded");
    }
  } catch (e) {
    if (!disposed) error.value = String(e);
  } finally {
    if (!disposed) adding.value = false;
  }
}
onMounted(() => load());
</script>

<template>
  <section class="drive-panel card">
    <div class="drive-toolbar">
      <button class="secondary" :disabled="busy" @click="emit('login')">
        <UserRound :size="16" />打开{{ providerName(provider) }}
      </button>
      <button
        class="icon-button"
        title="刷新网盘文件"
        :disabled="busy"
        @click="load()"
      >
        <RefreshCw :size="17" :class="{ spinning: loading }" />
      </button>
      <label class="drive-search"
        ><Search :size="16" /><input
          v-model="keyword"
          placeholder="筛选已加载的文件"
          aria-label="筛选已加载的网盘文件"
          maxlength="128"
          :disabled="busy"
      /></label>
    </div>
    <div class="drive-breadcrumbs" aria-label="网盘文件路径">
      <Home :size="16" />
      <template v-for="(crumb, i) in crumbs" :key="crumb.path">
        <ChevronRight v-if="i" :size="14" /><button
          :disabled="busy"
          @click="load(crumbs.slice(0, i + 1))"
        >
          {{ crumb.name }}
        </button>
      </template>
    </div>
    <div v-if="error" class="drive-error" role="alert">
      <p>{{ error }}</p>
      <button class="secondary" :disabled="busy" @click="emit('login')">
        登录{{ providerName(provider) }}</button
      ><button class="text-button" :disabled="busy" @click="load()">
        重新加载
      </button>
    </div>
    <div v-else class="drive-files" :aria-busy="loading">
      <div class="drive-file-head">
        <input
          type="checkbox"
          :checked="allSelected"
          :disabled="busy || !files.length"
          aria-label="全选网盘文件（最多100项）"
          @change="selectAll"
        /><span>文件名称</span><span>大小</span><span>修改时间</span>
      </div>
      <div v-for="file in files" :key="file.id" class="drive-file-row">
        <input
          v-model="selected"
          type="checkbox"
          :value="file.id"
          :disabled="
            busy || (selected.length >= 100 && !selected.includes(file.id))
          "
          :aria-label="`选择 ${file.name}`"
        />
        <div class="drive-file-name">
          <Folder v-if="file.isDir" class="folder-glyph" :size="22" /><File
            v-else
            :size="21"
          />
          <div>
            <button
              v-if="file.isDir"
              :disabled="busy"
              :title="file.name"
              @click="enter(file)"
            >
              {{ file.name }}</button
            ><span v-else :title="file.name">{{ file.name }}</span>
          </div>
        </div>
        <span class="file-size">{{
          file.isDir ? "文件夹" : formatBytes(file.size)
        }}</span
        ><span class="file-modified">{{ formatDate(file.modified) }}</span>
      </div>
      <div v-if="loading && !listing" class="drive-empty">
        <LoaderCircle class="spinning" :size="30" />
        <p>正在读取{{ providerName(provider) }}文件…</p>
      </div>
      <div v-else-if="listing && !files.length" class="drive-empty">
        <Folder :size="40" />
        <p>{{ keyword ? "没有匹配的文件" : "这个文件夹还是空的" }}</p>
      </div>
      <button
        v-if="listing?.nextCursor != null"
        class="load-more"
        :disabled="busy || page >= 200"
        @click="load(crumbs, true)"
      >
        {{
          loading
            ? "正在加载…"
            : page >= 200
              ? "已达到分页上限，请进入子文件夹查看"
              : "加载更多文件"
        }}
      </button>
    </div>
    <footer class="drive-selection">
      <span
        >已选择 <b>{{ selected.length }}</b> 项<small v-if="listing">
          · 已加载 {{ listing.files.length }} 项</small
        ></span
      >
      <button
        class="primary"
        :disabled="busy || !!error || !selected.length"
        @click="download"
      >
        <LoaderCircle
          v-if="adding"
          class="spinning"
          :size="16"
        /><ArrowDownToLine v-else :size="16" />{{
          adding ? "正在添加下载…" : "下载所选文件"
        }}
      </button>
    </footer>
  </section>
  <div class="directory-row">
    <Folder :size="16" /><span>下载保存到</span
    ><span class="directory" :title="downloadDir">{{ downloadDir }}</span
    ><button class="text-button" :disabled="busy" @click="emit('directory')">
      更改
    </button>
  </div>
</template>
