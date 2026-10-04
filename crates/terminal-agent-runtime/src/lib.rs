//! The desktop binary dispatches here before starting Tauri. No database or GUI is opened.
mod adapters;
mod bridge;
mod process;
mod spool;
mod viewer;
pub use adapters::{cli_command, command, configuration, resolve_executable};
pub use process::process_identity;
pub use spool::{append, create, read, write_private, Event, Spec};
use std::path::Path;

pub fn helper(args: &[String]) -> Option<Result<(), String>> {
    if args.first().map(String::as_str) != Some("--saaa-terminal-agent") {
        return None;
    }
    Some((|| {
        let mode = args.get(1).ok_or("helper_mode_missing")?;
        let directory = Path::new(args.get(2).ok_or("helper_directory_missing")?);
        let spec = read(directory)?;
        match mode.as_str() {
            "run" => process::run(directory, &spec),
            "mcp" => bridge::serve(directory, &spec),
            "hook" => bridge::hook(directory, &spec),
            "view" => viewer::view(directory, &spec),
            _ => Err("helper_mode_invalid".into()),
        }
    })())
}
#[cfg(test)]
mod tests;
