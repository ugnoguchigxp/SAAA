use std::{fs, path::PathBuf};

/// Reads the existing LARM credential sources without changing either one.
pub fn load_larm_token() -> Result<String, String> {
    let from_env = std::env::var("LARM_API_TOKEN")
        .ok()
        .map(|value| validate(&value).map(|_| value))
        .transpose()?;
    let from_file = existing_file_token()?;
    match (from_env, from_file) {
        (Some(env), Some(file)) if env != file => Err("LARMの資格情報源が一致しません。".into()),
        (Some(token), _) | (None, Some(token)) => Ok(token),
        (None, None) => Err("LARMの資格情報がありません。".into()),
    }
}

fn existing_file_token() -> Result<Option<String>, String> {
    let home = std::env::var_os("HOME").ok_or("HOMEを確認できません。")?;
    let path = PathBuf::from(home).join(".ssh/larm");
    if !path.exists() {
        return Ok(None);
    }
    let metadata = fs::symlink_metadata(&path)
        .map_err(|_| "LARM資格情報ファイルを確認できません。".to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 4112 {
        return Err("LARM資格情報ファイルを安全に読み取れません。".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err("LARM資格情報ファイルの所有者または権限を確認してください。".into());
        }
    }
    let raw = fs::read_to_string(path)
        .map_err(|_| "LARM資格情報ファイルを読み取れません。".to_string())?;
    let raw = raw.trim_end_matches('\n');
    let token = raw
        .strip_prefix("LARM_API_TOKEN=")
        .ok_or("LARM資格情報ファイルの形式が正しくありません。")?;
    validate(token)?;
    Ok(Some(token.to_string()))
}

fn validate(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 4096
        || value.trim().is_empty()
        || value.bytes().any(|b| matches!(b, 0 | b'\n' | b'\r'))
    {
        Err("LARM資格情報の形式が正しくありません。".into())
    } else {
        Ok(())
    }
}
