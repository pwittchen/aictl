use crate::config::MAX_TOOL_OUTPUT_LEN;

/// Truncate a result string to the output size limit.
/// Walks back to the nearest UTF-8 char boundary so multi-byte characters
/// landing on the cut don't trigger a panic in `String::truncate`.
pub(super) fn truncate_output(s: &mut String) {
    if s.len() > MAX_TOOL_OUTPUT_LEN {
        let mut idx = MAX_TOOL_OUTPUT_LEN;
        while idx > 0 && !s.is_char_boundary(idx) {
            idx -= 1;
        }
        s.truncate(idx);
        s.push_str("\n... (truncated)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_output_short() {
        let mut s = "short".to_string();
        truncate_output(&mut s);
        assert_eq!(s, "short");
    }

    #[test]
    fn truncate_output_over_limit() {
        let mut s = "x".repeat(MAX_TOOL_OUTPUT_LEN + 100);
        truncate_output(&mut s);
        assert!(s.ends_with("\n... (truncated)"));
        assert!(s.len() <= MAX_TOOL_OUTPUT_LEN + 20);
    }

    #[test]
    fn truncate_output_multibyte_on_boundary() {
        // Build a string where a multi-byte UTF-8 char straddles MAX_TOOL_OUTPUT_LEN.
        // 'é' is 2 bytes in UTF-8. Padding to MAX_TOOL_OUTPUT_LEN - 1 bytes of ASCII
        // then appending 'é' puts the char's first byte at MAX-1 and second byte at MAX,
        // so a naive truncate(MAX) would split it and panic.
        let mut s = "a".repeat(MAX_TOOL_OUTPUT_LEN - 1);
        s.push('é');
        s.push_str(&"b".repeat(100));
        truncate_output(&mut s);
        assert!(s.ends_with("\n... (truncated)"));
        assert!(s.is_char_boundary(s.len()));
    }
}
