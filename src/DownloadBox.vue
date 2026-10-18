<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import {
  ArrowDownToLine,
  ChevronRight,
  File,
  Folder,
  FolderOpen,
  Link,
  LoaderCircle,
  Search,
  Upload,
} from "@lucide/vue";
import {
  anonymousShare,
  providerName,
  shareProvider,
  type Provider,
} from "./providers";

type FileEntry = {
  id: string;
  name: string;
  size: number;
  isDir: boolean;
  parentId: string;
};
type Listing = {
  share: { provider: Provider; id: string; passcode: string };
  title: string;
  parentId: string;
  files: FileEntry[];
};
type BtFile = { index: number; path: string; size: number };
type Inspection =
  | { kind: "share"; listing: Listing }
  | {
      kind: "direct";
      url: string;
      name: string;
      size: number;
      rangeSupported: boolean;
    }
  | { kind: "bt"; torrent: { infoHash: string; name: string; files: BtFile[] } }
  | { kind: "btPending"; id: string; message: string };
type Created = { status: string; error: string };
const props = defineProps<{
  accounts: { provider: Provider; loggedIn: boolean }[];
  downloadDir: string;
}>();
const emit = defineEmits<{
  login: [provider: Provider];
  created: [tasks: Created[]];
  directory: [];
  folder: [];
}>();
const text = ref("");
const code = ref("");
const localFile = ref<string | null>(null);
const result = ref<Inspection | null>(null);
const parsing = ref(false);
const adding = ref(false);
const error = ref("");
const selected = ref<string[]>([]);
const filename = ref("");
const connections = ref(16);
const maxPeers = ref(64);
const uploadLimit = ref(512);
const seedMinutes = ref(0);
const crumbs = ref([{ id: "0", name: "分享文件" }]);
const input = ref<HTMLTextAreaElement | null>(null);
let version = 0;
let timer: ReturnType<typeof setTimeout> | undefined;
let pending: string | null = null;
defineExpose({ focus: () => input.value?.focus() });
const bytes = (n: number) => {
  if (!n) return "0 B";
  const i = Math.min(4, Math.floor(Math.log(n) / Math.log(1024)));
  return `${(n / 1024 ** i).toFixed(i ? 1 : 0)} ${["B", "KiB", "MiB", "GiB", "TiB"][i]}`;
};
const files = computed(() => {
  const r = result.value;
  if (r?.kind === "share") return r.listing.files;
  if (r?.kind === "bt")
    return r.torrent.files.map((f) => ({
      id: String(f.index),
      name: f.path,
      size: f.size,
      isDir: false,
      parentId: "",
    }));
  return [];
});
const allSelected = computed(
  () => !!files.value.length && selected.value.length === files.value.length,
);
const selectedSize = computed(() =>
  files.value
    .filter((f) => selected.value.includes(f.id))
    .reduce((n, f) => n + f.size, 0),
);
const detectedProvider = computed(() =>
  result.value?.kind === "share"
    ? result.value.listing.share.provider
    : shareProvider(text.value),
);
const needsLogin = computed(
  () =>
    !!detectedProvider.value &&
    !anonymousShare(detectedProvider.value) &&
    !props.accounts.some(
      (a) => a.provider === detectedProvider.value && a.loggedIn,
    ),
);
const label = computed(() =>
  result.value?.kind === "bt"
    ? "BT 种子"
    : result.value?.kind === "btPending"
      ? "磁力链接"
      : result.value?.kind === "share"
        ? providerName(result.value.listing.share.provider)
        : result.value?.kind === "direct"
          ? "普通文件链接"
          : "自动识别链接类型",
);

function cancel() {
  version++;
  clearTimeout(timer);
  parsing.value = false;
  const id = pending;
  pending = null;
  if (id) void invoke("cancel_bt_inspection", { id }).catch(() => {});
  if (result.value?.kind === "btPending") result.value = null;
}
watch([text, code], () => {
  cancel();
  result.value = null;
  selected.value = [];
  error.value = "";
});
watch(text, () => {
  localFile.value = null;
});
onBeforeUnmount(cancel);

