import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";
import whaleIconUrl from "../assets/dsh-icon-source.png";
import {
  CircleAlert,
  CircleCheck,
  Code2,
  Download,
  ExternalLink,
  Eye,
  FileCog,
  FileText,
  FolderOpen,
  Github,
  LoaderCircle,
  Minus,
  PackageCheck,
  Plus,
  Puzzle,
  RefreshCw,
  RotateCcw,
  Save,
  Search,
  Settings,
  SlidersHorizontal,
  Square,
  TerminalSquare,
  Trash2,
  Wrench,
  X,
  createIcons,
} from "lucide";
import "./styles.css";

type LaunchMode = "command" | "npx";
type Phase = "stopped" | "starting" | "ready" | "stopping" | "failed";
type DialogId = "manage-dialog" | "config-dialog" | "plugin-dialog" | "settings-dialog" | "update-dialog";

interface LauncherConfig {
  launch_mode: LaunchMode;
  executable: string;
  npx_package: string;
  working_directory: string;
  dsh_home: string;
  port: number;
  trusted_hosts: string[];
  auto_start: boolean;
  open_on_ready: boolean;
  close_behavior: "tray" | "exit";
  stop_dsh_on_exit: boolean;
  download_directory: string;
  download_ask: boolean;
  download_choose_location: boolean;
  auto_check_updates: boolean;
  window_width: number;
  window_height: number;
}

interface LauncherStatus {
  phase: Phase;
  message: string;
  url: string;
  pid: number | null;
  external: boolean;
  logs: string[];
}

interface TabState {
  id: number;
  title: string;
  frame: HTMLIFrameElement | null;
  loadedUrl: string;
  stale: boolean;
}

interface PackageInfo {
  current_version: string;
  latest_version: string;
  source: string;
  checked_at: string;
  detail: string;
}

interface ReleaseInfo {
  current_version: string;
  latest_version: string;
  tag_name: string;
  name: string;
  body: string;
  html_url: string;
  published_at: string;
  update_available: boolean;
}

interface ConfigFileInfo {
  id: string;
  name: string;
  path: string;
  content: string;
  editable: boolean;
}

interface InstalledPlugin { name: string; version: string; bundle: boolean; }
interface PluginSearchResult { name: string; version: string; description: string; homepage: string; npm_url: string; keywords: string[]; }
interface OperationResult { success: boolean; output: string; }
interface MarketMeta { id: string; label: string; color?: string | null; }
interface MarketPlugin {
  name: string; full_name: string; spec: string; description: string; url: string; homepage: string;
  avatar_url: string; topics: string[]; language: string; stars: number; pushed_at: string;
  archived: boolean; project_type: string; category: string; verified: boolean;
}
interface MarketCatalog { plugins: MarketPlugin[]; fetched_at: number; }

