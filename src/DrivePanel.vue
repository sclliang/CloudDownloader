<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview, type DragDropEvent } from "@tauri-apps/api/webview";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import {
  ArrowDownToLine,
  ArrowUpFromLine,
  Check,
  ChevronRight,
  Copy,
  File,
  Folder,
  FolderPlus,
  Home,
  LoaderCircle,
  RefreshCw,
  Search,
  Share2,
  X,
} from "@lucide/vue";
import {
  type DriveListing,
  type CreatedShare,
  formatBytes,
  formatDate,
  formatSharePeriod,
  shareText,
} from "./cloud";
defineProps<{ downloadDir: string }>();
const emit = defineEmits<{
  notice: [message: string];
  error: [message: string];
  login: [];
  downloaded: [];
  uploaded: [];
  changed: [];
  directory: [];
}>();
const listing = ref<DriveListing | null>(null);
const path = ref("/");
const pathInput = ref("/");
const keyword = ref("");
const loadedKeyword = ref("");
const selected = ref<string[]>([]);
const busy = ref(false);
const actionBusy = ref(false);
const loadError = ref("");
const mode = ref<"" | "upload" | "share" | "mkdir">("");
const uploadPaths = ref<string[]>([]);
const uploadDirectory = ref("/");
const concurrency = ref(4);
const folderName = ref("");
const period = ref(7);
const password = ref("");
const created = ref<CreatedShare | null>(null);
const modalError = ref("");
const panel = ref<HTMLElement | null>(null);
const dropDirectory = ref<string | null>(null);
const choosingUpload = ref(false);
let unlistenDrop: UnlistenFn | undefined;
let disposed = false;
let revision = 0;
const canDrop = computed(
  () =>
    !!listing.value &&
    !busy.value &&
    !actionBusy.value &&
    !mode.value &&
    !choosingUpload.value,
);
const selectedFiles = computed(
  () => listing.value?.files.filter((f) => selected.value.includes(f.id)) || [],
);
const crumbs = computed(() => {
  const segments = path.value.split("/").filter(Boolean);
  return [
    { name: "全部文件", path: "/" },
    ...segments.map((name, i) => ({
      name,
      path: "/" + segments.slice(0, i + 1).join("/"),
    })),
  ];
});
const allSelected = computed(
  () =>
    !!listing.value?.files.length &&
    listing.value.files
      .slice(0, 100)
      .every((f) => selected.value.includes(f.id)),
);
async function load(target = path.value, more = false) {
  const current = ++revision;
  busy.value = true;
  loadError.value = "";
  const search = more ? loadedKeyword.value : keyword.value;
  try {
    const result = await invoke<DriveListing>("drive_list", {
      path: target,
      page: more ? (listing.value?.page || 1) + 1 : 1,
      keyword: search,
    });
    if (current !== revision) return;
    if (more && listing.value?.accountId !== result.accountId)
      throw new Error("账号已切换，请刷新文件列表");
    const previous = more ? listing.value?.files || [] : [];
    const seen = new Set(previous.map((f) => f.id));
    listing.value = {
      ...result,
      files: [...previous, ...result.files.filter((f) => !seen.has(f.id))],
    };
    path.value = result.path;
    loadedKeyword.value = search;
    pathInput.value = result.path;
    if (!more) selected.value = [];
  } catch (e) {
    if (current === revision) {
      loadError.value = String(e);
      if (!more) {
        listing.value = null;
        selected.value = [];
      }
    }
  } finally {
    if (current === revision) busy.value = false;
  }
}
function navigate(target: string) {
  keyword.value = "";
  void load(target);
}
function selectAll() {
  selected.value = allSelected.value
    ? []
    : (listing.value?.files || []).slice(0, 100).map((f) => f.id);
}
async function download() {
  if (!listing.value || !selectedFiles.value.length) return;
  actionBusy.value = true;
  try {
    const tasks = await invoke<{ status: string; error: string }[]>(
      "drive_enqueue",
      { accountId: listing.value.accountId, files: selectedFiles.value },
    );
    const failure = tasks.find((t) => t.status === "error");
    if (failure) emit("error", failure.error);
    else emit("notice", `已添加 ${tasks.length} 个下载任务`);
    emit("downloaded");
  } catch (e) {
    emit("error", String(e));
  } finally {
    actionBusy.value = false;
  }
}
async function chooseUpload() {
  if (!canDrop.value) return;
  const directory = path.value;
  choosingUpload.value = true;
  try {
    const paths = await open({
      multiple: true,
      directory: false,
      title: "选择要上传到百度网盘的文件",
    });
    if (!paths) return;
    uploadPaths.value = typeof paths === "string" ? [paths] : paths;
    uploadDirectory.value = directory;
    modalError.value = "";
    mode.value = "upload";
  } catch (e) {
    emit("error", String(e));
  } finally {
    choosingUpload.value = false;
  }
}
async function enqueueUploads(paths: string[], directory: string) {
  if (!listing.value) return;
  const files = [...new Set(paths)];
  if (!files.length || files.length > 100)
    throw new Error("一次请选择 1 至 100 个本地文件上传");
  await invoke("upload_enqueue", {
    accountId: listing.value.accountId,
    directory,
    paths: files,
    concurrency: concurrency.value,
  });
  mode.value = "";
  emit("notice", `已添加 ${files.length} 个上传任务，目标目录：${directory}`);
  emit("uploaded");
}
function handleDragDrop(event: DragDropEvent) {
  dropDirectory.value = null;
  if (disposed || event.type === "leave" || !canDrop.value) return;
  // Native drag positions are physical pixels; DOM hit testing uses CSS pixels.
  const { x, y } = event.position.toLogical(window.devicePixelRatio);
  const element = document.elementFromPoint(x, y);
  if (!element || !panel.value?.contains(element)) return;
  const folder = element.closest<HTMLElement>("[data-upload-directory]");
  const directory = folder?.dataset.uploadDirectory ?? path.value;
  if (event.type !== "drop") {
    dropDirectory.value = directory;
    return;
  }
  actionBusy.value = true;
  void enqueueUploads(event.paths, directory)
    .catch((e) => emit("error", String(e)))
    .finally(() => {
      actionBusy.value = false;
    });
}
function prepareShare() {
  const random = new Uint8Array(4);
  crypto.getRandomValues(random);
  const alphabet = "abcdefghjkmnpqrstuvwxyz23456789";
  password.value = Array.from(
    random,
    (b) => alphabet[b % alphabet.length],
  ).join("");
  created.value = null;
  modalError.value = "";
  mode.value = "share";
}
async function submit() {
  if (!listing.value) return;
  actionBusy.value = true;
  modalError.value = "";
  try {
    if (mode.value === "upload") {
      await enqueueUploads(uploadPaths.value, uploadDirectory.value);
    } else if (mode.value === "share") {
      created.value = await invoke<CreatedShare>("drive_share", {
        accountId: listing.value.accountId,
        files: selectedFiles.value,
        period: period.value,
        password: password.value,
      });
      emit("changed");
    } else {
      await invoke("drive_mkdir", {
        accountId: listing.value.accountId,
        parent: path.value,
        name: folderName.value,
      });
      mode.value = "";
      folderName.value = "";
      emit("notice", "文件夹已创建");
      await load();
    }
  } catch (e) {
    modalError.value = String(e);
  } finally {
    actionBusy.value = false;
  }
}
async function copyShare() {
  if (!created.value) return;
  try {
    await navigator.clipboard.writeText(shareText(created.value));
    emit("notice", "分享链接和提取码已复制");
  } catch {
    modalError.value = "复制失败，请选中下方文字手动复制";
  }
}
onMounted(async () => {
  void load();
  try {
    const unlisten = await getCurrentWebview().onDragDropEvent(({ payload }) =>
      handleDragDrop(payload),
    );
    if (disposed) unlisten();
    else unlistenDrop = unlisten;
  } catch (e) {
    if (!disposed) emit("error", `无法启用拖拽上传，请使用「上传文件」：${e}`);
  }
});
onUnmounted(() => {
  disposed = true;
  ++revision;
  unlistenDrop?.();
});
</script>

