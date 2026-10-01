fn database_error(error: rusqlite::Error) -> String {
    error.to_string()
}
#[path = "../src/memory/personal_state/maintenance.rs"]
mod maintenance;
