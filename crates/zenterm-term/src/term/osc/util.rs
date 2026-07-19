pub(super) fn base64_decode(input: &[u8]) -> Result<Vec<u8>, base64::DecodeError> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(input)
}

/// Public base64 encode for use by terminal.rs responses.
pub(crate) fn base64_encode_for_response(input: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(input)
}

#[cfg(test)]
pub(super) fn base64_encode(input: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(input)
}

/// Parse a single `key=value` parameter from `s`.
///
/// Returns `(key, value)` if `s` contains `=`, otherwise `None`.
pub(super) fn split_key_value(s: &str) -> Option<(&str, &str)> {
    let mut parts = s.splitn(2, '=');
    let key = parts.next()?;
    let val = parts.next()?;
    Some((key, val))
}
