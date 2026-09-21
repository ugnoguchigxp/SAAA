//! Test-only fault injection. Production builds compile this as a no-op.
#![allow(dead_code)]

#[cfg(test)]
thread_local! {
    static POINT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(crate) fn set(point: Option<&'static str>) {
    POINT.with(|cell| cell.set(point));
}

pub(crate) fn maybe(point: &'static str) -> Result<(), String> {
    #[cfg(test)]
    {
        if POINT.with(|cell| cell.get()) == Some(point) {
            return Err(format!("injected_fault:{point}"));
        }
    }
    let _ = point;
    Ok(())
}
