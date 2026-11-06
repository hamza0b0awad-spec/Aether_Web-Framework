//! # Word-at-a-time validation primitives
//!
//! Validates ASCII character compliance, email structural bytes, and bounds checking.
//! The module name is historical: no vector intrinsics are used here or anywhere in this
//! tree (`core::arch` has no hits in any `.rs` file). The ASCII scan tests eight bytes at a
//! time with one 64-bit mask, which is the "64-bit word ASCII scan" the completeness matrix's
//! *SIMD-style validation* row prices as `PARTIALLY_IMPLEMENTED`.

/// Fast ASCII check examining 8 bytes in parallel via 64-bit integer masks.
#[inline(always)]
pub fn validate_ascii_fast(bytes: &[u8]) -> bool {
    let (chunks, remainder) = bytes.as_chunks::<8>();
    for chunk in chunks {
        let val = u64::from_ne_bytes(*chunk);
        if (val & 0x8080808080808080) != 0 {
            return false;
        }
    }
    for &b in remainder {
        if b >= 128 {
            return false;
        }
    }
    true
}

/// Zero-allocation email basic format validator.
#[inline]
pub fn validate_email_format(email: &str) -> bool {
    let bytes = email.as_bytes();
    if bytes.len() < 3 || bytes.len() > 254 {
        return false;
    }
    let mut at_pos = None;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'@' {
            if at_pos.is_some() {
                return false; // Multiple @
            }
            at_pos = Some(i);
        }
    }
    if let Some(pos) = at_pos {
        pos > 0 && pos < bytes.len() - 1 && bytes[pos + 1..].contains(&b'.')
    } else {
        false
    }
}