const app = document.querySelector<HTMLDivElement>("#app")!;
app.innerHTML = `
  <div class="shell">
    <header class="titlebar" data-tauri-drag-region>
      <nav class="toolbar-left" aria-label="DSH Launcher 工具">
        <button class="tool-button" data-dialog="manage-dialog" title="管理"><i data-lucide="package-check"></i><span>管理</span></button>
        <button class="tool-button" data-dialog="config-dialog" title="配置"><i data-lucide="sliders-horizontal"></i><span>配置</span></button>
        <button class="tool-button" data-dialog="plugin-dialog" title="插件"><i data-lucide="puzzle"></i><span>插件</span></button>
        <button class="tool-button" data-dialog="settings-dialog" title="设置"><i data-lucide="settings"></i><span>设置</span></button>
        <button id="new-window" class="tool-button" title="新建窗口"><i data-lucide="plus"></i><span>新建</span></button>
      </nav>
      <div id="tab-zone" class="tab-zone" data-tauri-drag-region>
        <div id="tab-strip" class="tab-strip" role="tablist" aria-label="dsh 标签页"></div>
        <div id="tab-drop-indicator" class="tab-drop-indicator" aria-hidden="true" hidden></div>
        <button id="tab-add" class="tab-add" title="新建标签页（Ctrl+T）"><i data-lucide="plus"></i></button>
      </div>
      <nav class="window-controls" aria-label="窗口控制">
        <button id="refresh-web" title="刷新 dsh WebUI"><i data-lucide="refresh-cw"></i></button>
        <button id="window-minimize" title="最小化"><i data-lucide="minus"></i></button>
        <button id="window-maximize" title="最大化"><i data-lucide="square"></i></button>
        <button id="window-close" class="close" title="关闭到托盘"><i data-lucide="x"></i></button>
      </nav>
    </header>

    <main id="workspace-view" class="workspace-view">
      <div id="workspace-state" class="workspace-state" data-phase="stopped">
        <button id="workspace-start" class="whale-button" type="button" aria-label="启动 dsh" title="启动 dsh"><span class="whale-wave whale-wave-one"></span><span class="whale-wave whale-wave-two"></span><img src="${whaleIconUrl}" alt="DeepSeek" draggable="false"></button>
        <h2 id="workspace-title">正在准备 dsh</h2>
        <p id="workspace-message">启动器会在服务就绪后载入 WebUI。</p>
      </div>
    </main>

    <div id="manage-dialog" class="modal" hidden>
      <div class="modal-backdrop" data-close-modal></div>
      <section class="modal-panel manage-panel" role="dialog" aria-modal="true" aria-labelledby="manage-title">
        <header class="modal-header"><div><p class="eyebrow">DSH LAUNCHER</p><h2 id="manage-title">管理</h2></div><button class="modal-close" data-close-modal title="关闭"><i data-lucide="x"></i></button></header>
        <div class="package-summary">
          <div><span class="summary-label">当前版本</span><strong id="current-version">未检查</strong></div>
          <div><span class="summary-label">最新版本</span><strong id="latest-version">未检查</strong></div>
          <div><span class="summary-label">启动来源</span><strong id="package-source">读取中</strong></div>
        </div>
        <div class="modal-actions"><button id="check-package" class="button quiet"><i data-lucide="search"></i><span>检查版本</span></button><button id="update-package" class="button primary"><i data-lucide="download"></i><span>更新 dsh</span></button></div>
        <div class="service-line"><div><span class="summary-label">Web 服务</span><strong id="service-state">读取中</strong></div><div class="service-buttons"><button id="manage-restart" class="icon-button" title="重启服务"><i data-lucide="rotate-ccw"></i></button><button id="manage-stop" class="icon-button danger" title="停止服务"><i data-lucide="square"></i></button></div></div>
        <div class="manage-config"><div class="section-title"><div><p class="eyebrow">STARTUP</p><h3>启动配置</h3></div><button id="manage-save" class="button primary"><i data-lucide="save"></i><span>保存并重启</span></button></div>
          <form id="settings-form" class="compact-form">
            <div class="field full"><label>CLI 来源</label><div class="segmented"><label><input type="radio" name="launch_mode" value="command" checked><span>已安装 dsh</span></label><label><input type="radio" name="launch_mode" value="npx"><span>npx</span></label></div></div>
            <div id="command-field" class="field full"><label for="executable">dsh 命令或完整路径</label><input id="executable" name="executable" autocomplete="off" spellcheck="false"></div>
            <div id="npx-field" class="field full" hidden><label for="npx-package">npm 包</label><input id="npx-package" name="npx_package" autocomplete="off" spellcheck="false"></div>
            <div class="field full"><label for="working-directory">工作目录</label><div class="path-input"><input id="working-directory" name="working_directory" autocomplete="off" spellcheck="false"><button id="browse-workspace" type="button" class="icon-button" title="选择目录"><i data-lucide="folder-open"></i></button></div></div>
            <div class="field"><label for="port">本地端口</label><input id="port" name="port" type="number" min="1024" max="65535"></div>
            <div class="field"><label for="dsh-home">DSH_HOME（可选）</label><input id="dsh-home" name="dsh_home" autocomplete="off" spellcheck="false"></div>
          </form>
        </div>
        <pre id="manage-output" class="operation-output" hidden></pre>
      </section>
    </div>

    <div id="config-dialog" class="modal" hidden>
      <div class="modal-backdrop" data-close-modal></div>
      <section class="modal-panel config-panel" role="dialog" aria-modal="true" aria-labelledby="config-title">
        <header class="modal-header"><div><p class="eyebrow">DSH HOME</p><h2 id="config-title">配置文件</h2></div><button class="modal-close" data-close-modal title="关闭"><i data-lucide="x"></i></button></header>
        <div class="config-toolbar"><select id="config-file-select" aria-label="选择配置文件"></select><button id="open-config-file" class="button quiet"><i data-lucide="external-link"></i><span>在文件管理器中打开</span></button></div>
        <p id="config-file-path" class="path-caption"></p>
        <textarea id="config-editor" class="config-editor" spellcheck="false" aria-label="配置文件内容"></textarea>
        <footer class="modal-footer"><span id="config-save-result"></span><button id="save-config-file" class="button primary"><i data-lucide="save"></i><span>保存文件</span></button></footer>
      </section>
    </div>

    <div id="plugin-dialog" class="modal" hidden>
      <div class="modal-backdrop" data-close-modal></div>
      <section class="modal-panel plugin-panel" role="dialog" aria-modal="true" aria-labelledby="plugin-title">
        <header class="modal-header"><div><p class="eyebrow">COMMUNITY CATALOG</p><h2 id="plugin-title">插件</h2></div><button class="modal-close" data-close-modal title="关闭"><i data-lucide="x"></i></button></header>
        <div class="plugin-install"><div class="input-with-icon"><i data-lucide="github"></i><input id="plugin-spec" placeholder="github:owner/repository" autocomplete="off" spellcheck="false"></div><button id="install-plugin" class="button primary"><i data-lucide="download"></i><span>安装</span></button></div>
        <p class="plugin-hint">安装会执行 <code>dsh plugin --profile web add &lt;spec&gt;</code>，服务会自动重启。目录来源：<a data-external href="https://dsh.aitreez.com/" rel="noreferrer">dsh.aitreez.com</a></p>
        <div class="plugin-tabs"><button class="plugin-tab active" data-plugin-tab="installed">已安装</button><button class="plugin-tab" data-plugin-tab="market">插件商店</button><button class="plugin-tab" data-plugin-tab="search">npm 搜索</button></div>
        <div id="installed-plugins" class="plugin-content"></div>
        <div id="market-plugins" class="plugin-content" hidden>
          <div class="market-toolbar">
            <div class="input-with-icon"><i data-lucide="search"></i><input id="market-search" placeholder="搜索名称、作者、简介或关键词" autocomplete="off" spellcheck="false"></div>
            <select id="market-type" aria-label="类型筛选"></select>
            <select id="market-category" aria-label="分类筛选"></select>
            <select id="market-sort" aria-label="排序"><option value="recent">最近活跃</option><option value="stars">Star 最多</option></select>
            <button id="market-refresh" class="icon-button" title="刷新商店数据"><i data-lucide="refresh-cw"></i></button>
          </div>
          <div id="market-list" class="market-list"></div>
          <footer class="market-footer">
            <span id="market-count"></span>
            <button id="market-more" class="button quiet" hidden>加载更多</button>
            <a data-external href="https://dsh.aitreez.com/" title="在浏览器打开插件商店">dsh.aitreez.com</a>
          </footer>
        </div>
        <div id="search-plugins" class="plugin-content" hidden><div class="plugin-search"><div class="input-with-icon"><i data-lucide="search"></i><input id="plugin-search-input" placeholder="搜索 dsh plugin"></div><button id="plugin-search-button" class="icon-button" title="搜索"><i data-lucide="search"></i></button></div><div id="plugin-search-results"></div></div>
        <pre id="plugin-output" class="operation-output" hidden></pre>
      </section>
    </div>
    <div id="settings-dialog" class="modal" hidden>
      <div class="modal-backdrop" data-close-modal></div>
      <section class="modal-panel settings-panel" role="dialog" aria-modal="true" aria-labelledby="settings-title">
        <header class="modal-header"><div><p class="eyebrow">DSH LAUNCHER</p><h2 id="settings-title">设置</h2></div><button class="modal-close" data-close-modal title="关闭"><i data-lucide="x"></i></button></header>
        <div class="settings-content">
          <div class="settings-section"><p class="eyebrow">WINDOW</p><h3>关闭按钮行为</h3><div class="segmented wide"><label><input type="radio" name="close_behavior" value="tray"><span>最小化到托盘</span></label><label><input type="radio" name="close_behavior" value="exit"><span>退出 Launcher</span></label></div></div>
          <label class="check-row"><input id="setting-auto-start" type="checkbox"><span><strong>启动后自动启动 dsh Web 服务</strong><small>Launcher 打开后立即启动本地服务</small></span></label>
          <label class="check-row"><input id="setting-stop-dsh" type="checkbox"><span><strong>退出 Launcher 时结束 dsh</strong><small>只结束由当前 Launcher 启动并登记的进程树</small></span></label>
          <div class="settings-section download-section"><p class="eyebrow">DOWNLOAD</p><h3>下载</h3>
            <div class="field full"><label for="setting-download-directory">默认下载目录</label><div class="path-input"><input id="setting-download-directory" autocomplete="off" spellcheck="false"><button id="browse-download-directory" type="button" class="icon-button" title="选择下载目录"><i data-lucide="folder-open"></i></button></div></div>
            <label class="check-row"><input id="setting-download-ask" type="checkbox"><span><strong>下载前确认</strong><small>每次下载前弹出确认提示</small></span></label>
            <label class="check-row compact"><input id="setting-download-choose" type="checkbox"><span><strong>下载前选择保存位置</strong><small>显示“另存为”对话框，可同时修改文件名</small></span></label>
          </div>
          <div class="settings-section about-section"><p class="eyebrow">ABOUT</p><h3>关于 DSH Launcher</h3><p class="about-copy">当前版本 <strong id="launcher-version">读取中</strong></p><div class="about-actions"><a class="button quiet" data-external href="https://github.com/WEP-56/DSH-Launcher"><i data-lucide="github"></i><span>GitHub 仓库</span></a><a class="button primary" data-external href="https://github.com/WEP-56/DSH-Launcher/releases"><i data-lucide="external-link"></i><span>前往 Release 下载更新</span></a></div></div>
          <label class="check-row compact update-check-row"><input id="setting-auto-check-updates" type="checkbox"><span><strong>启动时自动检查更新</strong><small>每次打开 Launcher 时检查 GitHub Release，不会自动下载</small></span></label>
          <div class="about-actions update-actions"><button id="check-launcher-update" class="button quiet"><i data-lucide="refresh-cw"></i><span>检查更新</span></button></div>
        </div>
        <footer class="modal-footer"><span id="settings-save-result"></span><button id="save-settings" class="button primary"><i data-lucide="save"></i><span>保存设置</span></button></footer>
      </section>
    </div>
    <div id="update-dialog" class="modal" hidden>
      <div class="modal-backdrop" data-close-modal></div>
      <section class="modal-panel update-panel" role="dialog" aria-modal="true" aria-labelledby="update-title">
        <header class="modal-header"><div><p class="eyebrow">DSH LAUNCHER</p><h2 id="update-title">发现新版本</h2></div><button class="modal-close" data-close-modal title="关闭"><i data-lucide="x"></i></button></header>
        <div class="update-content"><div class="update-version-line"><strong id="update-version"></strong><span id="update-published"></span></div><div id="update-body" class="update-body"></div></div>
        <footer class="modal-footer"><span>更新不会自动下载</span><a id="update-release-link" class="button primary" data-external href="#"><i data-lucide="external-link"></i><span>查看 Release</span></a></footer>
      </section>
    </div>
    <div id="toast" class="toast" role="status" hidden></div>
  </div>
`;

