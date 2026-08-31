use std::{fs, path::Path};

fn rust_sources(directory: &Path, output: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(directory).expect("source directory reads") {
        let path = entry.expect("source entry reads").path();
        if path.is_dir() {
            rust_sources(&path, output);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            output.push(path);
        }
    }
}

fn production_prefix(source: &str) -> &str {
    source
        .split_once("#[cfg(test)]\nmod tests")
        .map_or(source, |(production, _)| production)
}

#[test]
fn main_database_open_and_connection_ownership_are_centralized() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = Vec::new();
    rust_sources(&source_root, &mut sources);

    for path in sources {
        if path.file_name().is_some_and(|name| name == "tests.rs") {
            continue;
        }
        let relative = path
            .strip_prefix(&source_root)
            .expect("source path is relative");
        let source = fs::read_to_string(&path).expect("source reads");
        let production = production_prefix(&source);
        let compact = production
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>();

        assert!(
            !compact.contains("Arc<Mutex<Connection>>")
                && !compact.contains("Arc<Mutex<rusqlite::Connection>>"),
            "raw shared SQLite connection found in {}",
            relative.display()
        );
        assert!(
            !compact.contains("sqlite_writer.lock("),
            "production code obtains the Writer's raw connection lock in {}",
            relative.display()
        );

        if production.contains("Connection::open(") {
            assert!(
                matches!(
                    relative.to_str(),
                    Some("persistence/sqlite/writer.rs") | Some("backup.rs")
                ),
                "unexpected read-write SQLite open in {}",
                relative.display()
            );
        }
        if production.contains("Connection::open_with_flags(") {
            assert_eq!(
                relative.to_str(),
                Some("persistence/sqlite/readers.rs"),
                "unexpected SQLite flagged open in {}",
                relative.display()
            );
        }
        if production.contains("SqliteWriter::open(") {
            assert_eq!(
                relative.to_str(),
                Some("lib.rs"),
                "unexpected Writer construction in {}",
                relative.display()
            );
        }
    }

    let writer = fs::read_to_string(source_root.join("persistence/sqlite/writer.rs"))
        .expect("writer source reads");
    assert!(writer.contains("#[cfg(test)]\n    pub(crate) fn lock"));
    let owner_acquire = writer
        .find("DatabaseOwnerGuard::acquire")
        .expect("Writer acquires database ownership");
    let database_open = writer
        .find("Connection::open(database_path)")
        .expect("Writer opens the database");
    assert!(
        owner_acquire < database_open,
        "database ownership must be acquired before opening SQLite"
    );

    let readers = fs::read_to_string(source_root.join("persistence/sqlite/readers.rs"))
        .expect("readers source reads");
    assert!(readers.contains("SQLITE_OPEN_READ_ONLY"));
    assert!(readers.contains("PRAGMA query_only=ON"));
    assert!(readers.contains("let transaction = connection.transaction()"));

    let enrollment = fs::read_to_string(source_root.join("voice/profile/enrollment.rs"))
        .expect("voice enrollment source reads");
    let prepare = enrollment
        .find("let prepared = self.prepare_sample(input)?")
        .expect("voice sample preprocessing exists");
    let writer_lock = enrollment
        .find("writer.write(|connection|")
        .expect("voice sample persistence uses Writer");
    assert!(
        prepare < writer_lock,
        "audio preprocessing must precede the Writer lock"
    );

    let turns = fs::read_to_string(source_root.join("runtime/turns.rs"))
        .expect("conversation runtime source reads");
    let execute_turn = turns
        .split_once("pub(crate) async fn execute_conversation_turn")
        .expect("conversation runtime entry point exists")
        .1;
    let reader = execute_turn
        .find("state.sqlite_readers.read")
        .expect("context source uses a persistent Reader");
    let compose = execute_turn
        .find("context_window::compose")
        .expect("context composition is explicit");
    let writer = execute_turn
        .find("state.sqlite_writer.write")
        .expect("projection telemetry uses the Writer");
    assert!(
        reader < compose && compose < writer,
        "context data must be read first, composed after the Reader transaction, then recorded"
    );

    let readiness_reader = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/larm-readiness/database.ts"),
    )
    .expect("LARM readiness database reader source reads");
    assert!(readiness_reader.contains("readonly: true"));
    assert!(readiness_reader.contains("PRAGMA query_only=ON"));
}
