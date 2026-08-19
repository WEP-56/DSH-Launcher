// DSH Launcher 后端按职责拆成子模块，lib.rs 只负责声明模块和装配 Tauri 应用：
// - config      Launcher 自身配置的读写校验
// - state       共享运行时状态（服务状态、市场缓存、互斥锁）
// - exec        子进程执行辅助（命令解析、超时执行、进程树终止）
// - service     dsh web 服务的启动/停止/守护
// - plugins     插件的列出/搜索/安装/卸载/更新
// - market      插件商店目录 API
// - updates     dsh 包更新与 Launcher 自身的版本检查
// - dsh_files   DSH_HOME 下配置文件的读写
// - download    WebView 下载请求处理
// - windows_ui  多窗口与标签拖拽
// - tray        托盘菜单
// - util        零散通用工具
mod config;
mod download;
mod dsh_files;
mod exec;
mod market;
mod plugins;
mod service;
mod state;
mod tray;
mod updates;
mod util;
mod windows_ui;

use std::{thread, time::Duration};
use tauri::Manager;

use config::{read_config, CloseBehavior};
use service::stop_process;
use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            config::load_config,
            config::default_config,
            config::save_config,
            config::save_window_size,
            state::get_status,
            service::start_dsh,
            service::stop_dsh,
            service::restart_dsh,
            state::clear_logs,
            windows_ui::open_workspace,
            dsh_files::list_config_files,
            dsh_files::save_dsh_config,
            dsh_files::open_dsh_config,
            updates::get_package_info,
            updates::update_dsh,
            updates::get_launcher_version,
            updates::check_launcher_update,
            plugins::list_plugins,
            plugins::search_plugins,
            plugins::install_plugin,
            plugins::remove_plugin,
            plugins::update_plugins,
            plugins::check_plugin_updates,
            market::fetch_market,
            util::open_external,
            windows_ui::new_launcher_window,
            windows_ui::take_initial_tab,
            windows_ui::show_tab_drag_preview,
            windows_ui::move_tab_drag_preview,
            windows_ui::hide_tab_drag_preview,
            windows_ui::drop_tab
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            exec::adopt_login_shell_path();
            tray::setup_tray(app)?;
            // 主窗口也通过 builder 创建，这样它和分离出来的窗口都能挂载下载回调。
            windows_ui::spawn_launcher_window_named(
                &app.handle(),
                app.state::<AppState>().inner(),
                "control".into(),
                None,
                None,
                None,
            )?;
            windows_ui::setup_tab_drag_preview(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // 只有主窗口关闭时驻留托盘；新建的窗口正常销毁，
                // 否则每次“关闭”都只是隐藏，越积越多。
                if window.label() == "control" {
                    let config = read_config(&window.app_handle()).unwrap_or_default();
                    if matches!(config.close_behavior, CloseBehavior::Tray) {
                        api.prevent_close();
                        let _ = window.hide();
                    } else {
                        if config.stop_dsh_on_exit {
                            let state = window.app_handle().state::<AppState>();
                            let _ = stop_process(&window.app_handle(), state.inner());
                        }
                        // Let WebView2 finish its native window teardown before ending
                        // the tray-backed event loop. Immediate app.exit() can race
                        // Chromium's window-class cleanup (error 1412 on Windows).
                        let app = window.app_handle().clone();
                        thread::spawn(move || {
                            thread::sleep(Duration::from_millis(120));
                            app.exit(0);
                        });
                    }
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("failed to run DSH Launcher");
}