// 平台判断：macOS 上给 <body> 加 os-macos 类，仅用于切换原生红绿灯 + 工具栏右移布局。
// 用 navigator.userAgent 判断（Tauri 桌面 webview 的 UA 在 macOS 含 "Mac"），零 Rust/权限改动，
// 因此 Windows 永远不加该类、布局与作者原始版本完全一致。
if (/Mac/i.test(navigator.userAgent)) document.body.classList.add("os-macos");

createIcons({ icons: { CircleAlert, CircleCheck, Code2, Download, ExternalLink, Eye, FileCog, FileText, FolderOpen, Github, LoaderCircle, Minus, PackageCheck, Plus, Puzzle, RefreshCw, RotateCcw, Save, Search, Settings, SlidersHorizontal, Square, TerminalSquare, Trash2, Wrench, X } });
const $ = <T extends HTMLElement = HTMLInputElement>(selector: string): T => document.querySelector<T>(selector)!;
const currentWindow = getCurrentWindow();
let config: LauncherConfig;
let status: LauncherStatus = { phase: "stopped", message: "dsh 尚未启动", url: "http://127.0.0.1:3080", pid: null, external: false, logs: [] };
let configFiles: ConfigFileInfo[] = [];
let toastTimer: number | undefined;
let tabs: TabState[] = [];
let activeTab = 0;
let tabSequence = 0;
let tabNameSequence = 0;
let market: MarketCatalog | null = null;
const marketCategoryMeta: MarketMeta[] = [
  { id: "ui", label: "界面增强", color: "#a0c3ec" },
  { id: "agent-session", label: "Agent 与会话", color: "#c4b5fd" },
  { id: "development", label: "开发工具", color: "#ffffff" },
  { id: "communication", label: "消息通讯", color: "#ffc285" },
  { id: "data", label: "文件与数据", color: "#8ed6c4" },
  { id: "model-mcp", label: "模型与 MCP", color: "#9bb7ff" },
  { id: "security", label: "安全与治理", color: "#ff9c8c" },
  { id: "operations", label: "部署运维", color: "#d0d3d8" },
  { id: "lifestyle", label: "生活娱乐", color: "#ffb3d1" },
  { id: "research", label: "学习研究", color: "#b7d987" },
  { id: "other", label: "其他", color: "#7d8187" },
];
const marketTypeMeta: MarketMeta[] = [
  { id: "plugin", label: "插件" },
  { id: "skill", label: "技能" },
  { id: "collection", label: "插件合集" },
  { id: "channel", label: "渠道适配" },
  { id: "application", label: "完整应用" },
  { id: "infrastructure", label: "基础设施" },
  { id: "directory", label: "索引目录" },
  { id: "unknown", label: "待识别" },
];
const marketCategories = new Map(marketCategoryMeta.map((item) => [item.id, item]));
const marketTypes = new Map(marketTypeMeta.map((item) => [item.id, item]));
let marketShown = 0;
let marketSearchTimer: number | undefined;
let launcherVersion = "读取中";
let resizeSaveTimer: number | undefined;
const MARKET_PAGE = 120;
const marketFilter: { query: string; type: string; category: string; sort: "recent" | "stars" } = { query: "", type: "plugin", category: "all", sort: "recent" };

function toast(message: string, error = false): void {
  const node = $("#toast");
  node.textContent = message;
  node.dataset.error = String(error);
  node.hidden = false;
  window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => { node.hidden = true; }, 4200);
}

function showDialog(id: DialogId): void {
  $(`#${id}`).hidden = false;
  if (id === "manage-dialog") void refreshPackageInfo();
  if (id === "config-dialog") void loadConfigFiles();
  if (id === "plugin-dialog") void loadInstalledPlugins();
  if (id === "settings-dialog") fillSettings();
}
function closeDialogs(): void {
  document.querySelectorAll<HTMLElement>(".modal").forEach((modal) => { modal.hidden = true; });
}

function fillForm(value: LauncherConfig): void {
  $<HTMLInputElement>(`input[name="launch_mode"][value="${value.launch_mode}"]`).checked = true;
  $("#executable").value = value.executable;
  $("#npx-package").value = value.npx_package;
  $("#working-directory").value = value.working_directory;
  $("#dsh-home").value = value.dsh_home;
  $("#port").value = String(value.port);
  updateModeFields();
}
function fillSettings(): void {
  $<HTMLInputElement>(`input[name="close_behavior"][value="${config.close_behavior}"]`).checked = true;
  $("#setting-auto-start").checked = config.auto_start;
  $("#setting-stop-dsh").checked = config.stop_dsh_on_exit;
  $("#setting-download-directory").value = config.download_directory;
  $("#setting-download-ask").checked = config.download_ask;
  $("#setting-download-choose").checked = config.download_choose_location;
  $("#setting-auto-check-updates").checked = config.auto_check_updates;
  $("#launcher-version").textContent = launcherVersion;
  if (currentWindow.label === "control") $("#window-close").title = config.close_behavior === "tray" ? "关闭到托盘" : "退出 Launcher";
}
function readSettings(): LauncherConfig {
  return { ...config,
    close_behavior: ($<HTMLInputElement>("input[name=close_behavior]:checked")).value as "tray" | "exit",
    auto_start: $("#setting-auto-start").checked,
    stop_dsh_on_exit: $("#setting-stop-dsh").checked,
    download_directory: $("#setting-download-directory").value.trim(),
    download_ask: $("#setting-download-ask").checked,
    download_choose_location: $("#setting-download-choose").checked,
    auto_check_updates: $("#setting-auto-check-updates").checked,
  };
}
async function saveSettings(): Promise<void> {
  try {
    config = await invoke<LauncherConfig>("save_config", { config: readSettings() });
    fillForm(config);
    $("#settings-save-result").textContent = "已保存";
    toast("设置已保存");
  } catch (error) { toast(String(error), true); }
}
function persistWindowSize(width: number, height: number): void {
  window.clearTimeout(resizeSaveTimer);
  resizeSaveTimer = window.setTimeout(() => {
    config.window_width = Math.round(width);
    config.window_height = Math.round(height);
    void invoke("save_window_size", { width: Math.round(width), height: Math.round(height) });
  }, 350);
}
function readForm(): LauncherConfig {
  const mode = ($<HTMLInputElement>("input[name=launch_mode]:checked")).value as LaunchMode;
  return { ...config, launch_mode: mode, executable: $("#executable").value.trim(), npx_package: $("#npx-package").value.trim(), working_directory: $("#working-directory").value.trim(), dsh_home: $("#dsh-home").value.trim(), port: Number($("#port").value) };
}
function updateModeFields(): void {
  const npx = ($<HTMLInputElement>("input[name=launch_mode]:checked")).value === "npx";
  $("#command-field").hidden = npx;
  $("#npx-field").hidden = !npx;
}

