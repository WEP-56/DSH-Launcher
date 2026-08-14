import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";
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
type DialogId = "manage-dialog" | "config-dialog" | "plugin-dialog";

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
}

interface LauncherStatus {
  phase: Phase;
  message: string;
  url: string;
  pid: number | null;
  logs: string[];
}

interface PackageInfo {
  current_version: string;
  latest_version: string;
  source: string;
  checked_at: string;
  detail: string;
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
interface MarketCatalog { plugins: MarketPlugin[]; categories: MarketMeta[]; types: MarketMeta[]; fetched_at: number; source: string; }

const app = document.querySelector<HTMLDivElement>("#app")!;
app.innerHTML = `
  <div class="shell">
    <header class="titlebar" data-tauri-drag-region>
      <nav class="toolbar-left" aria-label="DSH Launcher 工具">
        <button class="tool-button" data-dialog="manage-dialog" title="管理"><i data-lucide="package-check"></i><span>管理</span></button>
        <button class="tool-button" data-dialog="config-dialog" title="配置"><i data-lucide="sliders-horizontal"></i><span>配置</span></button>
        <button class="tool-button" data-dialog="plugin-dialog" title="插件"><i data-lucide="puzzle"></i><span>插件</span></button>
        <button id="new-window" class="tool-button" title="新建 Launcher 窗口"><i data-lucide="plus"></i><span>新建</span></button>
      </nav>
      <div class="title-drag" data-tauri-drag-region></div>
      <nav class="window-controls" aria-label="窗口控制">
        <button id="refresh-web" title="刷新 dsh WebUI"><i data-lucide="refresh-cw"></i></button>
        <button id="window-minimize" title="最小化"><i data-lucide="minus"></i></button>
        <button id="window-maximize" title="最大化"><i data-lucide="square"></i></button>
        <button id="window-close" class="close" title="关闭到托盘"><i data-lucide="x"></i></button>
      </nav>
    </header>

    <main class="workspace-view">
      <iframe id="dsh-frame" title="DeepSeek Harness WebUI" allow="clipboard-read; clipboard-write" hidden></iframe>
      <div id="workspace-state" class="workspace-state">
        <div class="workspace-state-icon"><i data-lucide="terminal-square"></i></div>
        <h2 id="workspace-title">正在准备 dsh</h2>
        <p id="workspace-message">启动器会在服务就绪后载入 WebUI。</p>
        <button id="workspace-start" class="button primary"><i data-lucide="rotate-ccw"></i><span>启动服务</span></button>
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
    <div id="toast" class="toast" role="status" hidden></div>
  </div>
`;

createIcons({ icons: { CircleAlert, CircleCheck, Code2, Download, ExternalLink, Eye, FileCog, FileText, FolderOpen, Github, LoaderCircle, Minus, PackageCheck, Plus, Puzzle, RefreshCw, RotateCcw, Save, Search, SlidersHorizontal, Square, TerminalSquare, Trash2, Wrench, X } });
const $ = <T extends HTMLElement = HTMLInputElement>(selector: string): T => document.querySelector<T>(selector)!;
const currentWindow = getCurrentWindow();
let config: LauncherConfig;
let status: LauncherStatus = { phase: "stopped", message: "dsh 尚未启动", url: "http://127.0.0.1:3080", pid: null, logs: [] };
let configFiles: ConfigFileInfo[] = [];
let toastTimer: number | undefined;
let frameUrl = "";
let market: MarketCatalog | null = null;
let marketCategories = new Map<string, MarketMeta>();
let marketTypes = new Map<string, MarketMeta>();
let marketShown = 0;
let marketSearchTimer: number | undefined;
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
}
function closeDialogs(): void { document.querySelectorAll<HTMLElement>(".modal").forEach((modal) => { modal.hidden = true; }); }