async function accept(value: Inspection, request: number) {
  if (request !== version) {
    if (value.kind === "btPending")
      void invoke("cancel_bt_inspection", { id: value.id }).catch(() => {});
    return;
  }
  result.value = value;
  if (value.kind === "btPending") {
    pending = value.id;
    timer = setTimeout(async () => {
      if (request !== version) return;
      try {
        await accept(
          await invoke<Inspection>("poll_bt_inspection", { id: value.id }),
          request,
        );
      } catch (e) {
        if (request === version) {
          error.value = String(e);
          cancel();
        }
      }
    }, 700);
    return;
  }
  pending = null;
  parsing.value = false;
  if (value.kind === "direct") filename.value = value.name;
  if (value.kind === "bt")
    selected.value = value.torrent.files.map((f) => String(f.index));
  if (value.kind === "share") {
    selected.value =
      value.listing.files.length === 1 && !value.listing.files[0]!.isDir
        ? [value.listing.files[0]!.id]
        : [];
    crumbs.value = [{ id: "0", name: value.listing.title || "分享文件" }];
  }
}
async function inspect(file: string | null = null) {
  cancel();
  const request = version;
  parsing.value = true;
  error.value = "";
  result.value = null;
  selected.value = [];
  try {
    await accept(
      await invoke<Inspection>("inspect_download", {
        text: text.value,
        code: code.value,
        localFile: file,
      }),
      request,
    );
  } catch (e) {
    if (request === version) {
      error.value = String(e);
      parsing.value = false;
    }
  }
}
async function importTorrent() {
  try {
    const file = await open({
      multiple: false,
      directory: false,
      title: "导入 BT 种子",
      filters: [{ name: "BitTorrent 种子", extensions: ["torrent"] }],
    });
    if (typeof file !== "string") return;
    text.value = file;
    // Wait for the input watcher before installing the selected local source.
    await Promise.resolve();
    localFile.value = file;
    await inspect(file);
  } catch (e) {
    error.value = String(e);
  }
}
async function navigate(id: string, trail: typeof crumbs.value) {
  const r = result.value;
  if (r?.kind !== "share") return;
  cancel();
  const request = version;
  parsing.value = true;
  error.value = "";
  try {
    const listing = await invoke<Listing>("parse_link", {
      text: text.value,
      code: r.listing.share.passcode,
      parentId: id,
    });
    if (request !== version) return;
    result.value = { kind: "share", listing };
    selected.value = [];
    crumbs.value = trail;
  } catch (e) {
    if (request === version) error.value = String(e);
  } finally {
    if (request === version) parsing.value = false;
  }
}
async function download() {
  const r = result.value;
  if (!r || r.kind === "btPending" || adding.value) return;
  if (r.kind === "share" && needsLogin.value) {
    emit("login", r.listing.share.provider);
    return;
  }
  if (
    r.kind === "bt" &&
    (![maxPeers.value, uploadLimit.value, seedMinutes.value].every(
      Number.isInteger,
    ) ||
      maxPeers.value < 1 ||
      maxPeers.value > 500 ||
      uploadLimit.value < 0 ||
      uploadLimit.value > 1048576 ||
      seedMinutes.value < 0 ||
      seedMinutes.value > 1440)
  ) {
    error.value =
      "请输入有效的 BT 参数：节点 1–500、上传上限 0–1048576 KiB/s。";
    return;
  }
  adding.value = true;
  error.value = "";
  try {
    let tasks: Created[];
    if (r.kind === "direct")
      tasks = [
        await invoke<Created>("enqueue_direct", {
          url: r.url,
          filename: filename.value.trim(),
          connections: connections.value,
        }),
      ];
    else if (r.kind === "bt")
      tasks = [
        await invoke<Created>("enqueue_bt", {
          infoHash: r.torrent.infoHash,
          selected: selected.value.map(Number),
          maxPeers: maxPeers.value,
          uploadLimit: uploadLimit.value,
          seedMinutes: seedMinutes.value,
        }),
      ];
    else
      tasks = await invoke<Created[]>("enqueue", {
        share: r.listing.share,
        parentId: r.listing.parentId,
        ids: selected.value,
      });
    emit("created", tasks);
    const failed = tasks.find((t) => t.status === "error");
    if (failed) error.value = failed.error;
    else {
      result.value = null;
      selected.value = [];
      text.value = "";
      code.value = "";
      localFile.value = null;
    }
  } catch (e) {
    error.value = String(e);
  } finally {
    adding.value = false;
  }
}
</script>

