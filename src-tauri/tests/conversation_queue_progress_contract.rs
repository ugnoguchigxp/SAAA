fn database_error(error: rusqlite::Error) -> String {
    error.to_string()
}

#[path = "../src/runtime/conversation_check/queue_progress.rs"]
mod queue_progress;
#[path = "../src/task_queue.rs"]
mod task_queue;
