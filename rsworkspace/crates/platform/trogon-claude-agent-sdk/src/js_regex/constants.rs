pub(super) const INFINITE: usize = usize::MAX;

/// JavaScript's `WhiteSpace` and `LineTerminator` code points: what `\s` matches and `String.prototype.trim` strips.
pub(super) const WHITESPACE: &[(u16, u16)] = &[
    (0x09, 0x0D),
    (0x20, 0x20),
    (0xA0, 0xA0),
    (0x1680, 0x1680),
    (0x2000, 0x200A),
    (0x2028, 0x2029),
    (0x202F, 0x202F),
    (0x205F, 0x205F),
    (0x3000, 0x3000),
    (0xFEFF, 0xFEFF),
];

/// `\w` for `\b`: ASCII letters, digits and the underscore.
pub(super) const WORD: &[(u16, u16)] = &[(0x30, 0x39), (0x41, 0x5A), (0x5F, 0x5F), (0x61, 0x7A)];