// —— 标签页体系：每个标签一个 WebUI iframe，切换时保留状态 ——
const TAB_CLOSE_SVG = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="M18 6 6 18M6 6l12 12"/></svg>`;
const TAB_TEAR_DISTANCE = 36;

interface TabDrag { id: number; pointerId: number; startX: number; startY: number; started: boolean; torn: boolean; ghost: HTMLElement | null; nativePreview: boolean; }
let tabDrag: TabDrag | null = null;
let previewMoveFrame = 0;

function tabElement(id: number): HTMLElement | null {
  return document.querySelector<HTMLElement>(`.tab[data-tab-id="${id}"]`);
}

function renderTabs(): void {
  const strip = $("#tab-strip");
  strip.innerHTML = tabs.map((tab) => {
    const flags = `${tab.id === activeTab ? " active" : ""}${tabDrag?.started && tabDrag.id === tab.id ? " dragging" : ""}${tabDrag?.torn && tabDrag.id === tab.id ? " tearing" : ""}`;
    return `<div class="tab${flags}" data-tab-id="${tab.id}" role="tab" aria-selected="${tab.id === activeTab}" title="${escapeAttr(tab.title)}"><span class="tab-title">${escapeHtml(tab.title)}</span><button class="tab-close" title="关闭标签页" tabindex="-1">${TAB_CLOSE_SVG}</button></div>`;
  }).join("");
  strip.querySelectorAll<HTMLElement>(".tab").forEach((element) => {
    const id = Number(element.dataset.tabId);
    element.addEventListener("pointerdown", (event) => onTabPointerDown(event, id));
    element.addEventListener("auxclick", (event) => { if (event.button === 1) { event.preventDefault(); removeTab(id); } });
    element.addEventListener("dblclick", (event) => {
      if ((event.target as HTMLElement).closest(".tab-close")) return;
      const tab = tabs.find((item) => item.id === id);
      const titleElement = element.querySelector<HTMLElement>(".tab-title");
      if (tab && titleElement) startTabRename(tab, titleElement);
    });
    const close = element.querySelector<HTMLButtonElement>(".tab-close");
    close?.addEventListener("pointerdown", (event) => event.stopPropagation());
    close?.addEventListener("click", (event) => { event.stopPropagation(); removeTab(id); });
  });
  updateTabDensity();
}

function updateTabDensity(): void {
  // 三档收缩（阈值对应 styles.css 里 .tab 的各档 min-width）：标签再多也
  // 先压扁而不是溢出，压到极限才交给标签条横向滚动。
  const available = Math.max(0, $("#tab-zone").clientWidth - 46);
  const strip = $("#tab-strip");
  strip.classList.toggle("crowded", tabs.length * 88 > available);
  strip.classList.toggle("dense", tabs.length * 48 > available);
}

function syncFrames(): void {
  const ready = status.phase === "ready";
  $("#workspace-state").hidden = ready;
  for (const tab of tabs) {
    if (ready && tab.id === activeTab) {
      if (!tab.frame) {
        const frame = document.createElement("iframe");
        frame.className = "dsh-frame";
        frame.title = "DeepSeek Harness WebUI";
        frame.setAttribute("allow", "clipboard-read; clipboard-write");
        $("#workspace-view").appendChild(frame);
        // 新 iframe 从空白装载 WebUI 的过程会产生白屏，先隐藏，
        // 首次 load 完成后才揭晓（后续 src 重载复用同一监听）。
        frame.hidden = true;
        frame.addEventListener("load", () => {
          if (tab.id === activeTab) frame.hidden = false;
        });
        tab.frame = frame;
      }
      // 只在地址变化或被标记过期时装载。iframe.src 的 getter 会把地址规范化
      // （补上尾部斜杠），与 status.url 直接比较永远不相等，因此自己记录
      // loadedUrl，避免每条状态事件（包括日志推送）都触发一次 WebUI 重载。
      if (tab.loadedUrl !== status.url || tab.stale) {
        // 重载期间先隐藏、load 后揭晓，避免空白闪烁。
        tab.frame.hidden = true;
        tab.frame.src = status.url;
        tab.loadedUrl = status.url;
        tab.stale = false;
      } else {
        // 已加载完成的标签直接显示（切换回来不应闪屏）。
        tab.frame.hidden = false;
      }
    } else if (tab.frame) {
      tab.frame.hidden = true;
    }
  }
}

function addTab(title?: string): void {
  const id = ++tabSequence;
  tabs.push({ id, title: title?.trim() || `DSH ${++tabNameSequence}`, frame: null, loadedUrl: "", stale: false });
  activateTab(id);
}

function activateTab(id: number): void {
  if (!tabs.some((tab) => tab.id === id)) return;
  // 已激活时跳过重绘：双击重命名依赖两次点击落在同一个元素上。
  if (activeTab !== id) { activeTab = id; renderTabs(); }
  tabElement(id)?.scrollIntoView({ block: "nearest", inline: "nearest" });
  syncFrames();
}

function removeTab(id: number): void {
  const index = tabs.findIndex((tab) => tab.id === id);
  if (index < 0) return;
  const [removed] = tabs.splice(index, 1);
  removed.frame?.remove();
  if (!tabs.length) {
    // 最后一个标签：主窗口自动补一个新标签，新建的窗口直接关闭。
    if (currentWindow.label === "control") addTab();
    else void currentWindow.close();
    return;
  }
  if (activeTab === id) activeTab = tabs[Math.min(index, tabs.length - 1)].id;
  renderTabs();
  syncFrames();
}

function cycleTab(direction: number): void {
  if (tabs.length < 2) return;
  const index = tabs.findIndex((tab) => tab.id === activeTab);
  activateTab(tabs[(index + direction + tabs.length) % tabs.length].id);
}

function startTabRename(tab: TabState, titleElement: HTMLElement): void {
  const input = document.createElement("input");
  input.className = "tab-rename";
  input.value = tab.title;
  titleElement.replaceWith(input);
  input.focus();
  input.select();
  const commit = (): void => {
    const value = input.value.trim();
    if (value) tab.title = value;
    renderTabs();
  };
  input.addEventListener("blur", commit);
  input.addEventListener("keydown", (event) => {
    if (event.key === "Enter") input.blur();
    if (event.key === "Escape") { input.value = tab.title; input.blur(); }
    event.stopPropagation();
  });
  input.addEventListener("pointerdown", (event) => event.stopPropagation());
}

function onTabPointerDown(event: PointerEvent, id: number): void {
  if (event.button !== 0 || (event.target as HTMLElement).closest(".tab-close, .tab-rename")) return;
  activateTab(id);
  tabDrag = { id, pointerId: event.pointerId, startX: event.clientX, startY: event.clientY, started: false, torn: false, ghost: null, nativePreview: false };
}

function moveNativeTabPreview(): void {
  if (!tabDrag?.nativePreview || previewMoveFrame) return;
  previewMoveFrame = window.requestAnimationFrame(() => {
    previewMoveFrame = 0;
    if (tabDrag?.nativePreview) void invoke("move_tab_drag_preview");
  });
}

function setTabTorn(torn: boolean, x: number, y: number): void {
  if (!tabDrag) return;
  if (tabDrag.torn !== torn) {
    tabDrag.torn = torn;
    tabElement(tabDrag.id)?.classList.toggle("tearing", torn);
    $("#tab-drop-indicator").hidden = torn;
    tabDrag.ghost?.classList.toggle("outside-window", torn);
    const tab = tabs.find((item) => item.id === tabDrag!.id);
    if (torn) {
      tabDrag.nativePreview = true;
      void invoke("show_tab_drag_preview", { title: tab?.title ?? "" });
    } else if (tabDrag.nativePreview) {
      tabDrag.nativePreview = false;
      void invoke("hide_tab_drag_preview");
    }
  }
  if (tabDrag.ghost) tabDrag.ghost.style.transform = `translate(${x + 14}px, ${y + 10}px)`;
  if (torn) moveNativeTabPreview();
}

function ensureTabGhost(): void {
  if (!tabDrag || tabDrag.ghost) return;
  const tab = tabs.find((item) => item.id === tabDrag!.id);
  const ghost = document.createElement("div");
  ghost.className = "tab-ghost";
  ghost.innerHTML = `<span class="tab-ghost-icon">◉</span><strong>${escapeHtml(tab?.title ?? "")}</strong>`;
  document.body.append(ghost);
  tabDrag.ghost = ghost;
}

function updateTabDropIndicator(x: number): void {
  if (!tabDrag || tabDrag.torn) return;
  const indicator = $("#tab-drop-indicator");
  const zone = $("#tab-zone").getBoundingClientRect();
  const others = tabs.filter((tab) => tab.id !== tabDrag!.id);
  let left = zone.left + 8;
  for (const other of others) {
    const rect = tabElement(other.id)?.getBoundingClientRect();
    if (!rect) continue;
    if (x < rect.left + rect.width / 2) { left = rect.left; break; }
    left = rect.right;
  }
  indicator.style.transform = `translateX(${Math.round(left - zone.left)}px)`;
  indicator.hidden = false;
}

function reorderDraggedTab(x: number): void {
  if (!tabDrag) return;
  const dragId = tabDrag.id;
  const dragged = tabs.find((tab) => tab.id === dragId);
  if (!dragged) return;
  const others = tabs.filter((tab) => tab.id !== dragId);
  let insert = others.length;
  for (let index = 0; index < others.length; index += 1) {
    const rect = tabElement(others[index].id)?.getBoundingClientRect();
    if (rect && x < rect.left + rect.width / 2) { insert = index; break; }
  }
  const next = [...others.slice(0, insert), dragged, ...others.slice(insert)];
  // 拖拽过程中只搬移 DOM 节点（O(n) appendChild，不重建、不重绑事件），避免每次
  // 跨过中点就整条 innerHTML 重建导致的卡顿；最终顺序由 finishTabDrag 的 renderTabs 归一。
  if (next.some((tab, index) => tab !== tabs[index])) {
    tabs = next;
    const strip = $("#tab-strip");
    for (const tab of tabs) {
      const el = tabElement(tab.id);
      if (el) strip.appendChild(el);
    }
    tabElement(dragId)?.classList.add("dragging");
  }
  updateTabDropIndicator(x);
}

function finishTabDrag(event: PointerEvent, cancelled: boolean): void {
  if (!tabDrag || event.pointerId !== tabDrag.pointerId) return;
  const drag = tabDrag;
  tabDrag = null;
  if (previewMoveFrame) window.cancelAnimationFrame(previewMoveFrame);
  previewMoveFrame = 0;
  if (drag.nativePreview) void invoke("hide_tab_drag_preview");
  $("#tab-drop-indicator").hidden = true;
  // 纯点击（没拖起来）不重绘标签条：双击重命名依赖两次点击落在同一个元素上。
  if (!drag.started) return;
  drag.ghost?.remove();
  document.body.classList.remove("tab-dragging");
  const tab = tabs.find((item) => item.id === drag.id);
  if (!tab || cancelled || !drag.torn) { renderTabs(); return; }
  // 松手在窗口顶栏带之外：交给后端按光标位置决定“并入其他窗口”还是“拖出成新窗口”。
  void (async () => {
    try {
      const outcome = await invoke<string>("drop_tab", { title: tab.title, remaining: tabs.length });
      if (outcome === "adopted" || outcome === "detached") removeTab(tab.id);
      else renderTabs();
    } catch (error) {
      toast(String(error), true);
      renderTabs();
    }
  })();
}

window.addEventListener("pointermove", (event) => {
  if (!tabDrag || event.pointerId !== tabDrag.pointerId) return;
  if (!tabDrag.started) {
    if (Math.hypot(event.clientX - tabDrag.startX, event.clientY - tabDrag.startY) < 5) return;
    tabDrag.started = true;
    // 拖动真正开始才接管指针：捕获挂在不会被重建的容器上（标签条重排时会
    // 整体重绘），并且不影响未拖动时 click/dblclick 的正常派发。
    try { $("#tab-zone").setPointerCapture(event.pointerId); } catch { /* 不支持时退化为窗口内拖拽 */ }
    document.body.classList.add("tab-dragging");
    tabElement(tabDrag.id)?.classList.add("dragging");
    ensureTabGhost();
  }
  const zone = $("#tab-zone").getBoundingClientRect();
  const inBand = event.clientY >= zone.top - TAB_TEAR_DISTANCE && event.clientY <= zone.bottom + TAB_TEAR_DISTANCE
    && event.clientX >= -12 && event.clientX <= window.innerWidth + 12;
  if (inBand) {
    setTabTorn(false, event.clientX, event.clientY);
    reorderDraggedTab(event.clientX);
  } else {
    setTabTorn(true, event.clientX, event.clientY);
  }
  if (tabDrag.ghost) tabDrag.ghost.style.transform = `translate(${event.clientX + 14}px, ${event.clientY + 10}px)`;
});
window.addEventListener("pointerup", (event) => finishTabDrag(event, false));
window.addEventListener("pointercancel", (event) => finishTabDrag(event, true));

function renderStatus(next: LauncherStatus): void {
  // 去重守卫：launcher-status 事件高频可达（日志流/状态轮询），若关键字段未变则
  // 跳过本次整轮 DOM 更新，避免冗余重绘导致的主线程抖动。
  if (next.phase === status.phase && next.message === status.message && next.url === status.url && next.pid === status.pid && next.external === status.external) return;
  const wasReady = status.phase === "ready";
  status = next;
  const externalReady = next.phase === "ready" && next.external;
  $("#service-state").textContent = next.phase === "ready" ? (next.external ? "运行中（外部）" : "运行中") : next.phase === "starting" ? "启动中" : next.phase === "failed" ? "启动失败" : "已停止";
  const state = $("#workspace-state");
  state.dataset.phase = next.phase;
  $("#workspace-title").textContent = next.phase === "failed" ? "dsh 启动失败" : next.phase === "starting" ? "正在启动 dsh" : "dsh 尚未运行";
  $("#workspace-message").textContent = next.message;
  const startButton = $<HTMLButtonElement>("#workspace-start");
  startButton.toggleAttribute("disabled", next.phase === "starting" || next.phase === "stopping");
  const actionLabel = next.phase === "failed" ? "重启 dsh" : next.phase === "starting" ? "正在连接 dsh" : "启动 dsh";
  startButton.title = actionLabel;
  startButton.ariaLabel = actionLabel;
  // 外部启动的服务不归 Launcher 管：停止/重启在这里没有意义，禁用并说明。
  const restartButton = $<HTMLButtonElement>("#manage-restart");
  const stopButton = $<HTMLButtonElement>("#manage-stop");
  restartButton.toggleAttribute("disabled", next.phase === "starting" || next.phase === "stopping" || externalReady);
  stopButton.toggleAttribute("disabled", next.phase === "stopped" || next.phase === "stopping" || externalReady);
  restartButton.title = externalReady ? "服务由外部启动，Launcher 无法重启" : "重启服务";
  stopButton.title = externalReady ? "服务由外部启动，Launcher 无法停止" : "停止服务";
  // 进入就绪的瞬间把所有标签标记为待重载：服务可能重启过或换了端口，旧
  // iframe 内容已失效；后台标签等到被激活时再各自重载。
  if (next.phase === "ready" && !wasReady) tabs.forEach((tab) => { tab.stale = true; });
  syncFrames();
}
function refreshWeb(): void {
  const tab = tabs.find((item) => item.id === activeTab);
  if (status.phase !== "ready" || !tab?.frame || !tab.loadedUrl) { toast("dsh Web 服务尚未就绪", true); return; }
  // 跨域 iframe 访问 contentWindow.location 会抛 SecurityError，重新赋值 src 触发刷新。
  tab.frame.src = tab.loadedUrl;
}

async function runAction(command: "start_dsh" | "stop_dsh" | "restart_dsh"): Promise<void> {
  try { renderStatus(await invoke<LauncherStatus>(command)); } catch (error) { toast(String(error), true); }
}
async function refreshPackageInfo(): Promise<void> {
  $("#package-source").textContent = "检查中…";
  try {
    const info = await invoke<PackageInfo>("get_package_info");
    $("#current-version").textContent = info.current_version;
    $("#latest-version").textContent = info.latest_version;
    $("#package-source").textContent = info.source;
  } catch (error) { $("#package-source").textContent = "检查失败"; toast(String(error), true); }
}

function formatReleaseDate(value: string): string {
  const date = Date.parse(value);
  return Number.isNaN(date) ? "" : new Intl.DateTimeFormat("zh-CN", { year: "numeric", month: "short", day: "numeric" }).format(date);
}

function showReleaseDialog(release: ReleaseInfo): void {
  $("#update-title").textContent = release.name.trim() || "发现新版本";
  $("#update-version").textContent = release.tag_name || release.latest_version;
  $("#update-published").textContent = formatReleaseDate(release.published_at);
  const body = release.body.trim();
  $("#update-body").innerHTML = body ? escapeHtml(body).replace(/\r?\n/g, "<br>") : "此版本没有发布说明。";
  const link = $<HTMLAnchorElement>("#update-release-link");
  link.href = release.html_url;
  $("#update-dialog").hidden = false;
}

async function checkLauncherUpdate(showNoUpdate = false): Promise<void> {
  const button = $<HTMLButtonElement>("#check-launcher-update");
  button.disabled = true;
  try {
    const release = await invoke<ReleaseInfo>("check_launcher_update");
    if (release.update_available) {
      showReleaseDialog(release);
      toast(`发现 Launcher 新版本 ${release.latest_version}`);
    } else if (showNoUpdate) {
      toast(`当前已是最新版本（${release.current_version}）`);
    }
  } catch (error) {
    if (showNoUpdate) toast(`检查更新失败：${String(error)}`, true);
  } finally {
    button.disabled = false;
  }
}
async function saveStartupConfig(): Promise<void> {
  try {
    config = await invoke<LauncherConfig>("save_config", { config: readForm() });
    await runAction("restart_dsh");
    toast("启动配置已保存");
  } catch (error) { toast(String(error), true); }
}

async function loadConfigFiles(): Promise<void> {
  try {
    configFiles = await invoke<ConfigFileInfo[]>("list_config_files");
    const select = $("#config-file-select");
    select.innerHTML = configFiles.map((file) => `<option value="${file.id}">${file.name}</option>`).join("");
    renderConfigFile();
  } catch (error) { toast(String(error), true); }
}
function renderConfigFile(): void {
  const file = configFiles.find((item) => item.id === $("#config-file-select").value) ?? configFiles[0];
  if (!file) return;
  $("#config-file-path").textContent = file.path;
  $("#config-editor").value = file.content;
}
async function saveConfigFile(): Promise<void> {
  const id = $("#config-file-select").value;
  try {
    const file = await invoke<ConfigFileInfo>("save_dsh_config", { id, content: $("#config-editor").value });
    configFiles = configFiles.map((item) => item.id === file.id ? file : item);
    $("#config-save-result").textContent = "已保存";
    toast(`${file.name} 已保存`);
  } catch (error) { $("#config-save-result").textContent = String(error); toast(String(error), true); }
}

async function loadInstalledPlugins(): Promise<void> {
  const node = $("#installed-plugins");
  node.innerHTML = `<div class="loading"><i data-lucide="loader-circle"></i>正在读取 profile</div>`;
  createIcons({ icons: { LoaderCircle } });
  try {
    const plugins = await invoke<InstalledPlugin[]>("list_plugins");
    node.innerHTML = plugins.length ? plugins.map((plugin) => `<article class="plugin-item"><div><strong>${escapeHtml(plugin.name)}</strong><small>${escapeHtml(plugin.version)}${plugin.bundle ? " · bundle" : ""}</small></div><button class="icon-button danger remove-plugin" data-spec="${escapeAttr(plugin.name)}" title="卸载"><i data-lucide="trash-2"></i></button></article>`).join("") : `<div class="empty-state"><i data-lucide="puzzle"></i><p>Web profile 暂无额外插件</p><small>从插件商店选择一个 GitHub 项目开始。</small></div>`;
    createIcons({ icons: { Puzzle, Trash2 } });
    node.querySelectorAll<HTMLButtonElement>(".remove-plugin").forEach((button) => button.addEventListener("click", () => void removePlugin(button.dataset.spec ?? "")));
  } catch (error) { node.textContent = String(error); }
}
async function installPlugin(spec = $("#plugin-spec").value.trim()): Promise<void> {
  if (!spec) { toast("请输入 npm 包名或 github:owner/repository", true); return; }
  const output = $("#plugin-output"); output.hidden = false; output.textContent = `正在安装 ${spec}…`;
  try {
    const result = await invoke<OperationResult>("install_plugin", { spec });
    output.textContent = result.output || (result.success ? "安装完成" : "安装失败");
    toast(result.success ? (status.external ? "插件安装完成（当前沿用外部服务，需自行重启该服务生效）" : "插件安装完成，服务已重新启动") : "插件安装失败", !result.success);
    if (result.success) { $("#plugin-spec").value = ""; await loadInstalledPlugins(); }
  } catch (error) { output.textContent = String(error); toast(String(error), true); }
}
async function removePlugin(spec: string): Promise<void> {
  const output = $("#plugin-output"); output.hidden = false; output.textContent = `正在卸载 ${spec}…`;
  try {
    const result = await invoke<OperationResult>("remove_plugin", { spec });
    output.textContent = result.output || (result.success ? "卸载完成" : "卸载失败");
    toast(result.success ? (status.external ? "插件已卸载（当前沿用外部服务，需自行重启该服务生效）" : "插件已卸载，服务已重新启动") : "插件卸载失败", !result.success);
    if (result.success) await loadInstalledPlugins();
  } catch (error) { output.textContent = String(error); toast(String(error), true); }
}
async function searchPlugins(): Promise<void> {
  const node = $("#plugin-search-results"); node.innerHTML = `<div class="loading">正在查询 npm…</div>`;
  try {
    const results = await invoke<PluginSearchResult[]>("search_plugins", { query: $("#plugin-search-input").value.trim() });
    node.innerHTML = results.length ? results.map((item) => `<article class="search-item"><div><strong>${escapeHtml(item.name)}</strong><small>v${escapeHtml(item.version)}</small><p>${escapeHtml(item.description)}</p></div><button class="button quiet search-install" data-spec="${escapeAttr(item.name)}"><i data-lucide="download"></i><span>安装</span></button></article>`).join("") : `<div class="empty-state">没有找到带 dsh plugin 标签的 npm 包。</div>`;
    createIcons({ icons: { Download } });
    node.querySelectorAll<HTMLButtonElement>(".search-install").forEach((button) => button.addEventListener("click", () => void installPlugin(button.dataset.spec ?? "")));
  } catch (error) { node.textContent = String(error); }
}
function escapeHtml(value: string): string { return value.replace(/[&<>"']/g, (char) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#039;" }[char] ?? char)); }
function escapeAttr(value: string): string { return escapeHtml(value).replace(/`/g, "&#096;"); }

