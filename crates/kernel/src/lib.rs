//! tektile kernel: hand-written AArch64 NEON math + portable fallback.
//! Rust owns orchestration & safety; assembly owns the hot numeric loop.

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
extern "C" {
    fn dot_kernel(a: *const f32, b: *const f32, n: usize) -> f32;
}

/// f32 dot product. On aarch64 Linux dispatches to the NEON kernel; elsewhere a
/// portable loop that LLVM auto-vectorizes.
#[inline]
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len(), "dot: length mismatch");
    if a.is_empty() {
        return 0.0;
    }
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    {
        // SAFETY: equal non-zero lengths; kernel reads exactly n elements from each.
        unsafe { dot_kernel(a.as_ptr(), b.as_ptr(), a.len()) }
    }
    #[cfg(not(all(target_arch = "aarch64", target_os = "linux")))]
    {
        a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
    }
}

/// Portable reference (used to validate the asm kernel).
pub fn dot_ref(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// Cosine similarity — a real AI primitive built directly on the dot kernel.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let d = dot(a, b);
    let na = dot(a, a).sqrt();
    let nb = dot(b, b).sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        d / (na * nb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dot_matches_ref_all_lengths() {
        for n in [1usize, 3, 8, 15, 16, 17, 31, 32, 64, 100, 1000] {
            let a = vec![1.0f32; n];
            let b = vec![2.0f32; n];
            assert_eq!(dot(&a, &b), dot_ref(&a, &b), "n={n}");
        }
    }

    #[test]
    fn cosine_identical_is_one() {
        let a = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        assert!((cosine(&a, &a) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn dot_big_sin_cos_matches_within_tol() {
        let n = 1 << 16;
        let a: Vec<f32> = (0..n).map(|i| (i as f32).sin()).collect();
        let b: Vec<f32> = (0..n).map(|i| (i as f32).cos()).collect();
        let d = dot(&a, &b);
        let r = dot_ref(&a, &b);
        assert!((d - r).abs() / r.abs().max(1e-9) < 1e-3, "d={d} r={r}");
    }
}
