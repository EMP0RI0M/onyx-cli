// dot_kernel.s — hand-written AArch64 (ARMv8-A) dot product, NEON 4-accumulator
// unroll-by-16.  Signature: dot_kernel(x0=*a, x1=*b, x2=n) -> s0.
//
// Strategy:
//   * Bulk loop consumes 16 floats/iter, 4 loads+4 fmla into 4 accumulators
//     (v16..v19) to break the FMLA dependency chain (4x ILP).
//   * 4 accumulators are pairwise-fadded then horizontally summed via ADDV.
//   * Scalar tail handles 0..15 leftover elements.
//   * Only caller-saved v16..v31 used, no callee-saved clobber, correct AAPCS64.
    .text
    .globl dot_kernel
    .type  dot_kernel, %function
    .p2align 4
dot_kernel:
    mov   x3, x2          // x3 = remaining
    movi  v16.4s, #0
    movi  v17.4s, #0
    movi  v18.4s, #0
    movi  v19.4s, #0

.Lbulk:
    cmp   x3, #16
    b.lt  .Lreduce
    ld1   { v20.4s, v21.4s, v22.4s, v23.4s }, [x0], #64   // a[0..16]
    ld1   { v24.4s, v25.4s, v26.4s, v27.4s }, [x1], #64   // b[0..16]
    fmla  v16.4s, v20.4s, v24.4s
    fmla  v17.4s, v21.4s, v25.4s
    fmla  v18.4s, v22.4s, v26.4s
    fmla  v19.4s, v23.4s, v27.4s
    sub   x3, x3, #16
    b     .Lbulk

.Lreduce:
    fadd  v16.4s, v16.4s, v17.4s
    fadd  v18.4s, v18.4s, v19.4s
    fadd  v16.4s, v16.4s, v18.4s
    // Horizontal FP sum of the 4 lanes: NEON has no faddv (that is SVE), so
    // use pairwise faddp twice, then move lane 0.  addv would INTEGER-add lanes.
    faddp v16.4s, v16.4s, v16.4s      // [A,B,A,B]
    faddp v16.4s, v16.4s, v16.4s      // [A+B,A+B,A+B,A+B]
    fmov  s0, s16                     // s0 = horizontal FP sum

.Ltail:
    cmp   x3, #0
    b.le  .Ldone
    ld1   { v20.s }[0], [x0], #4
    ld1   { v21.s }[0], [x1], #4
    fmul  s22, s20, s21
    fadd  s0, s0, s22
    sub   x3, x3, #1
    b     .Ltail

.Ldone:
    ret
    .size dot_kernel, .-dot_kernel
    .section .note.GNU-stack,"",%progbits
