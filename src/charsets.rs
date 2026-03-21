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

/// Korean syllables sorted by approximate visual density (light → heavy).
pub const HANGUL: &[char] = &[
    '\u{3000}', // fullwidth space
    '이', '기', '시', '리', '디', '니', '비', '지', '미', '피', '히',
    '으', '그', '스', '르', '드', '느', '브', '즈', '므', '프',
    '가', '나', '다', '라', '마', '바', '사', '아', '자', '차', '카', '타', '파', '하',
    '고', '노', '도', '로', '모', '보', '소', '오', '조', '초', '코', '토', '포', '호',
    '구', '누', '두', '루', '무', '부', '수', '우', '주', '추', '쿠', '투', '푸', '후',
    '인', '긴', '신', '린', '딘', '닌', '빈', '진', '민', '핀',
    '간', '난', '단', '란', '만', '반', '산', '안', '잔', '찬', '칸', '탄', '판', '한',
    '곤', '논', '돈', '론', '몬', '본', '손', '온', '존', '촌', '콘', '톤', '폰', '혼',
    '골', '놀', '돌', '롤', '몰', '볼', '솔', '올', '졸', '콜', '톨', '폴', '홀',
    '굴', '눌', '둘', '룰', '물', '불', '술', '울', '줄', '쿨', '풀', '훌',
    '곱', '놉', '돕', '롭', '몹', '봅', '솝', '옵', '좁',
    '관', '달', '람', '맘', '밤', '삼', '잠', '참', '함',
    '굵', '뭄', '흥', '를', '뭘', '쌀', '뿔', '흠', '뿡', '뭇',
];

/// Hiragana sorted by visual density (light → heavy).
pub const HIRAGANA: &[char] = &[
    '\u{3000}', // fullwidth space
    'い', 'こ', 'し', 'く', 'り', 'つ', 'て', 'に', 'の', 'へ',
    'う', 'え', 'か', 'き', 'け', 'さ', 'す', 'せ', 'そ', 'た',
    'ち', 'と', 'な', 'ひ', 'ふ', 'ほ', 'ま', 'み', 'む', 'め',
    'も', 'や', 'ゆ', 'よ', 'ら', 'る', 'れ', 'ろ', 'わ', 'を',
    'あ', 'お', 'ぬ', 'ね', 'は', 'ん',
];

/// Katakana sorted by visual density (light → heavy).
pub const KATAKANA: &[char] = &[
    '\u{3000}', // fullwidth space
    'ノ', 'ソ', 'ン', 'シ', 'ツ', 'ス', 'リ', 'ク', 'ケ', 'コ',
    'イ', 'エ', 'カ', 'キ', 'サ', 'セ', 'タ', 'チ', 'テ', 'ト',
    'ナ', 'ニ', 'ハ', 'ヒ', 'フ', 'ヘ', 'ホ', 'マ', 'ミ', 'ム',
    'メ', 'モ', 'ヤ', 'ユ', 'ヨ', 'ラ', 'ル', 'レ', 'ロ', 'ワ',
    'ア', 'ウ', 'オ', 'ヌ', 'ネ', 'ヲ',
];

/// Common Hanja (CJK characters used in Korean) sorted by stroke count / visual density.
pub const HANJA: &[char] = &[
    '\u{3000}', // fullwidth space
    // 1-3 strokes
    '一', '二', '人', '十', '三', '大', '小', '上', '下', '山', '川', '口', '土',
    // 4-5 strokes
    '日', '月', '火', '水', '木', '金', '王', '中', '天', '心', '文', '方', '正', '生',
    '白', '石', '目', '田', '世', '主', '半', '北', '古', '立',
    // 6-8 strokes
    '年', '自', '名', '地', '光', '同', '回', '安', '色', '行', '西', '百', '有', '死',
    '先', '老', '全', '多', '交', '再', '成', '每', '身', '言', '里', '花', '長', '事',
    '京', '使', '命', '定', '明', '東', '物', '青', '空', '者', '門',
    // 9-12 strokes
    '南', '海', '美', '食', '風', '書', '家', '時', '高', '馬', '鬼', '國', '動', '強',
    '黃', '黑', '道', '電', '愛', '漢', '語', '學',
    // 13+ strokes
    '親', '頭', '龍', '韓', '體', '驗', '觀',
];

/// Available charset names for user selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharsetName {
    Standard,
    Letters,
    Dots,
    Digits,
    Blocks,
    Hangul,
    Hiragana,
    Katakana,
    Hanja,
}

impl CharsetName {
    pub fn chars(self) -> &'static [char] {
        match self {
            CharsetName::Standard => ASCII_STANDARD,
            CharsetName::Letters => ASCII_LETTERS,
            CharsetName::Dots => ASCII_DOTS,
            CharsetName::Digits => ASCII_DIGITS,
            CharsetName::Blocks => ASCII_BLOCKS,
            CharsetName::Hangul => HANGUL,
            CharsetName::Hiragana => HIRAGANA,
            CharsetName::Katakana => KATAKANA,
            CharsetName::Hanja => HANJA,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CharsetName::Standard => "standard",
            CharsetName::Letters => "letters",
            CharsetName::Dots => "dots",
            CharsetName::Digits => "digits",
            CharsetName::Blocks => "blocks",
            CharsetName::Hangul => "한글",
            CharsetName::Hiragana => "ひらがな",
            CharsetName::Katakana => "カタカナ",
            CharsetName::Hanja => "漢字",
        }
    }

    /// Whether this charset uses double-width (fullwidth) characters.
    pub fn is_wide(self) -> bool {
        matches!(self, CharsetName::Hangul | CharsetName::Hiragana | CharsetName::Katakana | CharsetName::Hanja)
    }
}
