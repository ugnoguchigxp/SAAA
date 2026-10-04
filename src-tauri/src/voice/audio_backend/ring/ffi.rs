use super::*;

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
