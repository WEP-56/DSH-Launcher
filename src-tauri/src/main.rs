// Prevents an extra console window behind the launcher on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    dsh_launcher_lib::run();
}
