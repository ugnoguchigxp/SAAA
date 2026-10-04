use super::*;
impl SqliteReaders {
    #[cfg(test)]
    pub(crate) fn lane_count(&self) -> usize {
        match self.source {
            ReaderSource::Persistent(_) => READER_LANES,
            #[cfg(any(
                test,
                feature = "quality-eval-harness",
                feature = "conversation-queue-e2e"
            ))]
            ReaderSource::Serialized(_) => 1,
        }
    }

    #[cfg(test)]
    pub(crate) fn cached_settings_revision(&self) -> Option<i64> {
        match &self.source {
            ReaderSource::Persistent(readers) => readers
                .settings_snapshot
                .lock()
                .ok()
                .and_then(|cache| cache.as_ref().map(|cached| cached.revision)),
            ReaderSource::Serialized(_) => None,
        }
    }
}
