//! Windows release builds use the GUI subsystem (no extra console window).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    code_plus_plus_desktop::start();
}
