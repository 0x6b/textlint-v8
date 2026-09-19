const THIRD_PARTY_NOTICES: &str =
    include_str!(concat!(env!("OUT_DIR"), "/THIRD_PARTY_NOTICES.txt"));

/// Returns the license and attribution notices embedded in this build.
pub const fn third_party_notices() -> &'static str {
    THIRD_PARTY_NOTICES
}
