//! Samples the guard's tests and the conformance suite refuse, kept in one place.

/// Text that could mislead an approver: control and escape characters, bidirectional controls,
/// separators and invisible format characters, alone or inside an argument.
pub(crate) const MISLEADING_TEXT: [&str; 16] = [
    "echo\n\x1b[2J\u{202e}gnp.exe",
    "a\nb",
    "tab\there",
    "esc\u{1b}[31m",
    "c1\u{85}",
    "del\u{7f}",
    "bidi\u{202e}",
    "iso\u{2066}x",
    "mark\u{200f}",
    "alm\u{61c}",
    "sep\u{2028}",
    "para\u{2029}",
    "zw\u{200b}x",
    "bom\u{feff}",
    "tag\u{e0041}",
    "ok\u{2026}[0 characters hidden]\n",
];

/// Payload hashes that are not 64 lowercase hexadecimal characters.
pub(crate) fn bad_payload_hashes() -> Vec<String> {
    vec![
        String::new(),
        "ab".to_owned(),
        "AB".repeat(32),
        "g".repeat(64),
        "ab".repeat(33),
    ]
}

/// Argument lists that cannot be shown whole to an approver: an argument over 256 characters
/// and more than 32 arguments.
pub(crate) fn unreviewable_argvs() -> Vec<Vec<String>> {
    vec![
        vec!["sh".to_owned(), "x".repeat(257)],
        vec!["sh".to_owned(), "x".repeat(300)],
        (0..33).map(|i| format!("a{i}")).collect(),
    ]
}
