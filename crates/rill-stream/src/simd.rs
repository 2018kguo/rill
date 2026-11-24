//! Finding the next byte of a small ASCII set, 16 bytes at a time.
//!
//! A byte is in the set when `LOW[byte & 15] & HIGH[byte >> 4]` is not zero: one table
//! lookup per nibble, both done for 16 bytes by a single shuffle each (wasm's
//! `i8x16.swizzle`, NEON's `tbl`). Every high nibble gets its own bit, so the test is
//! exact, and bytes from 0x80 up (UTF-8) never match.

/// The first `unit` (ASCII) in `text`: memchr over the bytes, keeping a match that is
/// a unit's low byte with a zero high byte.
pub fn find_unit(text: &[u16], unit: u16) -> Option<usize> {
    debug_assert!(unit < 0x80);
    // SAFETY: any u16 slice is readable as twice as many bytes.
    let bytes = unsafe { core::slice::from_raw_parts(text.as_ptr().cast::<u8>(), text.len() * 2) };
    let mut at = 0;
    while let Some(offset) = memchr::memchr(unit as u8, &bytes[at..]) {
        let index = at + offset;
        let (low, high) = if cfg!(target_endian = "little") {
            (0, 1)
        } else {
            (1, 0)
        };
        if index % 2 == low && bytes[index - low + high] == 0 {
            return Some(index / 2);
        }
        at = index + 1;
    }
    None
}

/// Nibble tables for a set of ASCII bytes.
#[derive(Clone, Copy)]
pub struct ByteSet {
    low: [u8; 16],
    high: [u8; 16],
}

impl ByteSet {
    pub const fn new(bytes: &[u8]) -> Self {
        let mut low = [0u8; 16];
        let mut high = [0u8; 16];
        let mut index = 0;
        while index < bytes.len() {
            let byte = bytes[index];
            assert!(byte < 0x80, "ASCII only");
            let bit = 1u8 << (byte >> 4);
            high[(byte >> 4) as usize] = bit;
            low[(byte & 15) as usize] |= bit;
            index += 1;
        }
        Self { low, high }
    }

    #[inline(always)]
    pub fn contains(&self, byte: u8) -> bool {
        self.low[usize::from(byte & 15)] & self.high[usize::from(byte >> 4)] != 0
    }

    /// Bit `i` set when `chunk[i]` is in the set (a unit that is not ASCII is in none).
    #[inline(always)]
    pub fn mask16_units(&self, chunk: &[u16; 16]) -> u32 {
        imp::mask16(self, &imp::narrow(chunk))
    }
}

#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
mod imp {
    use core::arch::wasm32::*;

    /// Units from 0x80 up become 0x80 (signed saturation sends 0x8000 and up to 0, so
    /// clamp first).
    #[inline(always)]
    pub fn narrow(chunk: &[u16; 16]) -> [u8; 16] {
        // SAFETY: unaligned loads of 32 bytes the caller owns.
        unsafe {
            let low = u16x8_min(v128_load(chunk.as_ptr().cast()), u16x8_splat(0x80));
            let high = u16x8_min(v128_load(chunk.as_ptr().add(8).cast()), u16x8_splat(0x80));
            let mut out = [0u8; 16];
            v128_store(out.as_mut_ptr().cast(), u8x16_narrow_i16x8(low, high));
            out
        }
    }

    #[inline(always)]
    pub fn mask16(set: &super::ByteSet, chunk: &[u8; 16]) -> u32 {
        // SAFETY: 16-byte unaligned loads of arrays the caller owns.
        unsafe {
            let bytes = v128_load(chunk.as_ptr().cast());
            let low = v128_load(set.low.as_ptr().cast());
            let high = v128_load(set.high.as_ptr().cast());
            let hits = v128_and(
                u8x16_swizzle(low, v128_and(bytes, u8x16_splat(15))),
                u8x16_swizzle(high, u8x16_shr(bytes, 4)),
            );
            if !v128_any_true(hits) {
                return 0;
            }
            u32::from(u8x16_bitmask(u8x16_ne(hits, u8x16_splat(0))))
        }
    }
}

#[cfg(target_arch = "aarch64")]
mod imp {
    use core::arch::aarch64::*;