// —— 插件商店（数据来自 dsh.aitreez.com API，后端获取并缓存）——
function safeColor(value: string | null | undefined): string {
  return value && /^#[0-9a-fA-F]{3,8}$/.test(value) ? value : "#7d8187";
}
function safeHttpUrl(value: string): string {
  return /^https?:\/\//i.test(value) ? value : "";
}
function formatStars(count: number): string {
  return count >= 1000 ? `${(count / 1000).toFixed(1).replace(/\.0$/, "")}k` : String(count);
}
function timeAgo(iso: string): string {
  const time = Date.parse(iso);
  if (Number.isNaN(time)) return "";
  const days = Math.floor((Date.now() - time) / 86_400_000);
  if (days <= 0) return "今天活跃";
  if (days < 30) return `${days} 天前活跃`;
  if (days < 365) return `${Math.floor(days / 30)} 个月前活跃`;
  return `${Math.floor(days / 365)} 年前活跃`;
}

function marketCard(plugin: MarketPlugin): string {
  const category = marketCategories.get(plugin.category);
  const typeLabel = marketTypes.get(plugin.project_type)?.label;
  const badges = [
    plugin.verified ? `<span class="market-badge verified">已验证</span>` : "",
    plugin.archived ? `<span class="market-badge archived">已归档</span>` : "",
    category ? `<span class="market-badge tinted" style="--badge-color:${safeColor(category.color)}">${escapeHtml(category.label)}</span>` : "",
    typeLabel && marketFilter.type === "all" ? `<span class="market-badge">${escapeHtml(typeLabel)}</span>` : "",
  ].join("");
  const meta = [`★ ${formatStars(plugin.stars)}`, plugin.language, timeAgo(plugin.pushed_at)]
    .filter(Boolean).map((part) => escapeHtml(String(part))).join(" · ");
  const avatar = safeHttpUrl(plugin.avatar_url);
  const repo = safeHttpUrl(plugin.url);
  return `<article class="market-item">
    <img class="market-avatar" ${avatar ? `src="${escapeAttr(avatar)}"` : ""} alt="" loading="lazy">
    <div class="market-info">
      <div class="market-title"><strong>${escapeHtml(plugin.name)}</strong><small>${escapeHtml(plugin.full_name)}</small>${badges}</div>
      <p class="market-desc">${escapeHtml(plugin.description || "无描述")}</p>
      <div class="market-meta">${meta}</div>
    </div>
    <div class="market-actions">
      ${repo ? `<a class="icon-button" data-external href="${escapeAttr(repo)}" title="在 GitHub 查看"><i data-lucide="external-link"></i></a>` : ""}
      <button class="button quiet market-install" data-spec="${escapeAttr(plugin.spec)}" title="安装到 web profile"><i data-lucide="download"></i><span>安装</span></button>
    </div>
  </article>`;
}

