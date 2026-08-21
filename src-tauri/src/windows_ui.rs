use serde::Serialize;
use std::{
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::Duration,
};
#[cfg(target_os = "macos")]
use tauri::TitleBarStyle;
use tauri::{
    webview::DownloadEvent, AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder,
};
#[cfg(target_os = "macos")]
use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState};

use crate::{
    download::handle_download_request,
    state::{AppState, Phase},
};

// 标题栏的逻辑高度（px），前端 .titlebar 与这里必须一致；拖拽落点命中
// 其他窗口的这段区域时视为“拖入标签栏”。macOS 用原生 Overlay 标题栏、
// 前端 .titlebar 保持 35px（与 Windows/Linux 一致，Overlay 由红绿灯所在
// 系统栏提供，前端无需加高），各平台统一为 35px，否则 43~48px 的拖放会
// 误判为“拖出成新窗口”。
const TITLEBAR_LOGICAL_HEIGHT: f64 = 35.0;
const TAB_DRAG_PREVIEW_LABEL: &str = "tab-drag-preview";

fn next_window_label() -> String {
    static WINDOW_COUNTER: AtomicU64 = AtomicU64::new(0);
    format!(
        "control-{}",
        WINDOW_COUNTER.fetch_add(1, Ordering::Relaxed) + 1
    )
}

fn reveal_window_fallback(window: tauri::WebviewWindow) {
    // 窗口以隐藏状态创建，由前端首帧就绪后 show()，避免启动白屏。这里兜底：
    // 前端脚本万一没跑起来，也要在几秒内把窗口亮出来，不能凭空消失。
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(3));
        if !window.is_visible().unwrap_or(true) {
            let _ = window.show();
        }
    });
}

pub fn spawn_launcher_window_named(
    app: &AppHandle,
    state: &AppState,
    label: String,
    initial_tab: Option<String>,
    position: Option<(f64, f64)>,
    size: Option<(f64, f64)>,
) -> Result<(), String> {
    if let Some(title) = initial_tab {
        if let Ok(mut pending) = state.pending_tabs.lock() {
            pending.insert(label.clone(), title);
        }
    }
    // Use the app URL so Tauri resolves the dev server in development and
    // the bundled frontend in production. External URLs do not reliably get
    // the launcher initialization scripts and window APIs.
    let (width, height) = size.unwrap_or((880.0, 760.0));
    let mut builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("index.html".into()))
        .title("DSH Launcher")
        .inner_size(width, height)
        .min_inner_size(620.0, 560.0)
        .resizable(true)
        .visible(false);
    // macOS：启用原生装饰 + 叠加标题栏，露出标准红黄绿按钮（左侧）。
    // transparent(true) 配合 macos-private-api，让标题栏下方的原生振动模糊
    // （NSVisualEffectView）能够透出；前端把标题栏设为半透明以呈现毛玻璃。
    // Windows/Linux：保持无边框自定义标题栏（与原始行为完全一致）。
    #[cfg(target_os = "macos")]
    {
        builder = builder
            .decorations(true)
            .title_bar_style(TitleBarStyle::Overlay)
            .transparent(true)
            // Overlay 模式下系统会在红绿灯右侧绘制原生标题文字；清空标题
            // 只保留红绿灯，避免“DSH Launcher”文字与前端居中的标签区在
            // 小窗口下交叠。窗口标题由 Dock/App 菜单提供，无需在此显示。
            .title("");
    }
    #[cfg(not(target_os = "macos"))]
    {
        builder = builder.decorations(false);
    }
    builder = match position {
        Some((x, y)) => builder.position(x, y),
        None => builder.center(),
    };
    let download_app = app.clone();
    let window = builder
        .on_download(move |_webview, event| match event {
            DownloadEvent::Requested { url, destination } => {
                handle_download_request(&download_app, &url, destination)
            }
            DownloadEvent::Finished { url, path, success } => {
                if success {
                    if let Some(path) = path {
                        eprintln!("下载完成 {url}：{}", path.display());
                    }
                } else {
                    eprintln!("下载失败 {url}");
                }
                true
            }
            _ => true,
        })
        .build()
        .map_err(|error| format!("无法创建 Launcher 窗口：{error}"))?;
    // macOS：安装原生振动模糊（HeaderView 材质），标题栏呈系统毛玻璃质感。
    // 失败不影响功能，仅降级为纯色标题栏。
    #[cfg(target_os = "macos")]
    {
        let _ = apply_vibrancy(
            &window,
            NSVisualEffectMaterial::HeaderView,
            Some(NSVisualEffectState::Active),
            None,
        );
    }
    reveal_window_fallback(window);
    Ok(())
}

