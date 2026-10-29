// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    flowcapture_lib::updater::run_startup_hooks();
    flowcapture_lib::run()
}
