/// Character sets for ASCII rendering, sorted by visual "brightness" (dark → bright).
/// Each charset maps pixel brightness (0–255) to a character.

/// Standard ASCII ramp — classic, good general-purpose rendering.
pub const ASCII_STANDARD: &[char] = &[
    ' ', '.', '\'', '`', '^', '"', ',', ':', ';', 'I', 'l', '!', 'i', '>', '<', '~', '+', '_',
    '-', '?', ']', '[', '}', '{', '1', ')', '(', '|', '\\', '/', 't', 'f', 'j', 'r', 'x', 'n',
    'u', 'v', 'c', 'z', 'X', 'Y', 'U', 'J', 'C', 'L', 'Q', '0', 'O', 'Z', 'm', 'w', 'q', 'p',
    'd', 'b', 'k', 'h', 'a', 'o', '*', '#', 'M', 'W', '&', '8', '%', 'B', '@', '$',
];

/// Letters only — A-Za-z sorted by visual density.
pub const ASCII_LETTERS: &[char] = &[
    ' ', 'l', 'i', 'I', 'c', 'r', 'v', 'x', 'n', 'u', 'z', 'j', 'f', 't', 'J', 'L', 'C', 'Y',
    'U', 'X', 'Z', 'o', 'a', 'h', 'k', 'b', 'd', 'p', 'q', 'w', 'm', 'O', 'Q', 'S', 'G', 'D',
    'B', 'W', 'M', '#', '@',
];

/// Dots/blocks only — minimal, clean look.
pub const ASCII_DOTS: &[char] = &[' ', '.', '·', ':', '∘', '○', '●', '◉', '■', '█'];

/// Digits only.
pub const ASCII_DIGITS: &[char] = &[' ', '1', '7', ':', ';', '3', '5', '4', '2', '6', '9', '8', '0', '#'];

/// Block elements — pixel-art style, great with color mode.
pub const ASCII_BLOCKS: &[char] = &[' ', '░', '▒', '▓', '█'];

/// Edge direction characters for outline mode.
pub const EDGE_CHARS: &[char] = &['─', '╱', '│', '╲'];

/// Available charset names for user selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharsetName {
    Standard,
    Letters,
    Dots,
    Digits,
    Blocks,
}

impl CharsetName {
    pub fn chars(self) -> &'static [char] {
        match self {
            CharsetName::Standard => ASCII_STANDARD,
            CharsetName::Letters => ASCII_LETTERS,
            CharsetName::Dots => ASCII_DOTS,
            CharsetName::Digits => ASCII_DIGITS,
            CharsetName::Blocks => ASCII_BLOCKS,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CharsetName::Standard => "standard",
            CharsetName::Letters => "letters",
            CharsetName::Dots => "dots",
            CharsetName::Digits => "digits",
            CharsetName::Blocks => "blocks",
        }
    }

}
