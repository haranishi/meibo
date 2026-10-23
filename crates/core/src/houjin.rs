//! 法人番号の検証と合成用番号の決定的な生成。
use crate::types::EngineError;

fn invalid(reason: &str) -> EngineError {
    EngineError::InvalidCorporateNumber {
        reason: reason.to_owned(),
    }
}

fn check_digit(base: &[u8]) -> u8 {
    let sum: u32 = base
        .iter()
        .rev()
        .enumerate()
        .map(|(i, b)| u32::from(b - b'0') * if i % 2 == 0 { 1 } else { 2 })
        .sum();
    u8::try_from(9 - sum % 9).expect("検査用数字は1〜9")
}

/// 13桁のASCII数字と検査用数字を検証する。
///
/// # Errors
/// 桁数・文字・検査用数字が不正なら理由つきで返す。
pub fn validate(number: &str) -> Result<(), EngineError> {
    if number.len() != 13 {
        return Err(invalid("expected 13 ASCII digits"));
    }
    if !number.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid("non-ASCII digit"));
    }
    let bytes = number.as_bytes();
    if bytes[0] - b'0' != check_digit(&bytes[1..]) {
        return Err(invalid("check digit mismatch"));
    }
    Ok(())
}

/// 0始まりの連番から基礎番号が6・8・9始まりの合成番号を作る。
///
/// # Errors
/// 3×10^11件の番号帯を超えると拒否する。折り返しはしない。
pub fn generate(sequence: u64) -> Result<String, EngineError> {
    const BAND_SIZE: u64 = 100_000_000_000;
    let head = match sequence / BAND_SIZE {
        0 => '6',
        1 => '8',
        2 => '9',
        _ => return Err(invalid("synthetic sequence out of range")),
    };
    let base = format!("{head}{:011}", sequence % BAND_SIZE);
    Ok(format!("{}{base}", check_digit(base.as_bytes())))
}
