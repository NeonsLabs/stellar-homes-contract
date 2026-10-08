//! Extraordinary grace period extension governance logic.

pub const MAX_GRACE_EXTENSION_SECS: u64 = 60 * 24 * 3600; // 60 days

pub fn validate_grace_extension(requested_secs: u64) -> bool {
    requested_secs > 0 && requested_secs <= MAX_GRACE_EXTENSION_SECS
}