function filteredMarket(): MarketPlugin[] {
  if (!market) return [];
  let items = market.plugins;
  if (marketFilter.type !== "all") items = items.filter((plugin) => plugin.project_type === marketFilter.type);
  if (marketFilter.category !== "all") items = items.filter((plugin) => plugin.category === marketFilter.category);
  if (marketFilter.query) {
    const query = marketFilter.query;
    items = items.filter((plugin) =>
      `${plugin.name} ${plugin.full_name} ${plugin.description} ${plugin.language} ${plugin.topics.join(" ")}`.toLowerCase().includes(query));
  }
  if (marketFilter.sort === "stars") {
    items = [...items].sort((a, b) => b.stars - a.stars);
  } else {
    items = [...items].sort((a, b) => {
      const aTime = Date.parse(a.pushed_at);
      const bTime = Date.parse(b.pushed_at);
      if (Number.isNaN(aTime)) return Number.isNaN(bTime) ? 0 : 1;
      if (Number.isNaN(bTime)) return -1;
      return bTime - aTime;
    });
  }
  return items;
}

function renderMarketList(): void {
  if (!market) return;
  const node = $("#market-list");
  const items = filteredMarket();
  const visible = items.slice(0, marketShown);
  node.innerHTML = visible.length
    ? visible.map(marketCard).join("")
    : `<div class="empty-state"><i data-lucide="puzzle"></i><p>没有匹配的插件</p><small>换个关键词或筛选条件试试。</small></div>`;
  createIcons({ icons: { Download, ExternalLink, Puzzle } });
  node.querySelectorAll<HTMLImageElement>(".market-avatar[src]").forEach((img) =>
    img.addEventListener("error", () => img.removeAttribute("src")));
  node.querySelectorAll<HTMLButtonElement>(".market-install").forEach((button) =>
    button.addEventListener("click", async () => {
      button.disabled = true;
      try { await installPlugin(button.dataset.spec ?? ""); } finally { button.disabled = false; }
    }));
  $("#market-count").textContent = `${items.length} 个项目 · 共收录 ${market.plugins.length}`;
  const more = $("#market-more");
  more.hidden = items.length <= marketShown;
  if (!more.hidden) more.textContent = `加载更多（还有 ${items.length - marketShown} 个）`;
}

