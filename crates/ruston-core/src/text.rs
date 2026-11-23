//! Checks on untrusted text that every frontend shows.

/// Characters a file manager, terminal or notification draws as nothing.
/// Direction overrides and isolates can make `fdp.scr` read as `rcs.pdf`, or
/// reverse the address after a sender's name; the zero-width space, word
/// joiner and byte-order mark only hide parts of the text. Joiners (U+200C,
/// U+200D) stay: words in several scripts and emoji need them, and they
/// cannot reorder or hide anything.
pub fn is_invisible_mark(c: char) -> bool {
    matches!(
        c,
        '\u{061C}'
            | '\u{200B}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
    )
}