    /// Units from 0x80 up become 0x80.
    #[inline(always)]
    pub fn narrow(chunk: &[u16; 16]) -> [u8; 16] {
        // SAFETY: loads of 32 bytes the caller owns.
        unsafe {
            let low = vminq_u16(vld1q_u16(chunk.as_ptr()), vdupq_n_u16(0x80));
            let high = vminq_u16(vld1q_u16(chunk.as_ptr().add(8)), vdupq_n_u16(0x80));
            let mut out = [0u8; 16];
            vst1q_u8(
                out.as_mut_ptr(),
                vcombine_u8(vmovn_u16(low), vmovn_u16(high)),
            );
            out
        }
    }

    #[inline(always)]
    pub fn mask16(set: &super::ByteSet, chunk: &[u8; 16]) -> u32 {
        // SAFETY: NEON is always present on aarch64; loads of arrays the caller owns.
        unsafe {
            let bytes = vld1q_u8(chunk.as_ptr());
            let hits = vandq_u8(
                vqtbl1q_u8(vld1q_u8(set.low.as_ptr()), vandq_u8(bytes, vdupq_n_u8(15))),
                vqtbl1q_u8(vld1q_u8(set.high.as_ptr()), vshrq_n_u8(bytes, 4)),
            );
            if vmaxvq_u8(hits) == 0 {
                return 0;
            }
            // Narrow each lane to a nibble, then pick one bit per lane.
            let nonzero = vtstq_u8(hits, hits);
            let nibbles = vshrn_n_u16(vreinterpretq_u16_u8(nonzero), 4);
            let bits = vget_lane_u64(vreinterpret_u64_u8(nibbles), 0);
            // Each lane is 4 bits wide: keep one bit per lane.
            let mut mask = 0u32;
            let mut rest = bits & 0x1111_1111_1111_1111;
            while rest != 0 {
                let lane = rest.trailing_zeros() / 4;
                mask |= 1 << lane;
                rest &= rest - 1;
            }
            mask
        }
    }
}

#[cfg(not(any(
    all(target_arch = "wasm32", target_feature = "simd128"),
    target_arch = "aarch64"
)))]
mod imp {
    #[inline(always)]
    pub fn narrow(chunk: &[u16; 16]) -> [u8; 16] {
        chunk.map(|unit| unit.min(0x80) as u8)
    }

    #[inline(always)]
    pub fn mask16(set: &super::ByteSet, chunk: &[u8; 16]) -> u32 {
        let mut mask = 0;
        for (index, &byte) in chunk.iter().enumerate() {
            mask |= u32::from(set.contains(byte)) << index;
        }
        mask
    }
}

#[cfg(test)]
mod tests {
    use super::ByteSet;

    #[test]
    fn mask_matches_the_scalar_test() {
        const SET: ByteSet = ByteSet::new(b"*_`\\\n&<>[]~");
        let mut chunk = [0u8; 16];
        for seed in 0u32..20_000 {
            for (index, byte) in chunk.iter_mut().enumerate() {
                *byte = (seed
                    .wrapping_mul(2_654_435_761)
                    .rotate_left(index as u32 * 3)
                    >> 7) as u8;
            }
            let want: u32 = (0..16)
                .map(|i| u32::from(SET.contains(chunk[i])) << i)
                .sum();
            assert_eq!(super::imp::mask16(&SET, &chunk), want, "{chunk:?}");
        }
        let mut units = [0u16; 16];
        for seed in 0u32..20_000 {
            for (index, unit) in units.iter_mut().enumerate() {
                let value = seed
                    .wrapping_mul(2_654_435_761)
                    .rotate_left(index as u32 * 5);
                *unit = if value & 1 == 0 {
                    (value >> 9) as u16 & 0x7F
                } else {
                    (value >> 7) as u16
                };
            }
            let want: u32 = (0..16)
                .map(|i| u32::from(units[i] < 0x80 && SET.contains(units[i] as u8)) << i)
                .sum();
            assert_eq!(SET.mask16_units(&units), want, "{units:?}");
        }
        for byte in 0..=255u8 {
            assert_eq!(
                SET.contains(byte),
                b"*_`\\\n&<>[]~".contains(&byte),
                "{byte}"
            );
        }
    }
}