<template>
  <section
    ref="panel"
    class="drive-panel card"
    :class="{ 'drop-active': canDrop && dropDirectory !== null }"
  >
    <div
      v-if="canDrop && dropDirectory !== null"
      class="drive-drop-notice"
      role="status"
    >
      <ArrowUpFromLine :size="22" />
      <span
        >松开即可上传到 <strong>{{ dropDirectory }}</strong></span
      >
    </div>
    <div class="drive-toolbar">
      <span class="provider-tag">百度网盘</span>
      <button class="primary" :disabled="!canDrop" @click="chooseUpload">
        <ArrowUpFromLine :size="16" />上传文件
      </button>
      <button
        class="secondary"
        :disabled="!listing || busy || actionBusy"
        @click="
          modalError = '';
          mode = 'mkdir';
        "
      >
        <FolderPlus :size="16" />新建文件夹
      </button>
      <button
        class="icon-button"
        title="刷新网盘文件"
        :disabled="busy"
        @click="load()"
      >
        <RefreshCw :size="17" :class="{ spinning: busy }" />
      </button>
      <form class="drive-search" @submit.prevent="load()">
        <Search :size="16" /><input
          v-model="keyword"
          placeholder="搜索当前目录及子目录"
          aria-label="搜索网盘文件"
          maxlength="128"
        /><button class="text-button" :disabled="busy">搜索</button>
      </form>
    </div>
    <div class="drive-breadcrumbs">
      <Home :size="16" /><template
        v-for="(crumb, i) in crumbs"
        :key="crumb.path"
        ><ChevronRight v-if="i" :size="14" /><button
          :data-upload-directory="crumb.path"
          :class="{ 'drop-folder': canDrop && dropDirectory === crumb.path }"
          :disabled="busy"
          @click="navigate(crumb.path)"
        >
          {{ crumb.name }}
        </button></template
      >
    </div>
    <form class="drive-path" @submit.prevent="navigate(pathInput)">
      <label for="drive-path">路径</label
      ><input id="drive-path" v-model="pathInput" :disabled="busy" /><button
        class="text-button"
        :disabled="busy"
      >
        前往
      </button>
    </form>
    <p class="drive-drop-hint">
      拖入本地文件即可上传到当前目录，也可直接拖到下方文件夹。
    </p>
    <div v-if="loadError" class="drive-error" role="alert">
      <p>{{ loadError }}</p>
      <button class="secondary" @click="emit('login')">打开百度网盘</button
      ><button class="text-button" @click="load()">重新加载</button>
    </div>
    <div v-else class="drive-files">
      <div class="drive-file-head">
        <input
          type="checkbox"
          :checked="allSelected"
          :disabled="busy || !listing?.files.length"
          aria-label="全选网盘文件（最多100项）"
          @change="selectAll"
        /><span>文件名称</span><span>大小</span><span>修改时间</span>
      </div>
      <div
        v-for="file in listing?.files || []"
        :key="file.id"
        class="drive-file-row"
        :data-upload-directory="file.isDir ? file.path : undefined"
        :class="{
          'drop-folder': canDrop && file.isDir && dropDirectory === file.path,
        }"
      >
        <input
          v-model="selected"
          type="checkbox"
          :value="file.id"
          :disabled="
            busy ||
            actionBusy ||
            (selected.length >= 100 && !selected.includes(file.id))
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
              :title="file.path"
              :disabled="busy"
              @click="navigate(file.path)"
            >
              {{ file.name }}</button
            ><span v-else :title="file.path">{{ file.name }}</span
            ><small v-if="keyword">{{ file.path }}</small>
          </div>
        </div>
        <span class="file-size">{{
          file.isDir ? "文件夹" : formatBytes(file.size)
        }}</span
        ><span class="file-modified">{{ formatDate(file.modified) }}</span>
      </div>
      <div v-if="busy && !listing" class="drive-empty">
        <LoaderCircle class="spinning" :size="30" />
        <p>正在读取网盘文件…</p>
      </div>
      <div v-else-if="listing && !listing.files.length" class="drive-empty">
        <Folder :size="40" />
        <p>{{ keyword ? "没有找到匹配的文件" : "这个文件夹还是空的" }}</p>
        <span>{{
          keyword ? "试试其他关键词" : "拖入本地文件或点击「上传文件」"
        }}</span>
      </div>
      <button
        v-if="listing?.hasMore"
        class="load-more"
        :disabled="busy"
        @click="load(path, true)"
      >
        {{ busy ? "正在加载…" : "加载更多文件" }}
      </button>
    </div>
    <footer class="drive-selection">
      <span
        >已选择 <b>{{ selected.length }}</b> 项
        <small v-if="listing"
          >· 已加载 {{ listing.files.length }} 项</small
        ></span
      ><button
        class="secondary"
        :disabled="!selected.length || busy || actionBusy"
        @click="prepareShare"
      >
        <Share2 :size="16" />创建分享</button
      ><button
        class="primary"
        :disabled="!selected.length || busy || actionBusy"
        @click="download"
      >
        <LoaderCircle
          v-if="actionBusy"
          :size="16"
          class="spinning"
        /><ArrowDownToLine v-else :size="16" />下载所选文件
      </button>
    </footer>
  </section>
  <div class="directory-row">
    <Folder :size="16" /><span>下载保存到</span
    ><span class="directory" :title="downloadDir">{{ downloadDir }}</span
    ><button class="text-button" @click="emit('directory')">更改</button>
  </div>
  <Teleport to="body"
    ><div
      v-if="mode"
      class="modal-backdrop"
      @click.self="!actionBusy && (mode = '')"
    >
      <form class="cloud-dialog" @submit.prevent="submit">
        <div class="dialog-heading">
          <h2>
            {{
              mode === "upload"
                ? "上传到百度网盘"
                : mode === "mkdir"
                  ? "新建文件夹"
                  : created
                    ? "分享已创建"
                    : "创建文件分享"
            }}
          </h2>
          <button
            type="button"
            class="icon-button"
            aria-label="关闭对话框"
            :disabled="actionBusy"
            @click="mode = ''"
          >
            <X :size="18" />
          </button>
        </div>
        <div v-if="modalError" class="error-banner" role="alert">
          {{ modalError }}
        </div>
        <template v-if="mode === 'upload'"
          ><label>目标目录<input :value="uploadDirectory" readonly /></label>
          <ul class="review-files">
            <li v-for="file in uploadPaths" :key="file" :title="file">
              {{ file.split(/[\\/]/).pop() }}
            </li>
          </ul>
          <label
            >每个文件的分片并发<select v-model.number="concurrency">
              <option v-for="n in [1, 2, 4, 8]" :key="n" :value="n">
                {{ n }} 路并发
              </option>
            </select></label
          >
          <p class="dialog-note">
            最多同时上传 2 个文件，总分片并发不超过
            8。遇到同名文件会提示处理，已有文件不会被覆盖。
          </p></template
        >
        <template v-else-if="mode === 'mkdir'"
          ><label
            >名称<input
              v-model="folderName"
              placeholder="文件夹名称"
              maxlength="120"
              required
              autofocus
          /></label>
          <p class="dialog-note">创建位置：{{ path }}</p></template
        >
        <template v-else-if="!created"
          ><ul class="review-files">
            <li v-for="file in selectedFiles" :key="file.id">
              {{ file.name }}
            </li>
          </ul>
          <label
            >有效期<select v-model.number="period">
              <option :value="1">1 天</option>
              <option :value="7">7 天</option>
              <option :value="30">30 天</option>
              <option :value="0">永久有效</option>
            </select></label
          ><label
            >提取码<input
              v-model="password"
              maxlength="4"
              minlength="4"
              pattern="[a-zA-Z0-9]{4}"
              required
          /></label>
          <p class="dialog-note">
            持有链接和提取码的人可以访问所选内容。创建后可在「分享记录」中取消分享。
          </p></template
        >
        <template v-else
          ><div class="share-success">
            <Check :size="22" />链接已生成 ·
            {{ formatSharePeriod(created.period) }}
          </div>
          <textarea
            :value="shareText(created)"
            readonly
            aria-label="生成的分享链接"
            rows="3"
          /><button type="button" class="primary wide" @click="copyShare">
            <Copy :size="16" />复制链接和提取码
          </button></template
        >
        <div v-if="!created || mode !== 'share'" class="dialog-actions">
          <button
            type="button"
            class="secondary"
            :disabled="actionBusy"
            @click="mode = ''"
          >
            取消</button
          ><button class="primary" :disabled="actionBusy">
            <LoaderCircle v-if="actionBusy" class="spinning" :size="16" />{{
              actionBusy ? "正在处理…" : mode === "upload" ? "开始上传" : "创建"
            }}
          </button>
        </div>
      </form>
    </div></Teleport
  >
</template>
