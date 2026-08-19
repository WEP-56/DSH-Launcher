use std::{thread, time::Duration};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Manager,
};

use crate::{
    config::read_config,
    service::{start_with_feedback, stop_process},
    state::AppState,
    windows_ui::{open_workspace, show_control},
};

pub fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "打开 DSH", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "启动设置", true, None::<&str>)?;
    let restart = MenuItem::with_id(app, "restart", "重启服务", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &settings, &restart, &quit])?;
    let mut tray = TrayIconBuilder::new().menu(&menu).tooltip("DSH Launcher");
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.on_menu_event(|app, event| match event.id().as_ref() {
        "open" => {
            let state = app.state::<AppState>();
            if open_workspace(app.clone(), state).is_err() {
                show_control(app);
            }
        }
        "settings" => show_control(app),
        "restart" => {
            let app_handle = app.clone();
            thread::spawn(move || {
                let state = app_handle.state::<AppState>().inner().clone();
                // 防呆：插件/更新操作进行中忽略托盘重启，操作结束会自动恢复服务
                //（界面此时也显示“维护中”并禁用启动入口）。
                let Ok(_ops) = state.ops.try_lock() else {
                    return;
                };
                let _ = stop_process(&app_handle, &state);
                let _ = start_with_feedback(&app_handle, &state);
            });
        }
        "quit" => {
            let config = read_config(app).unwrap_or_default();
            if config.stop_dsh_on_exit {
                let state = app.state::<AppState>();
                let _ = stop_process(app, state.inner());
            }
            let app = app.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(120));
                app.exit(0);
            });
        }
        _ => {}
    })
    .build(app)?;
    Ok(())
}
