//! Lock-free SPSC ring for VoiceProcessingIO callbacks.
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

pub const CAPACITY: usize = 131_072;

pub struct SpscF32 {
    samples: Box<[AtomicU32]>,
    mask: usize,
    write: AtomicUsize,
    read: AtomicUsize,
    epoch: AtomicU64,
    consumer_epoch: AtomicU64,
}

impl SpscF32 {
    pub fn new() -> Self {
        Self {
            samples: (0..CAPACITY)
                .map(|_| AtomicU32::new(0))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            mask: CAPACITY - 1,
            write: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
            epoch: AtomicU64::new(0),
            consumer_epoch: AtomicU64::new(0),
        }
    }

    pub fn generation(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }

    pub fn clear(&self) {
        self.epoch.fetch_add(1, Ordering::AcqRel);
        self.write.store(0, Ordering::Release);
    }

    pub fn len(&self) -> usize {
        let write = self.write.load(Ordering::Acquire);
        if self.consumer_epoch.load(Ordering::Acquire) != self.epoch.load(Ordering::Acquire) {
            return write.min(CAPACITY - 1);
        }
        let read = self.read.load(Ordering::Acquire);
        let used = write.wrapping_sub(read);
        if used >= CAPACITY {
            0
        } else {
            used
        }
    }

    pub fn write(&self, src: &[f32]) -> usize {
        if src.is_empty() {
            return 0;
        }
        let write = self.write.load(Ordering::Relaxed);
        let read =
            if self.consumer_epoch.load(Ordering::Acquire) == self.epoch.load(Ordering::Acquire) {
                self.read.load(Ordering::Acquire)
            } else {
                0
            };
        let used = write.wrapping_sub(read);
        if used >= CAPACITY {
            return 0;
        }
        let space = CAPACITY.saturating_sub(used).saturating_sub(1);
        let n = src.len().min(space);
        for (index, sample) in src.iter().take(n).enumerate() {
            self.samples[(write.wrapping_add(index)) & self.mask]
                .store(sample.to_bits(), Ordering::Relaxed);
        }
        self.write.store(write.wrapping_add(n), Ordering::Release);
        n
    }

    pub fn read(&self, dst: &mut [f32]) -> usize {
        if dst.is_empty() {
            return 0;
        }
        let epoch = self.epoch.load(Ordering::Acquire);
        let mut read = self.read.load(Ordering::Relaxed);
        if self.consumer_epoch.load(Ordering::Relaxed) != epoch {
            self.consumer_epoch.store(epoch, Ordering::Release);
            read = 0;
            self.read.store(0, Ordering::Release);
        }
        let write = self.write.load(Ordering::Acquire);
        let available = write.wrapping_sub(read);
        if available == 0 || available >= CAPACITY {
            return 0;
        }
        let n = dst.len().min(available);
        for (index, slot) in dst.iter_mut().take(n).enumerate() {
            *slot = f32::from_bits(
                self.samples[(read.wrapping_add(index)) & self.mask].load(Ordering::Relaxed),
            );
        }
        if self.epoch.load(Ordering::Acquire) != epoch {
            return 0;
        }
        self.read.store(read.wrapping_add(n), Ordering::Release);
        n
    }
}

#[no_mangle]
pub unsafe extern "C" fn saaa_spsc_write_f32(
    ring: *mut SpscF32,
    src: *const f32,
    count: u32,
) -> u32 {
    if ring.is_null() || src.is_null() || count == 0 {
        return 0;
    }
    let samples = std::slice::from_raw_parts(src, count as usize);
    (*ring).write(samples) as u32
}

#[no_mangle]
pub unsafe extern "C" fn saaa_spsc_read_f32(ring: *mut SpscF32, dst: *mut f32, count: u32) -> u32 {
    if ring.is_null() || dst.is_null() || count == 0 {
        return 0;
    }
    let samples = std::slice::from_raw_parts_mut(dst, count as usize);
    (*ring).read(samples) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_reads_without_overlap() {
        let ring = SpscF32::new();
        assert_eq!(ring.write(&[1.0, 2.0, 3.0]), 3);
        let mut out = [0.0; 4];
        assert_eq!(ring.read(&mut out), 3);
        assert_eq!(&out[..3], &[1.0, 2.0, 3.0]);
        assert_eq!(ring.read(&mut out), 0);
    }

    #[test]
    fn clear_bumps_generation_and_discards() {
        let ring = SpscF32::new();
        ring.write(&[1.0, 2.0]);
        let before = ring.generation();
        ring.clear();
        assert!(ring.generation() > before);
        assert_eq!(ring.len(), 0);
        let mut out = [0.0; 2];
        assert_eq!(ring.read(&mut out), 0);
        assert_eq!(ring.write(&[9.0]), 1);
        assert_eq!(ring.read(&mut out), 1);
        assert_eq!(out[0], 9.0);
    }

    #[test]
    fn never_fills_the_last_slot() {
        let ring = SpscF32::new();
        let block = vec![0.5; CAPACITY];
        assert_eq!(ring.write(&block), CAPACITY - 1);
        assert_eq!(ring.write(&[1.0]), 0);
    }
}
