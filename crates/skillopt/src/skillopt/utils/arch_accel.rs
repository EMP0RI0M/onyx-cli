//! Architecture-specific assembly and SIMD acceleration for SkillOpt evaluation and scoring.
//!
//! Provides optimized routines for:
//! 1. AArch64 inline assembly branchless `min3` in Levenshtein distance inner loops (`csel`).
//! 2. AArch64 NEON vector character punctuation and whitespace normalization.
//! 3. Portable fallback implementations ensuring identical differential behavior on any architecture.

#[cfg(target_arch = "aarch64")]
use std::arch::aarch64::*;
#[cfg(target_arch = "aarch64")]
use std::arch::asm;

/// Branchless minimum of three unsigned 32-bit integers.
/// Uses native AArch64 `cmp` + `csel` instructions when compiled on AArch64.
#[inline(always)]
pub fn min3_u32(a: u32, b: u32, c: u32) -> u32 {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let res: u32;
        asm!(
            "cmp {a:w}, {b:w}",
            "csel {res:w}, {a:w}, {b:w}, ls",
            "cmp {res:w}, {c:w}",
            "csel {res:w}, {res:w}, {c:w}, ls",
            a = in(reg) a,
            b = in(reg) b,
            c = in(reg) c,
            res = out(reg) res,
            options(pure, nomem, nostack)
        );
        res
    }

    #[cfg(not(target_arch = "aarch64"))]
    {
        a.min(b).min(c)
    }
}

/// Fast ASCII punctuation stripping.
/// Accelerated with 128-bit NEON vector operations on AArch64.
pub fn fast_strip_ascii_punctuation(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;

    #[cfg(target_arch = "aarch64")]
    unsafe {
        // Process in 16-byte chunks using NEON
        while i + 16 <= bytes.len() {
            let chunk = vld1q_u8(bytes.as_ptr().add(i));

            // ASCII punctuation ranges:
            // 0x21..=0x2F (! to /)
            let ge_21 = vcgeq_u8(chunk, vdupq_n_u8(0x21));
            let le_2f = vcleq_u8(chunk, vdupq_n_u8(0x2F));
            let is_r1 = vandq_u8(ge_21, le_2f);

            // 0x3A..=0x40 (: to @)
            let ge_3a = vcgeq_u8(chunk, vdupq_n_u8(0x3A));
            let le_40 = vcleq_u8(chunk, vdupq_n_u8(0x40));
            let is_r2 = vandq_u8(ge_3a, le_40);

            // 0x5B..=0x60 ([ to `)
            let ge_5b = vcgeq_u8(chunk, vdupq_n_u8(0x5B));
            let le_60 = vcleq_u8(chunk, vdupq_n_u8(0x60));
            let is_r3 = vandq_u8(ge_5b, le_60);

            // 0x7B..=0x7E ({ to ~)
            let ge_7b = vcgeq_u8(chunk, vdupq_n_u8(0x7B));
            let le_7e = vcleq_u8(chunk, vdupq_n_u8(0x7E));
            let is_r4 = vandq_u8(ge_7b, le_7e);

            let is_punct = vorrq_u8(vorrq_u8(is_r1, is_r2), vorrq_u8(is_r3, is_r4));

            let mut mask_arr = [0u8; 16];
            vst1q_u8(mask_arr.as_mut_ptr(), is_punct);

            for j in 0..16 {
                if mask_arr[j] == 0 {
                    out.push(bytes[i + j]);
                }
            }
            i += 16;
        }
    }

    // Scalar remainder or portable path
    while i < bytes.len() {
        let b = bytes[i];
        if !b.is_ascii_punctuation() {
            out.push(b);
        }
        i += 1;
    }

    String::from_utf8(out).unwrap_or_else(|_| {
        input.chars().filter(|c| !c.is_ascii_punctuation()).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_min3_u32_cases() {
        assert_eq!(min3_u32(10, 20, 30), 10);
        assert_eq!(min3_u32(20, 10, 30), 10);
        assert_eq!(min3_u32(30, 20, 10), 10);
        assert_eq!(min3_u32(42, 42, 42), 42);
        assert_eq!(min3_u32(100, 5, 200), 5);
        assert_eq!(min3_u32(0, 1, 2), 0);
    }

    #[test]
    fn test_fast_strip_ascii_punctuation_parity() {
        let sample = "Hello, world! [SkillOpt & Tektile: 100% hybrid/optimized]-test#42; 'success' ~ok?";
        let fast_res = fast_strip_ascii_punctuation(sample);
        let standard_res: String = sample.chars().filter(|c| !c.is_ascii_punctuation()).collect();
        assert_eq!(fast_res, standard_res);
    }
}
