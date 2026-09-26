use rusqlite::{backup::Backup, Connection, OpenFlags};
use std::{fs, path::PathBuf, time::Duration};

pub fn open_isolated_database() -> Result<(Connection, PathBuf), String> {
    let directory = std::env::var_os("SAAA_MVP2X_APP_DATA_DIR")
        .map(PathBuf::from)
        .ok_or("SAAA_MVP2X_APP_DATA_DIR を指定してください。")?;
    let source = std::env::var_os("SAAA_PREVIEW_SOURCE_DB")
        .map(PathBuf::from)
        .ok_or("SAAA_PREVIEW_SOURCE_DB を指定してください。")?;
    open_isolated_database_at(directory, source)
}

pub fn source_config_fingerprint() -> Result<String, String> {
    let source = std::env::var_os("SAAA_PREVIEW_SOURCE_DB")
        .map(PathBuf::from)
        .ok_or("SAAA_PREVIEW_SOURCE_DB を指定してください。")?;
    let db = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| "コピー元の設定を再確認できません。")?;
    crate::config::load(&db).map(|config| config.fingerprint)
}

fn open_isolated_database_at(
    directory: PathBuf,
    source: PathBuf,
) -> Result<(Connection, PathBuf), String> {
    if !directory.is_absolute() || !source.is_absolute() {
        return Err("隔離ディレクトリとコピー元DBは絶対pathで指定してください。".into());
    }
    let metadata = fs::symlink_metadata(&directory)
        .map_err(|_| "隔離ディレクトリが存在しません。".to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("隔離ディレクトリは実ディレクトリにしてください。".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o777 != 0o700 {
            return Err("隔離ディレクトリの権限は0700にしてください。".into());
        }
    }
    let directory = fs::canonicalize(directory)
        .map_err(|_| "隔離ディレクトリを解決できません。".to_string())?;
    let source =
        fs::canonicalize(source).map_err(|_| "コピー元DBを解決できません。".to_string())?;
    if !source.is_file() || source.parent() == Some(directory.as_path()) {
        return Err("コピー元DBと隔離先を分けてください。".into());
    }
    let target = directory.join("saaa.sqlite3");
    if target == source {
        return Err("本番DBをpreview先に指定できません。".into());
    }
    if target.exists() {
        let target_meta =
            fs::symlink_metadata(&target).map_err(|_| "隔離DBを確認できません。".to_string())?;
        if !target_meta.is_file() || target_meta.file_type().is_symlink() {
            return Err("隔離DBは実ファイルにしてください。".into());
        }
        if fs::canonicalize(&target).ok().as_deref() == Some(source.as_path()) {
            return Err("本番DBをpreview先に指定できません。".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let source_meta =
                fs::metadata(&source).map_err(|_| "コピー元DBを確認できません。".to_string())?;
            if target_meta.dev() == source_meta.dev() && target_meta.ino() == source_meta.ino() {
                return Err("本番DBのhardlinkをpreview先に指定できません。".into());
            }
        }
    }
    if !target.exists() {
        let source_db = Connection::open_with_flags(&source, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| "コピー元DBを読み取れません。".to_string())?;
        let temporary = tempfile::Builder::new()
            .prefix(".saaa-preview-")
            .suffix(".sqlite3.partial")
            .tempfile_in(&directory)
            .map_err(|_| "隔離DBの一時ファイルを作成できません。".to_string())?;
        let mut destination = Connection::open(temporary.path())
            .map_err(|_| "隔離DBを作成できません。".to_string())?;
        {
            let backup = Backup::new(&source_db, &mut destination)
                .map_err(|_| "SQLite Online Backupを開始できません。".to_string())?;
            backup
                .run_to_completion(32, Duration::from_millis(20), None)
                .map_err(|_| "SQLite Online Backupに失敗しました。".to_string())?;
        }
        drop(destination);
        temporary
            .persist_noclobber(&target)
            .map_err(|_| "隔離DBを確定できません。".to_string())?;
    }
    let db = Connection::open(&target).map_err(|_| "隔離DBを開けません。".to_string())?;
    db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")
        .map_err(|_| "隔離DBを初期化できません。".to_string())?;
    Ok((db, directory))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn online_backup_preserves_saved_configuration_and_old_lease() {
        let source_dir = tempfile::tempdir().unwrap();
        let target_dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(target_dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }
        let source_path = source_dir.path().join("saaa.sqlite3");
        let source = Connection::open(&source_path).unwrap();
        source.execute_batch(
            "CREATE TABLE settings_documents(namespace TEXT,key TEXT,value_json TEXT,PRIMARY KEY(namespace,key));
             CREATE TABLE credential_secrets(service TEXT,account TEXT,secret TEXT);
             CREATE TABLE larm_voice_lease_slot(id INTEGER PRIMARY KEY,idempotency_key TEXT,updated_at TEXT);
             CREATE TABLE conversations(id TEXT PRIMARY KEY,title TEXT,task_mode TEXT,created_at TEXT,updated_at TEXT);
             CREATE TABLE conversation_messages(id TEXT PRIMARY KEY,conversation_id TEXT,role TEXT,content TEXT,created_at TEXT);
             INSERT INTO larm_voice_lease_slot VALUES(1,'old-production-key','before');
             INSERT INTO settings_documents VALUES('providers.model','default',
               '{\"harness\":{\"address\":\"http://127.0.0.1:9810\",\"larmProfile\":\"saaa-conversation-ornith15\"},\"providers\":[{\"id\":\"system-tts\",\"kind\":\"system-tts\",\"enabled\":true,\"voice\":\"default\"}]}');
             INSERT INTO settings_documents VALUES('routing.tasks','default',
               '{\"voiceSpeak\":{\"source\":\"provider\",\"providerId\":\"system-tts\"}}');
             PRAGMA user_version=41;",
        ).unwrap();
        drop(source);
        let (mut isolated, _) =
            open_isolated_database_at(target_dir.path().to_path_buf(), source_path.clone())
                .unwrap();
        let config = crate::config::load(&isolated).unwrap();
        assert_eq!(config.profile_label, "saaa-conversation-ornith15");
        crate::repository::migrate(&isolated).unwrap();
        crate::repository::open_session(
            &mut isolated,
            "preview-1",
            "conversation-1",
            &config.fingerprint,
            "new-preview-key",
            "now",
        )
        .unwrap();
        let old_key: String = isolated
            .query_row(
                "SELECT idempotency_key FROM larm_voice_lease_slot WHERE id=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(old_key, "old-production-key");
        let original =
            Connection::open_with_flags(source_path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let preview_count: i64 = original
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name='conversation_preview_sessions'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(preview_count, 0);
    }

    #[test]
    fn isolation_rejects_non_private_and_same_source_directory() {
        let source_dir = tempfile::tempdir().unwrap();
        let path = source_dir.path().join("saaa.sqlite3");
        Connection::open(&path).unwrap();
        assert!(open_isolated_database_at(source_dir.path().to_path_buf(), path.clone()).is_err());
        let other = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(other.path(), fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert!(open_isolated_database_at(other.path().to_path_buf(), path).is_err());
    }
}
