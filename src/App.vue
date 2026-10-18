<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import DrivePanel from "./DrivePanel.vue";
import CloudDrivePanel from "./CloudDrivePanel.vue";
import { type Provider, providers, providerName } from "./providers";
import UploadPanel from "./UploadPanel.vue";
import SharePanel from "./SharePanel.vue";
import DownloadBox from "./DownloadBox.vue";
import TitleBar from "./TitleBar.vue";
import { version } from "../package.json";
import type { UploadTask, CreatedShare } from "./cloud";
import {
  ArrowDownToLine,
  ArrowDown,
  ArrowUpRight,
  ArrowUpFromLine,
  Share2,
  Check,
  CheckCheck,
  ChevronRight,
  CircleAlert,
  CircleCheck,
  CloudDownload,
  File,
  FileVideo,
  Folder,
  FolderOpen,
  Link,
  LoaderCircle,
  Pause,
  Play,
  Plus,
  RefreshCw,
  Search,
  ShieldCheck,
  Trash2,
  UserRound,
  X,
} from "@lucide/vue";

type Account = {
  provider: Provider;
  loggedIn: boolean;
  name: string;
  message: string;
};
type Share = { provider: Provider; id: string; passcode: string };
type Task = {
  bt?: { selected: number[]; maxPeers: number } | null;
  uploadSpeed?: number;
  direct?: { url: string; connections: number; rangeSupported: boolean } | null;
  connections?: number;
  own?: { accountId: string } | null;
  share: Share;
  id: string;
  name: string;
  size: number;
  completed: number;
  speed: number;
  status: string;
  error: string;
  directory: string;
  createdAt: number;
};
type Snapshot = {
  accounts: Account[];
  tasks: Task[];
  downloadDir: string;
  engineRunning: boolean;
  engineError: string;
  uploads: UploadTask[];
  shares: CreatedShare[];
};
const state = ref<Snapshot>({
  accounts: providers.map((p) => ({
    provider: p.id,
    loggedIn: false,
    name: "未登录",
    message: "登录账号后开始下载",
  })),
  tasks: [],
  downloadDir: "",
  engineRunning: false,
  engineError: "",
  uploads: [],
  shares: [],
});
const tab = ref("all");
const driveProvider = ref<Provider>("quark");
const driveBusy = ref(false);
const downloadBox = ref<InstanceType<typeof DownloadBox> | null>(null);
const signingIn = ref<Provider[]>([]);
const refreshing = ref<Provider[]>([]);
const accountFor = (provider: Provider) =>
  state.value.accounts.find((a) => a.provider === provider) || {
    provider,
    loggedIn: false,
    name: "未登录",
    message: "登录账号后开始下载",
  };
const connectedCount = computed(
  () => state.value.accounts.filter((a) => a.loggedIn).length,
);
const error = ref("");
const toast = ref("");
const filter = ref("");
const pendingAction = ref<string | null>(null);
const removeTarget = ref<Task | null>(null);
const deleteLocalFiles = ref(false);
const removing = ref(false);
const removeError = ref("");
let interval: ReturnType<typeof setInterval> | undefined;
let toastTimer: ReturnType<typeof setTimeout> | undefined;
let unlisten: UnlistenFn | undefined;
let loadingSnapshot = false;
const activeStatuses = ["active", "waiting", "resolving", "seeding"];
const active = computed(() =>
  state.value.tasks.filter((t) => activeStatuses.includes(t.status)),
);
const completed = computed(() =>
  state.value.tasks.filter((t) => t.status === "complete"),
);
const failed = computed(() =>
  state.value.tasks.filter((t) => t.status === "error"),
);
const speed = computed(() =>
  state.value.tasks.reduce((sum, t) => sum + t.speed, 0),
);
const tasks = computed(() =>
  state.value.tasks.filter(
    (t) =>
      (tab.value === "all" ||
        (tab.value === "active" && activeStatuses.includes(t.status)) ||
        (tab.value === "complete" && t.status === "complete") ||
        (tab.value === "error" && t.status === "error")) &&
      t.name.toLowerCase().includes(filter.value.toLowerCase()),
  ),
);
const title = computed(
  () =>
    ({
      all: "下载工作台",
      active: "正在下载",
      complete: "已完成",
      error: "需要处理",
      account: "账号管理",
      drive: "我的网盘",
      uploads: "上传任务",
      shares: "分享记录",
    })[tab.value],
);
const bytes = (n: number) => {
  if (!n) return "0 B";
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  const i = Math.min(Math.floor(Math.log(n) / Math.log(1024)), 4);
  return `${(n / 1024 ** i).toFixed(i ? 1 : 0)} ${units[i]}`;
};
const progress = (t: Task) =>
  t.size
    ? Math.min(100, (t.completed / t.size) * 100)
    : t.status === "complete"
      ? 100
      : 0;