function fillForm(value: LauncherConfig): void {
  $<HTMLInputElement>(`input[name="launch_mode"][value="${value.launch_mode}"]`).checked = true;
  $("#executable").value = value.executable;
  $("#npx-package").value = value.npx_package;
  $("#working-directory").value = value.working_directory;
  $("#dsh-home").value = value.dsh_home;
  $("#port").value = String(value.port);
  updateModeFields();
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

function renderStatus(next: LauncherStatus): void {
  const wasReady = status.phase === "ready";
  status = next;
  $("#service-state").textContent = next.phase === "ready" ? "运行中" : next.phase === "starting" ? "启动中" : next.phase === "failed" ? "启动失败" : "已停止";
  const frame = $("#dsh-frame") as HTMLIFrameElement;
  const state = $("#workspace-state");
  frame.hidden = next.phase !== "ready";
  state.hidden = next.phase === "ready";
  $("#workspace-title").textContent = next.phase === "failed" ? "dsh 启动失败" : next.phase === "starting" ? "正在启动 dsh" : "dsh 尚未运行";
  $("#workspace-message").textContent = next.message;
  $("#workspace-start").toggleAttribute("disabled", next.phase === "starting" || next.phase === "stopping");
  $("#manage-restart").toggleAttribute("disabled", next.phase === "starting" || next.phase === "stopping");
  $("#manage-stop").toggleAttribute("disabled", next.phase === "stopped" || next.phase === "stopping");
  // 只在进入 ready 或地址变化时装载 iframe。iframe.src 的 getter 会把地址
  // 规范化（补上尾部斜杠），直接与 next.url 比较永远不相等，每条状态事件
  // （包括日志推送）都会触发一次 WebUI 重载。
  if (next.phase === "ready" && (!wasReady || frameUrl !== next.url)) {
    frameUrl = next.url;
    frame.src = next.url;
  }
}
function refreshWeb(): void {
  const frame = $("#dsh-frame") as HTMLIFrameElement;
  if (status.phase !== "ready" || !frameUrl) { toast("dsh Web 服务尚未就绪", true); return; }
  // 跨域 iframe 访问 contentWindow.location 会抛 SecurityError，重新赋值 src 触发刷新。
  frame.src = frameUrl;
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
    toast(result.success ? "插件安装完成，服务已重新启动" : "插件安装失败", !result.success);
    if (result.success) { $("#plugin-spec").value = ""; await loadInstalledPlugins(); }
  } catch (error) { output.textContent = String(error); toast(String(error), true); }
}
async function removePlugin(spec: string): Promise<void> {
  const output = $("#plugin-output"); output.hidden = false; output.textContent = `正在卸载 ${spec}…`;
  try {
    const result = await invoke<OperationResult>("remove_plugin", { spec });
    output.textContent = result.output || (result.success ? "卸载完成" : "卸载失败");
    toast(result.success ? "插件已卸载，服务已重新启动" : "插件卸载失败", !result.success);
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

// —— 插件商店（数据来自 dsh.aitreez.com，后端抓取并缓存）——
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
  if (marketFilter.sort === "stars") items = [...items].sort((a, b) => b.stars - a.stars);
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
    ...market.types.filter((item) => typeCounts.has(item.id))
      .map((item) => `<option value="${escapeAttr(item.id)}">${escapeHtml(item.label)} (${typeCounts.get(item.id)})</option>`),
  ].join("");
  if (marketFilter.type !== "all" && !typeCounts.has(marketFilter.type)) marketFilter.type = "all";
  typeSelect.value = marketFilter.type;
  const categorySelect = $<HTMLSelectElement>("#market-category");
  categorySelect.innerHTML = [
    `<option value="all">全部分类</option>`,
    ...market.categories.filter((item) => categoryCounts.has(item.id))
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
    marketCategories = new Map(market.categories.map((item) => [item.id, item]));
    marketTypes = new Map(market.types.map((item) => [item.id, item]));
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
$("#refresh-web").addEventListener("click", refreshWeb);
$("#new-window").addEventListener("click", () => void invoke("new_launcher_window").catch((error) => toast(String(error), true)));
$("#window-minimize").addEventListener("click", () => void currentWindow.minimize());
$("#window-maximize").addEventListener("click", () => void currentWindow.toggleMaximize());
$("#window-close").addEventListener("click", () => void currentWindow.close());
// 只有主窗口关闭时驻留托盘，新建窗口是真正关闭。
if (currentWindow.label !== "control") $("#window-close").title = "关闭窗口";
document.addEventListener("keydown", (event) => { if (event.key === "Escape") closeDialogs(); });

async function init(): Promise<void> {
  await listen<LauncherStatus>("launcher-status", (event) => renderStatus(event.payload));
  const [loadedConfig, loadedStatus] = await Promise.all([invoke<LauncherConfig>("load_config"), invoke<LauncherStatus>("get_status")]);
  config = loadedConfig;
  fillForm(config);
  renderStatus(loadedStatus);
  if (config.auto_start && loadedStatus.phase === "stopped") await runAction("start_dsh");
}
void init().catch((error) => toast(String(error), true));
