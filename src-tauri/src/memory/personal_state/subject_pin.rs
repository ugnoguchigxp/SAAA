pub(super) fn pin_subject(owner: &str, subject: &str) -> Result<(), String> {
    let service = "com.saaa.personal-state-subject";
    match crate::credentials::load_named_secret(service, owner)
        .map_err(|_| "personal-subject-storage")?
    {
        Some(saved) if saved.as_bytes() == subject.as_bytes() => Ok(()),
        Some(_) => Err("personal-subject-changed".into()),
        None => crate::credentials::store_named_secret(service, owner, subject.as_bytes())
            .map_err(|_| "personal-subject-storage".into()),
    }
}
