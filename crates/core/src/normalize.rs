//! 表示値を変えずに比較・検索のためのキーを作る。
use crate::types::EngineError;
use unicode_normalization::UnicodeNormalization;

/// 社名の比較キー（NFKC、ひらがなをカタカナ、小文字）。
#[must_use]
pub fn name_key(value: &str) -> String {
    value
        .nfkc()
        .map(|c| match c {
            '\u{3041}'..='\u{3096}' | '\u{309d}'..='\u{309e}' => {
                char::from_u32(u32::from(c) + 0x60).unwrap_or(c)
            }
            _ => c,
        })
        .flat_map(char::to_lowercase)
        .collect()
}

/// 検索語を正規化し、正規化後の文字数とともに返す。
///
/// # Errors
/// 正規化後に英数字・かな・漢字等がない語を拒否する。
pub fn search_term(value: &str) -> Result<(String, usize), EngineError> {
    let key = name_key(value);
    if !key.chars().any(char::is_alphanumeric) {
        return Err(EngineError::InvalidSearchTerm);
    }
    let length = key.chars().count();
    Ok((key, length))
}

/// 電話の全角ASCII表記を半角にする。0・ハイフン・他の文字は保つ。
#[must_use]
pub fn phone(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '\u{ff01}'..='\u{ff5e}' => char::from_u32(u32::from(c) - 0xfee0).unwrap_or(c),
            '\u{3000}' => ' ',
            _ => c,
        })
        .collect()
}