fn spawn_launcher_window(
    app: &AppHandle,
    state: &AppState,
    initial_tab: Option<String>,
    position: Option<(f64, f64)>,
    size: Option<(f64, f64)>,
) -> Result<(), String> {
    spawn_launcher_window_named(app, state, next_window_label(), initial_tab, position, size)
}

pub fn setup_tab_drag_preview(app: &tauri::App) -> tauri::Result<()> {
    let preview = WebviewWindowBuilder::new(
        app,
        TAB_DRAG_PREVIEW_LABEL,
        WebviewUrl::App("drag-preview.html".into()),
    )
    .title("标签拖拽预览")
    .inner_size(190.0, 30.0)
    .resizable(false)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(false)
    .visible(false);
    // 预览窗不需要毛玻璃，只要背景透明。macOS 上 `.transparent(true)` 依赖
    // macos-private-api，而主窗口已经用它做振动模糊；这里保持仅非 macOS 启用，
    // macOS 下退化为不透明小窗，不影响拖拽落点判定。
    #[cfg(not(target_os = "macos"))]
    let preview = preview.transparent(true);
    let preview = preview.build()?;
    preview.set_ignore_cursor_events(true)?;
    Ok(())
}

fn position_tab_drag_preview(app: &AppHandle) -> Result<tauri::WebviewWindow, String> {
    let preview = app
        .get_webview_window(TAB_DRAG_PREVIEW_LABEL)
        .ok_or("标签拖拽预览窗口不可用")?;
    let cursor = app
        .cursor_position()
        .map_err(|error| format!("无法获取光标位置：{error}"))?;
    preview
        .set_position(tauri::PhysicalPosition::new(
            (cursor.x + 14.0).round() as i32,
            (cursor.y + 10.0).round() as i32,
        ))
        .map_err(|error| error.to_string())?;
    Ok(preview)
}

