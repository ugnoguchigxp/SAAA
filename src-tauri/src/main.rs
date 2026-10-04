// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if let Some(result) = saaa_lib::terminal_agent_helper(&args) {
        if let Err(error) = result {
            eprintln!("{error}");
            std::process::exit(2);
        }
        return;
    }
    saaa_lib::run()
}