const statusName = (status: string) =>
  ({
    active: "下载中",
    seeding: "做种中",
    waiting: "等待中",
    resolving: "获取地址",
    paused: "已暂停",
    complete: "已完成",
    error: "下载失败",
    removed: "已移除",
    cancelled: "已取消",
  })[status] || status;
const fileIcon = (name: string) =>
  /\.(mp4|mkv|avi|mov|webm|flv|ts)$/i.test(name) ? FileVideo : File;
function notice(message: string) {
  toast.value = message;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (toast.value = ""), 4500);
}
async function refresh() {
  if (loadingSnapshot) return;
  loadingSnapshot = true;
  try {
    state.value = await invoke<Snapshot>("snapshot");
  } catch (e) {
    error.value = String(e);
  } finally {
    loadingSnapshot = false;
  }
}
async function login(provider: Provider) {
  try {
    await invoke("open_login", { provider });
    if (!signingIn.value.includes(provider)) signingIn.value.push(provider);
    notice(`请在打开的${providerName(provider)}窗口完成登录`);
  } catch (e) {
    error.value = String(e);
  }
}
async function checkAccount(provider: Provider, silent = false) {
  if (refreshing.value.includes(provider)) return;
  refreshing.value.push(provider);
  try {
    const account = await invoke<Account>("refresh_account", { provider });
    const i = state.value.accounts.findIndex((a) => a.provider === provider);
    if (i >= 0) state.value.accounts[i] = account;
    else state.value.accounts.push(account);
    if (account.loggedIn) {
      if (signingIn.value.includes(provider) || !silent)
        notice(`${providerName(provider)}已连接`);
      signingIn.value = signingIn.value.filter((p) => p !== provider);
    } else if (!silent) notice(account.message);
  } catch (e) {
    if (!silent) error.value = String(e);
  } finally {
    refreshing.value = refreshing.value.filter((p) => p !== provider);
  }
}
async function chooseDirectory() {
  const path = await open({
    directory: true,
    multiple: false,
    defaultPath: state.value.downloadDir,
    title: "选择下载文件夹",
  });
  if (typeof path === "string") {
    await invoke("set_directory", { path });
    await refresh();
  }
}
async function downloadsCreated(tasks: { status: string; error: string }[]) {
  const count = tasks.filter((t) => t.status !== "error").length;
  if (count) notice(`已添加 ${count} 个下载任务`);
  await refresh();
}
async function action(task: Task, action: string) {
  pendingAction.value = task.id;
  error.value = "";
  try {
    await invoke("task_action", { id: task.id, action });
    await refresh();
  } catch (e) {
    error.value = String(e);
  } finally {
    pendingAction.value = null;
  }
}
function confirmRemove(task: Task) {
  deleteLocalFiles.value = false;
  removeError.value = "";
  removeTarget.value = task;
}
async function remove() {
  const task = removeTarget.value;
  if (!task || removing.value) return;
  removing.value = true;
  removeError.value = "";
  pendingAction.value = task.id;
  try {
    await invoke("task_action", {
      id: task.id,
      action: "remove",
      deleteFiles: deleteLocalFiles.value,
    });
    removeTarget.value = null;
    notice(
      deleteLocalFiles.value
        ? "任务及本地文件已删除"
        : "任务已移除，本地文件已保留",
    );
  } catch (e) {
    removeError.value = String(e);
  } finally {
    removing.value = false;
    pendingAction.value = null;
    await refresh();
  }
}
async function folder(id?: string) {
  try {
    await invoke("open_directory", { id: id || null });
  } catch (e) {
    error.value = String(e);
  }
}
onMounted(async () => {
  await refresh();
  unlisten = await listen("tasks-changed", refresh);
  for (const provider of providers) void checkAccount(provider.id, true);
  interval = setInterval(() => {
    for (const provider of signingIn.value) void checkAccount(provider, true);
  }, 4000);
});
onUnmounted(() => {
  clearInterval(interval);
  clearTimeout(toastTimer);
  unlisten?.();
});
</script>