function resetMarketList(): void {
  marketShown = MARKET_PAGE;
  renderMarketList();
}

function populateMarketFilters(): void {
  if (!market) return;
  const typeCounts = new Map<string, number>();
  const categoryCounts = new Map<string, number>();
  for (const plugin of market.plugins) {
    typeCounts.set(plugin.project_type, (typeCounts.get(plugin.project_type) ?? 0) + 1);
    categoryCounts.set(plugin.category, (categoryCounts.get(plugin.category) ?? 0) + 1);
  }
  const typeSelect = $<HTMLSelectElement>("#market-type");
  typeSelect.innerHTML = [
    `<option value="all">全部类型 (${market.plugins.length})</option>`,
    ...marketTypeMeta.filter((item) => typeCounts.has(item.id))
      .map((item) => `<option value="${escapeAttr(item.id)}">${escapeHtml(item.label)} (${typeCounts.get(item.id)})</option>`),
  ].join("");
  if (marketFilter.type !== "all" && !typeCounts.has(marketFilter.type)) marketFilter.type = "all";
  typeSelect.value = marketFilter.type;
  const categorySelect = $<HTMLSelectElement>("#market-category");
  categorySelect.innerHTML = [
    `<option value="all">全部分类</option>`,
    ...marketCategoryMeta.filter((item) => categoryCounts.has(item.id))
      .map((item) => `<option value="${escapeAttr(item.id)}">${escapeHtml(item.label)} (${categoryCounts.get(item.id)})</option>`),
  ].join("");
  if (marketFilter.category !== "all" && !categoryCounts.has(marketFilter.category)) marketFilter.category = "all";
  categorySelect.value = marketFilter.category;
}

async function loadMarket(force: boolean): Promise<boolean> {
  const node = $("#market-list");
  if (!market) {
    node.innerHTML = `<div class="loading"><i data-lucide="loader-circle"></i>正在载入插件商店…</div>`;
    createIcons({ icons: { LoaderCircle } });
  }
  try {
    market = await invoke<MarketCatalog>("fetch_market", { force });
    populateMarketFilters();
    resetMarketList();
    return true;
  } catch (error) {
    if (market) { toast(String(error), true); return false; }
    node.innerHTML = `<div class="empty-state"><i data-lucide="circle-alert"></i><p>插件商店加载失败</p><small>${escapeHtml(String(error))}</small><button id="market-retry" class="button quiet">重试</button></div>`;
    createIcons({ icons: { CircleAlert } });
    $("#market-retry").addEventListener("click", () => void loadMarket(true));
    return false;
  }
}