<template>
  <section class="new-download card unified-download">
    <div class="section-title">
      <span class="section-icon"><Link :size="18" /></span>
      <h2>新建下载</h2>
      <span class="unified-type">{{ label }}</span>
    </div>
    <div class="share-input-wrap">
      <textarea
        ref="input"
        v-model="text"
        placeholder="粘贴下载链接，自动识别文件直链、网盘分享或磁力链接…"
        aria-label="下载链接"
        spellcheck="false"
        :disabled="adding"
      />
      <div class="unified-controls">
        <label class="unified-passcode"
          >提取码<input
            v-model="code"
            placeholder="自动识别"
            maxlength="64"
            aria-label="提取码"
            :disabled="adding"
        /></label>
        <button class="secondary" :disabled="adding" @click="importTorrent">
          <Upload :size="16" />导入种子
        </button>
        <button v-if="parsing" class="secondary" @click="cancel">
          取消解析
        </button>
        <button
          v-else
          class="primary"
          :disabled="adding || !text.trim()"
          @click="inspect(localFile)"
        >
          <Search :size="16" />解析链接
        </button>
      </div>
    </div>
    <p
      v-if="result?.kind === 'btPending'"
      class="unified-pending"
      role="status"
    >
      <LoaderCircle :size="16" class="spinning" />{{ result.message }}
    </p>
    <p v-else-if="parsing" class="unified-pending" role="status">
      <LoaderCircle :size="16" class="spinning" />正在识别并读取文件列表…
    </p>
    <p v-if="error" class="unified-error" role="alert">{{ error }}</p>
    <button
      v-if="needsLogin && error && detectedProvider"
      class="text-button"
      @click="emit('login', detectedProvider)"
    >
      打开{{ providerName(detectedProvider) }}登录
    </button>

    <div v-if="result && result.kind !== 'btPending'" class="unified-preview">
      <template v-if="result.kind === 'direct'">
        <p class="unified-summary">
          <File :size="18" />{{ result.name }} ·
          {{ result.size ? bytes(result.size) : "未知大小" }}
        </p>
        <div class="unified-options">
          <label class="unified-filename"
            >文件名<input
              v-model="filename"
              maxlength="180"
              aria-label="下载文件名"
              :disabled="adding"
          /></label>
          <label
            >加速连接<select
              v-model.number="connections"
              aria-label="下载连接数"
              :disabled="adding"
            >
              <option v-for="n in [1, 4, 8, 16]" :key="n" :value="n">
                最多 {{ n }} 路
              </option>
            </select></label
          >
        </div>
        <p class="unified-hint">
          {{
            result.rangeSupported
              ? "服务器支持分段下载，实际速度取决于服务器和网络。"
              : "服务器不支持分段，将使用单连接；恢复时从头下载。"
          }}
        </p>
      </template>
      <template v-else>
        <div v-if="result.kind === 'share'" class="breadcrumbs unified-crumbs">
          <template v-for="(crumb, i) in crumbs" :key="crumb.id"
            ><ChevronRight v-if="i" :size="14" /><button
              :disabled="parsing || adding"
              @click="navigate(crumb.id, crumbs.slice(0, i + 1))"
            >
              {{ crumb.name }}
            </button></template
          >
        </div>
        <p v-else class="unified-summary">
          <File :size="18" />{{ result.torrent.name }} ·
          {{ result.torrent.files.length }} 个文件
        </p>
        <div class="file-list unified-files">
          <div class="file-head">
            <input
              type="checkbox"
              :checked="allSelected"
              :disabled="adding || parsing"
              aria-label="全选文件"
              @change="selected = allSelected ? [] : files.map((f) => f.id)"
            /><span>文件名称</span><span>大小</span>
          </div>
          <div v-for="file in files" :key="file.id" class="file-row">
            <input
              v-model="selected"
              type="checkbox"
              :value="file.id"
              :disabled="adding || parsing"
              :aria-label="`选择 ${file.name}`"
            /><span class="file-icon" :class="{ directory: file.isDir }"
              ><Folder v-if="file.isDir" :size="21" /><File
                v-else
                :size="21" /></span
            ><button
              v-if="file.isDir"
              class="file-name"
              :disabled="adding || parsing"
              @click="
                navigate(file.id, [...crumbs, { id: file.id, name: file.name }])
              "
            >
              {{ file.name }}<ChevronRight :size="14" /></button
            ><span v-else class="file-name" :title="file.name">{{
              file.name
            }}</span
            ><span class="file-size">{{
              file.isDir ? "文件夹" : bytes(file.size)
            }}</span>
          </div>
          <p v-if="!files.length" class="no-files">这个文件夹里没有文件</p>
        </div>
        <template v-if="result.kind === 'bt'">
          <div class="unified-options bt-options">
            <label
              >最多连接节点<input
                v-model.number="maxPeers"
                type="number"
                min="1"
                max="500"
                aria-label="BT 最大节点数"
                :disabled="adding" /></label
            ><label
              >上传上限（KiB/s）<input
                v-model.number="uploadLimit"
                type="number"
                min="0"
                max="1048576"
                aria-label="BT 上传上限"
                :disabled="adding" /></label
            ><label
              >完成后做种<select
                v-model.number="seedMinutes"
                aria-label="BT 做种时间"
                :disabled="adding"
              >
                <option :value="0">完成后停止</option>
                <option :value="30">30 分钟</option>
                <option :value="60">1 小时</option>
                <option :value="1440">24 小时</option>
              </select></label
            >
          </div>
          <p class="unified-hint">
            BT 下载过程中会向节点上传数据，上传上限填 0
            表示不限速。仅下载所选文件；跨文件的校验分片可能保留少量相邻文件数据。
          </p>
        </template>
      </template>
      <footer class="selection-footer">
        <span>{{
          result.kind === "direct"
            ? "已识别 1 个文件"
            : `已选择 ${selected.length} 项 · ${bytes(selectedSize)}`
        }}</span
        ><button
          class="primary"
          :disabled="
            adding || parsing || (result.kind !== 'direct' && !selected.length)
          "
          @click="download"
        >
          <LoaderCircle
            v-if="adding"
            :size="17"
            class="spinning"
          /><ArrowDownToLine v-else :size="17" />{{
            adding
              ? "正在创建任务…"
              : result.kind === "share" && needsLogin
                ? "登录后下载"
                : "开始下载"
          }}
        </button>
      </footer>
    </div>
    <div class="directory-row">
      <FolderOpen :size="16" /><span>保存到</span
      ><button class="directory" :title="downloadDir" @click="emit('folder')">
        {{ downloadDir || "正在加载…" }}</button
      ><button
        class="text-button"
        :disabled="adding"
        @click="emit('directory')"
      >
        更改
      </button>
    </div>
  </section>