#[tauri::command]
pub fn show_tab_drag_preview(app: AppHandle, title: String) -> Result<(), String> {
    let preview = position_tab_drag_preview(&app)?;
    let title = serde_json::to_string(&title).map_err(|error| error.to_string())?;
    preview
        .eval(&format!("window.setTabTitle?.({title})"))
        .map_err(|error| error.to_string())?;
    preview.show().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn move_tab_drag_preview(app: AppHandle) -> Result<(), String> {
    position_tab_drag_preview(&app).map(|_| ())
}

#[tauri::command]
pub fn hide_tab_drag_preview(app: AppHandle) -> Result<(), String> {
    if let Some(preview) = app.get_webview_window(TAB_DRAG_PREVIEW_LABEL) {
        preview.hide().map_err(|error| error.to_string())?;
    }
    Ok(())
}

// 必须是 async：同步命令在主线程执行，而在主线程上同步创建 WebView 会在
// Windows 上死锁（wry#583）——新窗口停在白屏，且事件循环被卡住，所有窗口的
// 拖拽/最小化/关闭全部失效。
#[tauri::command]
pub async fn new_launcher_window(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    spawn_launcher_window(&app, state.inner(), None, None, None)
}

#[tauri::command]
pub fn take_initial_tab(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
) -> Option<String> {
    state
        .pending_tabs
        .lock()
        .ok()
        .and_then(|mut pending| pending.remove(window.label()))
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TabDropOutcome {
    Adopted,
    Detached,
    None,
}

#[derive(Debug, Clone, Serialize)]
struct AdoptedTab {
    title: String,
}

// 标签被拖出窗口后松手：落点在其他 Launcher 窗口的标签栏上就让它收编，
// 否则在光标处新建一个窗口带走这个标签。必须 async（见 new_launcher_window）。
#[tauri::command]
pub async fn drop_tab(
    app: AppHandle,
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    title: String,
    remaining: u32,
) -> Result<TabDropOutcome, String> {
    let cursor = app
        .cursor_position()
        .map_err(|error| format!("无法获取光标位置：{error}"))?;
    for (label, target) in app.webview_windows() {
        if label == window.label() || !label.starts_with("control") {
            continue;
        }
        if target.is_minimized().unwrap_or(false) || !target.is_visible().unwrap_or(false) {
            continue;
        }
        let (Ok(position), Ok(size)) = (target.outer_position(), target.outer_size()) else {
            continue;
        };
        let scale = target.scale_factor().unwrap_or(1.0);
        // 比标题栏多放宽几个像素，拖拽不必像素级精确。
        let strip_bottom = position.y as f64 + (TITLEBAR_LOGICAL_HEIGHT + 8.0) * scale;
        let inside_x =
            cursor.x >= position.x as f64 && cursor.x <= position.x as f64 + size.width as f64;
        let inside_y = cursor.y >= position.y as f64 && cursor.y <= strip_bottom;
        if inside_x && inside_y {
            let _ = target.set_focus();
            app.emit_to(label.as_str(), "adopt-tab", AdoptedTab { title })
                .map_err(|error| error.to_string())?;
            // 被拆出的窗口只剩这一枚标签时，并入目标窗口后应立即销毁源窗口。
            // 稍作延迟，让命令结果先回到前端；主窗口不能关闭，否则会触发托盘退出逻辑。
            if window.label() != "control" && remaining <= 1 {
                let source = window.clone();
                thread::spawn(move || {
                    thread::sleep(Duration::from_millis(80));
                    let _ = source.close();
                });
            }
            return Ok(TabDropOutcome::Adopted);
        }
    }
    if remaining <= 1 {
        // 窗口里唯一的标签拖到空白处没有意义（新窗口=原窗口），只支持并入其他窗口。
        return Ok(TabDropOutcome::None);
    }
    let monitor_scale = app
        .monitor_from_point(cursor.x, cursor.y)
        .ok()
        .flatten()
        .map(|monitor| monitor.scale_factor())
        .unwrap_or(1.0);
    // 新窗口的标签栏正好落在光标下方一点，观感接近浏览器的拖出。
    let position = (
        cursor.x / monitor_scale - 120.0,
        cursor.y / monitor_scale - 16.0,
    );
    // 拖出的窗口沿用来源窗口的尺寸。
    let size = window.inner_size().ok().map(|inner| {
        let scale = window.scale_factor().unwrap_or(1.0);
        (
            (inner.width as f64 / scale).max(620.0),
            (inner.height as f64 / scale).max(560.0),
        )
    });
    spawn_launcher_window(&app, state.inner(), Some(title), Some(position), size)?;
    Ok(TabDropOutcome::Detached)
}

#[tauri::command]
pub fn open_workspace(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    {
        let runtime = state.runtime.lock().map_err(|_| "启动器状态锁已损坏")?;
        if runtime.phase != Phase::Ready {
            return Err("dsh Web 服务尚未就绪".into());
        }
    }
    if let Some(control) = app.get_webview_window("control") {
        control.show().map_err(|error| error.to_string())?;
        control.set_focus().map_err(|error| error.to_string())?;
    }
    let _ = app.emit("show-workspace", ());
    Ok(())
}

pub fn show_control(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("control") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}