document.querySelectorAll<HTMLButtonElement>("[data-dialog]").forEach((button) => button.addEventListener("click", () => showDialog(button.dataset.dialog as DialogId)));
document.querySelectorAll<HTMLElement>("[data-close-modal]").forEach((node) => node.addEventListener("click", closeDialogs));
document.querySelectorAll<HTMLButtonElement>(".modal-close").forEach((button) => button.addEventListener("click", closeDialogs));
document.querySelectorAll<HTMLInputElement>("input[name=launch_mode]").forEach((input) => input.addEventListener("change", updateModeFields));
$("#config-file-select").addEventListener("change", renderConfigFile);
$("#save-config-file").addEventListener("click", () => void saveConfigFile());
$("#open-config-file").addEventListener("click", () => void invoke("open_dsh_config", { id: $("#config-file-select").value }));
$("#manage-save").addEventListener("click", () => void saveStartupConfig());
$("#save-settings").addEventListener("click", () => void saveSettings());
$("#check-launcher-update").addEventListener("click", () => void checkLauncherUpdate(true));
$("#check-package").addEventListener("click", () => void refreshPackageInfo());
$("#update-package").addEventListener("click", async () => {
  const output = $("#manage-output");
  output.hidden = false;
  output.textContent = "正在更新 dsh…（运行中的服务会先停止，更新后自动恢复）";
  try {
    const result = await invoke<OperationResult>("update_dsh");
    output.textContent = result.output || (result.success ? "更新完成" : "更新失败");
    toast(result.success ? "dsh 更新完成" : "dsh 更新失败", !result.success);
    if (result.success) await refreshPackageInfo();
  } catch (error) {
    output.textContent = String(error);
    toast(String(error), true);
  }
});
$("#manage-restart").addEventListener("click", () => void runAction("restart_dsh"));
$("#manage-stop").addEventListener("click", () => void runAction("stop_dsh"));
$("#workspace-start").addEventListener("click", () => void runAction("start_dsh"));
$("#plugin-spec").addEventListener("keydown", (event) => { if (event.key === "Enter") void installPlugin(); });
$("#install-plugin").addEventListener("click", () => void installPlugin());
$("#plugin-search-button").addEventListener("click", () => void searchPlugins());
$("#plugin-search-input").addEventListener("keydown", (event) => { if (event.key === "Enter") void searchPlugins(); });
document.querySelectorAll<HTMLButtonElement>(".plugin-tab").forEach((tab) => tab.addEventListener("click", () => {
  document.querySelectorAll<HTMLButtonElement>(".plugin-tab").forEach((item) => item.classList.toggle("active", item === tab));
  document.querySelectorAll<HTMLElement>(".plugin-content").forEach((item) => { item.hidden = item.id !== `${tab.dataset.pluginTab}-plugins`; });
  if (tab.dataset.pluginTab === "market" && !market) void loadMarket(false);
}));
$("#market-search").addEventListener("input", () => {
  window.clearTimeout(marketSearchTimer);
  marketSearchTimer = window.setTimeout(() => {
    marketFilter.query = $("#market-search").value.trim().toLowerCase();
    resetMarketList();
  }, 160);
});
$<HTMLSelectElement>("#market-type").addEventListener("change", () => { marketFilter.type = $<HTMLSelectElement>("#market-type").value; resetMarketList(); });
$<HTMLSelectElement>("#market-category").addEventListener("change", () => { marketFilter.category = $<HTMLSelectElement>("#market-category").value; resetMarketList(); });
$<HTMLSelectElement>("#market-sort").addEventListener("change", () => { marketFilter.sort = $<HTMLSelectElement>("#market-sort").value as "recent" | "stars"; resetMarketList(); });
$("#market-refresh").addEventListener("click", async () => {
  const button = $<HTMLButtonElement>("#market-refresh");
  button.disabled = true;
  try { if (await loadMarket(true)) toast("插件商店数据已刷新"); } finally { button.disabled = false; }
});
$("#market-more").addEventListener("click", () => { marketShown += MARKET_PAGE; renderMarketList(); });
// webview 里外部链接不会自己打开，统一交给系统浏览器。
document.addEventListener("click", (event) => {
  const anchor = (event.target as HTMLElement | null)?.closest?.("a[data-external]");
  if (anchor instanceof HTMLAnchorElement && anchor.href) {
    event.preventDefault();
    void invoke("open_external", { url: anchor.href }).catch((error) => toast(String(error), true));
  }
});
$("#browse-workspace").addEventListener("click", async () => { const selected = await open({ directory: true, multiple: false, defaultPath: $("#working-directory").value || undefined }); if (typeof selected === "string") $("#working-directory").value = selected; });
$("#browse-download-directory").addEventListener("click", async () => { const selected = await open({ directory: true, multiple: false, defaultPath: $("#setting-download-directory").value || undefined }); if (typeof selected === "string") $("#setting-download-directory").value = selected; });
$("#refresh-web").addEventListener("click", refreshWeb);
$("#tab-add").addEventListener("click", () => addTab());
// 标签多到放不下时用滚轮横向滚动标签条。
$("#tab-strip").addEventListener("wheel", (event) => {
  const strip = $("#tab-strip");
  if (strip.scrollWidth > strip.clientWidth) { event.preventDefault(); strip.scrollLeft += event.deltaY; }
}, { passive: false });
// 中键按下默认会进入自动滚动模式，抢在标签的中键关闭之前按掉。
$("#tab-strip").addEventListener("mousedown", (event) => { if (event.button === 1) event.preventDefault(); });
let resizeRaf = 0;
window.addEventListener("resize", () => {
  // rAF 节流：窗口拖动缩放时 resize 高频触发，合并到下一帧只算一次，避免连续重排。
  if (resizeRaf) return;
  resizeRaf = window.requestAnimationFrame(() => { resizeRaf = 0; updateTabDensity(); });
});
$("#new-window").addEventListener("click", () => void invoke("new_launcher_window").catch((error) => toast(String(error), true)));
$("#window-minimize").addEventListener("click", () => void currentWindow.minimize());
$("#window-maximize").addEventListener("click", () => void currentWindow.toggleMaximize());
$("#window-close").addEventListener("click", () => void currentWindow.close());
// 只有主窗口关闭时驻留托盘，新建窗口是真正关闭。
if (currentWindow.label !== "control") $("#window-close").title = "关闭窗口";
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape") { closeDialogs(); return; }
  if (!event.ctrlKey || event.altKey || event.metaKey) return;
  const key = event.key.toLowerCase();
  if (key === "t" && !event.shiftKey) { event.preventDefault(); addTab(); }
  else if (key === "w" && !event.shiftKey) { event.preventDefault(); removeTab(activeTab); }
  else if (key === "tab") { event.preventDefault(); cycleTab(event.shiftKey ? -1 : 1); }
});

async function revealWindow(): Promise<void> {
  // 窗口以隐藏状态创建（tauri.conf.json / spawn_launcher_window），首帧内容
  // 就绪后才显示，避免 WebView 初始化期间的白屏；失败时由后端兜底定时器接管。
  try {
    await currentWindow.show();
    await currentWindow.setFocus();
  } catch { /* 后端兜底 */ }
}

async function init(): Promise<void> {
  await listen<LauncherStatus>("launcher-status", (event) => renderStatus(event.payload));
  // 其他窗口把标签拖到本窗口顶栏时，由后端路由过来收编。
  await listen<{ title: string }>("adopt-tab", (event) => addTab(event.payload.title));
  const [loadedConfig, loadedStatus, version, initialTab] = await Promise.all([
    invoke<LauncherConfig>("load_config"),
    invoke<LauncherStatus>("get_status"),
    invoke<string>("get_launcher_version"),
    invoke<string | null>("take_initial_tab"),
  ]);
  config = loadedConfig;
  launcherVersion = version;
  if (currentWindow.label === "control") {
    const width = Math.max(620, Math.min(4096, config.window_width || 880));
    const height = Math.max(560, Math.min(4096, config.window_height || 760));
    await currentWindow.setSize(new LogicalSize(width, height));
    await currentWindow.onResized(async ({ payload }) => {
      if (await currentWindow.isMaximized() || await currentWindow.isFullscreen()) return;
      const logical = payload.toLogical(await currentWindow.scaleFactor());
      persistWindowSize(logical.width, logical.height);
    });
  }
  // 拖出标签生成的窗口带着原标签的标题，普通窗口用默认名。
  addTab(initialTab ?? undefined);
  fillForm(config);
  fillSettings();
  renderStatus(loadedStatus);
  await revealWindow();
  if (currentWindow.label === "control" && config.auto_check_updates) void checkLauncherUpdate(false);
  if (config.auto_start && loadedStatus.phase === "stopped") await runAction("start_dsh");
}
void init().catch((error) => { void revealWindow(); toast(String(error), true); });