</template>

<style scoped>
.unified-download {
  min-width: 0;
}
.unified-type {
  margin-left: auto;
  font-size: 12px;
  color: var(--muted);
}
.unified-controls {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 3px 10px 10px 14px;
  flex-wrap: wrap;
}
.unified-passcode {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-right: auto;
  color: var(--muted);
  font-size: 12px;
}
.unified-passcode input {
  width: 90px;
  font-size: 11px;
  padding: 6px 8px;
}
.unified-controls input,
.unified-options input,
.unified-options select {
  border: 1px solid var(--line);
  border-radius: 7px;
  background: var(--surface);
  color: var(--ink);
  padding: 8px 10px;
  min-width: 0;
}
.unified-preview {
  margin-top: 16px;
  border: 1px solid var(--line);
  border-radius: 10px;
  overflow: hidden;
}
.unified-summary,
.unified-pending {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13px;
  padding: 12px 16px;
  margin: 0;
  overflow-wrap: anywhere;
}
.unified-pending {
  color: var(--accent);
}
.unified-error {
  color: var(--danger, #bc505c);
  font-size: 13px;
  line-height: 1.6;
  margin: 12px 0;
  overflow-wrap: anywhere;
}
.unified-options {
  display: flex;
  gap: 12px;
  padding: 12px 16px;
  flex-wrap: wrap;
}
.unified-options label {
  display: flex;
  flex-direction: column;
  gap: 6px;
  font-size: 12px;
  color: var(--muted);
  min-width: 120px;
  flex: 1;
}
.unified-options .unified-filename {
  flex: 3;
}
.unified-hint {
  color: var(--muted);
  font-size: 12px;
  line-height: 1.6;
  padding: 0 16px 12px;
  margin: 0;
}
.unified-files {
  max-height: 300px;
  overflow: auto;
  margin: 0;
}
.unified-files .file-head {
  position: sticky;
  top: 0;
  z-index: 1;
  background: var(--surface-soft);
}
.unified-crumbs {
  padding: 12px 16px;
}
.unified-preview .selection-footer {
  padding: 12px 16px;
  gap: 10px;
  flex-wrap: wrap;
}
.unified-files .file-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.unified-download textarea {
  min-height: 58px;
}
</style>