<template>
  <div class="app-shell">
    <TitleBar @error="(message) => (error = message)" />
    <aside class="sidebar">
      <div class="brand">
        <span class="brand-mark"
          ><ArrowDownToLine :size="24" :stroke-width="2.7"
        /></span>
        <div>CloudDownloader<small>云盘与文件下载</small></div>
      </div>
      <button
        class="new-button"
        @click="
          tab = 'all';
          $nextTick(() => downloadBox?.focus());
        "
      >
        <Plus :size="18" />新建下载
      </button>
      <div class="nav-caption">下载管理</div>
      <nav>
        <button :class="{ selected: tab === 'all' }" @click="tab = 'all'">
          <CloudDownload :size="19" />全部任务<span>{{
            state.tasks.length
          }}</span>
        </button>
        <button :class="{ selected: tab === 'active' }" @click="tab = 'active'">
          <ArrowDown :size="19" />正在下载<span v-if="active.length">{{
            active.length
          }}</span>
        </button>
        <button
          :class="{ selected: tab === 'complete' }"
          @click="tab = 'complete'"
        >
          <CircleCheck :size="19" />已完成<span v-if="completed.length">{{
            completed.length
          }}</span>
        </button>
        <button :class="{ selected: tab === 'error' }" @click="tab = 'error'">
          <CircleAlert :size="19" />需要处理<span
            v-if="failed.length"
            class="error-count"
            >{{ failed.length }}</span
          >
        </button>
      </nav>
      <div class="nav-caption second-caption">云端空间</div>
      <nav>
        <button :class="{ selected: tab === 'drive' }" @click="tab = 'drive'">
          <Folder :size="19" />我的网盘
        </button>
        <button
          :class="{ selected: tab === 'uploads' }"
          @click="tab = 'uploads'"
        >
          <ArrowUpFromLine :size="19" />上传任务<span
            v-if="state.uploads.length"
            >{{ state.uploads.length }}</span
          >
        </button>
        <button :class="{ selected: tab === 'shares' }" @click="tab = 'shares'">
          <Share2 :size="19" />分享记录
        </button>
        <button
          :class="{ selected: tab === 'account' }"
          @click="tab = 'account'"
        >
          <UserRound :size="19" />账号管理
        </button>
      </nav>
      <div class="sidebar-bottom">
        <div class="account-mini">
          <div class="avatar"><UserRound :size="20" /></div>
          <div>
            <strong>{{ connectedCount }} 个网盘已连接</strong
            ><small>{{ "管理你的网盘账号" }}</small>
          </div>
          <button title="管理网盘账号" @click="tab = 'account'">
            <ArrowUpRight :size="18" />
          </button>
        </div>
        <div class="engine-state">
          <i :class="{ ready: state.engineRunning }"></i
          >{{ state.engineRunning ? "下载引擎已就绪" : "下载引擎未就绪"
          }}<span>v{{ version }}</span>
        </div>
      </div>
    </aside>
    <main class="main-content" :class="{ 'drive-workspace': tab === 'drive' }">
      <header class="page-header">
        <div class="page-heading-copy">
          <div class="eyebrow">CLOUD DOWNLOADER</div>
          <h1>{{ title }}</h1>
          <p>
            {{
              tab === "drive"
                ? driveProvider === "baidu"
                  ? "浏览百度网盘文件，上传、下载和分享。"
                  : `浏览${providerName(driveProvider)}，选择文件或文件夹下载到本地。`
                : tab === "uploads"
                  ? "分片并发传输，随时掌握上传进度。"
                  : tab === "shares"
                    ? "管理通过本程序创建的百度分享链接。"
                    : "文件直链、网盘分享与 BT 下载，让文件抵达本地。"
            }}
          </p>
        </div>
        <span
          class="connection-badge"
          :class="{ connected: connectedCount > 0 }"
          ><i></i
          >{{
            connectedCount ? `${connectedCount} 个网盘已连接` : "尚未登录"
          }}</span
        >
      </header>
      <div v-if="error || state.engineError" class="error-banner" role="alert">
        <CircleAlert :size="18" /><span>{{ error || state.engineError }}</span
        ><button aria-label="关闭错误提示" @click="error = ''">
          <X :size="16" />
        </button>
      </div>
      <template v-if="tab === 'drive'">
        <div class="drive-providers" role="group" aria-label="选择个人网盘">
          <button
            v-for="provider in providers"
            :key="provider.id"
            :class="{ selected: driveProvider === provider.id }"
            :aria-pressed="driveProvider === provider.id"
            :disabled="driveBusy"
            @click="driveProvider = provider.id"
          >
            <span
              class="drive-provider-dot"
              :style="{ background: provider.color }"
            ></span
            >{{ provider.name }}
          </button>
        </div>
        <DrivePanel
          v-if="driveProvider === 'baidu'"
          :download-dir="state.downloadDir"
          @notice="notice"
          @error="error = $event"
          @login="login('baidu')"
          @changed="refresh"
          @directory="chooseDirectory"
          @downloaded="
            tab = 'all';
            refresh();
          "
          @uploaded="
            tab = 'uploads';
            refresh();
          "
        />
        <CloudDrivePanel
          v-else
          :key="driveProvider"
          :provider="driveProvider"
          :connected="accountFor(driveProvider).loggedIn"
          :download-dir="state.downloadDir"
          @login="login(driveProvider)"
          @notice="notice"
          @changed="refresh"
          @directory="chooseDirectory"
          @busy="driveBusy = $event"
          @downloaded="
            tab = 'all';
            refresh();
          "
        />
      </template>
      <UploadPanel
        v-else-if="tab === 'uploads'"
        :tasks="state.uploads"
        @changed="refresh"
        @error="error = $event"
        @browse="
          driveProvider = 'baidu';
          tab = 'drive';
        "
      />
      <SharePanel
        v-else-if="tab === 'shares'"
        :shares="state.shares"
        @changed="refresh"
        @error="error = $event"
        @notice="notice"
        @browse="
          driveProvider = 'baidu';
          tab = 'drive';
        "
      />
      <template v-else-if="tab === 'account'">
        <p class="account-intro">
          选择网盘，在官方窗口完成扫码或验证码登录。各网盘的登录状态独立保存在这台电脑上。
        </p>
        <div class="account-grid">
          <section
            v-for="provider in providers"
            :key="provider.id"
            class="account-card card"
          >
            <span
              class="provider-avatar"
              :style="{ background: provider.color }"
              >{{ provider.initial }}</span
            >
            <h2>{{ provider.name }}</h2>
            <strong>{{ accountFor(provider.id).name }}</strong>
            <p :class="{ connected: accountFor(provider.id).loggedIn }">
              {{ accountFor(provider.id).message }}
            </p>
            <button class="primary" @click="login(provider.id)">
              <UserRound :size="17" />{{
                accountFor(provider.id).loggedIn ? "打开网盘" : "登录账号"
              }}
            </button>
            <button
              class="text-button"
              :disabled="refreshing.includes(provider.id)"
              @click="checkAccount(provider.id)"
            >
              <RefreshCw
                :size="15"
                :class="{ spinning: refreshing.includes(provider.id) }"
              />刷新登录状态
            </button>
          </section>
        </div>
        <div class="privacy-note">
          <ShieldCheck :size="17" />登录过期时，重新连接即可继续下载。
        </div>
      </template>
      <template v-else>
        <DownloadBox
          v-show="tab === 'all'"
          ref="downloadBox"
          :accounts="state.accounts"
          :download-dir="state.downloadDir"
          @login="login"
          @created="downloadsCreated"
          @directory="chooseDirectory"
          @folder="folder()"
        />
        <div class="task-heading">
          <div>
            <h2>下载列表</h2>
            <span>{{ state.tasks.length }} 个任务</span>
          </div>
          <div class="speed-label" v-if="speed">
            <ArrowDown :size="14" />{{ bytes(speed) }}/s
          </div>
          <div class="task-search">
            <Search :size="15" /><input
              v-model="filter"
              placeholder="搜索任务"
              aria-label="搜索下载任务"
            />
          </div>
        </div>
        <section class="task-list card">
          <div v-if="!tasks.length" class="empty-state">
            <span class="empty-illustration"
              ><CloudDownload :size="44" :stroke-width="1.3" /><span
                ><Plus :size="14" /></span
            ></span>
            <h3>
              {{
                filter
                  ? "没有找到匹配的任务"
                  : tab === "complete"
                    ? "还没有已完成的下载"
                    : tab === "error"
                      ? "目前没有需要处理的任务"
                      : tab === "active"
                        ? "目前没有正在下载的任务"
                        : "你的下一份文件，从这里出发"
              }}
            </h3>
            <p>
              {{
                filter
                  ? "试试其他文件名"
                  : "粘贴文件直链、网盘分享或磁力链接，也可导入种子。"
              }}
            </p>
            <div class="empty-steps">
              <span><Link :size="14" />粘贴链接</span
              ><ChevronRight :size="12" /><span
                ><CheckCheck :size="14" />选择文件</span
              ><ChevronRight :size="12" /><span
                ><ArrowDownToLine :size="14" />下载到本地</span
              >
            </div>
          </div>
          <article v-for="task in tasks" :key="task.id" class="task-row">
            <div class="task-file-icon">
              <component :is="fileIcon(task.name)" :size="25" />
            </div>
            <div class="task-details">
              <div class="task-name-row">
                <strong :title="task.name">{{ task.name }}</strong
                ><span class="status-pill" :class="task.status"
                  ><i></i>{{ statusName(task.status) }}</span
                >
              </div>
              <div
                class="task-progress"
                :class="[
                  task.status,
                  { indeterminate: !task.size && task.status === 'active' },
                ]"
              >
                <div :style="{ width: `${progress(task)}%` }"></div>
              </div>
              <div class="task-meta">
                <span class="provider-tag">{{
                  task.bt
                    ? "BT 下载"
                    : task.direct
                      ? "普通链接"
                      : providerName(task.share.provider) +
                        (task.own ? " · 我的网盘" : "")
                }}</span>
                <span
                  >{{ bytes(task.completed) }} /
                  {{
                    task.size || task.status === "complete"
                      ? bytes(task.size)
                      : "未知大小"
                  }}</span
                ><span v-if="task.speed">{{ bytes(task.speed) }}/s</span
                ><span v-if="task.status === 'complete'"
                  ><Check :size="12" />已保存到本地</span
                ><span v-else-if="task.size"
                  >{{ progress(task).toFixed(1) }}%</span
                >
              </div>
              <p v-if="task.bt" class="direct-task-detail">
                已选 {{ task.bt.selected.length }} 个文件 ·
                {{ task.connections || 0 }} 个节点 / 最多
                {{ task.bt.maxPeers }} · 上传
                {{ bytes(task.uploadSpeed || 0) }}/s
              </p>
              <p v-if="task.direct" class="direct-task-detail">
                {{
                  task.direct.rangeSupported
                    ? task.status === "active"
                      ? `${task.connections || 0} 路连接 · 最多 ${task.direct.connections} 路`
                      : `支持分段 · 最多 ${task.direct.connections} 路连接`
                    : "单连接 · 服务器不支持分段，恢复时从头下载"
                }}
              </p>
              <p v-if="task.error" class="task-error">{{ task.error }}</p>
            </div>
            <div class="task-actions">
              <LoaderCircle
                v-if="pendingAction === task.id"
                :size="19"
                class="spinning"
              /><template v-else
                ><button
                  v-if="['active', 'waiting', 'seeding'].includes(task.status)"
                  class="icon-button"
                  title="暂停下载"
                  aria-label="暂停下载"
                  @click="action(task, 'pause')"
                >
                  <Pause :size="17" /></button
                ><button
                  v-if="['paused', 'error', 'cancelled'].includes(task.status)"
                  class="icon-button"
                  :title="
                    task.status === 'error'
                      ? task.direct
                        ? '重试下载'
                        : '重新解析并重试'
                      : '继续下载'
                  "
                  aria-label="继续下载"
                  @click="action(task, 'resume')"
                >
                  <Play :size="17" /></button
                ><button
                  class="icon-button"
                  title="打开下载文件夹"
                  aria-label="打开下载文件夹"
                  @click="folder(task.id)"
                >
                  <FolderOpen :size="17" /></button
                ><button
                  class="icon-button"
                  title="删除下载任务"
                  aria-label="删除下载任务"
                  :disabled="task.status === 'resolving'"
                  @click="confirmRemove(task)"
                >
                  <Trash2 :size="17" /></button
              ></template>
            </div>
          </article>
        </section>
        <div class="bottom-note">
          <ShieldCheck :size="14" /><span
            >关闭窗口后在托盘继续下载 · 右键托盘图标可退出程序</span
          >
        </div>
      </template>
    </main>
    <Transition name="toast"
      ><div v-if="toast" class="toast-message" role="status">
        <CircleCheck :size="18" />{{ toast }}
      </div></Transition
    >
    <div
      v-if="removeTarget"
      class="modal-backdrop"
      @click.self="!removing && (removeTarget = null)"
    >
      <section
        class="confirm-dialog remove-task-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="remove-title"
      >
        <span class="dialog-icon"><Trash2 :size="24" /></span>
        <h2 id="remove-title">移除这个下载任务？</h2>
        <p class="remove-task-name">{{ removeTarget.name }}</p>
        <p>任务会停止，并从列表中移除。</p>
        <label class="remove-files-option">
          <input
            v-model="deleteLocalFiles"
            type="checkbox"
            :disabled="removing"
          />
          <span>同时删除本地文件</span>
        </label>
        <p class="remove-files-note" :class="{ destructive: deleteLocalFiles }">
          {{
            deleteLocalFiles
              ? "将永久删除已下载或未完成的文件及续传信息，无法恢复。"
              : "未勾选时，已下载的文件和续传信息会保留。"
          }}
        </p>
        <p class="remove-file-path" :title="removeTarget.directory">
          保存位置：{{ removeTarget.directory }}
        </p>
        <p v-if="removeError" class="remove-task-error" role="alert">
          {{ removeError }}
        </p>
        <div class="button-row">
          <button
            class="secondary"
            :disabled="removing"
            @click="removeTarget = null"
          >
            保留任务</button
          ><button class="danger" :disabled="removing" @click="remove">
            <LoaderCircle v-if="removing" :size="15" class="spinning" />
            {{
              removing
                ? "正在删除…"
                : deleteLocalFiles
                  ? "删除任务及文件"
                  : "仅移除任务"
            }}
          </button>
        </div>
      </section>
    </div>
  </div>
</template>
