#[cfg(target_os = "macos")]
pub(super) fn pin_subject(owner: &str, subject: &str) -> Result<(), String> {
    let service = "com.saaa.personal-state-subject";
    match security_framework::passwords::get_generic_password(service, owner) {
        Ok(saved) if saved == subject.as_bytes() => Ok(()),
        Ok(_) => Err("personal-subject-changed".into()),
        Err(e) if e.code() == -25300 => {
            security_framework::passwords::set_generic_password(service, owner, subject.as_bytes())
                .map_err(|_| "personal-subject-storage".into())
        }
        Err(_) => Err("personal-subject-storage".into()),
    }
}
#[cfg(not(target_os = "macos"))]
pub(super) fn pin_subject(_owner: &str, _subject: &str) -> Result<(), String> {
    Err("personal-protected-storage-unavailable".into())
}
