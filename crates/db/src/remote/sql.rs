//! Inspect trusted SQL without rewriting or splitting the transmitted script.
use super::RemoteErrorKind;
use crate::Result;

#[derive(Clone, Copy, Default)]
pub(super) struct Effects {
    pub inserts: bool,
}

/// The wrapper owns outer transactions. Savepoints cannot commit its BEGIN.
pub(super) fn inspect(sql: &str, in_transaction: bool) -> Result<Effects> {
    let mut statement = Statement::default();
    let mut effects = Effects::default();
    let mut parentheses = 0usize;
    for token in (Tokens { sql, offset: 0 }) {
        match token {
            Token::Word(word) if statement.in_body() => {
                if word.eq_ignore_ascii_case("CASE") {
                    statement.cases += 1;
                } else if word.eq_ignore_ascii_case("END") {
                    if statement.cases == 0 {
                        statement.trigger = Trigger::None;
                    } else {
                        statement.cases -= 1;
                    }
                }
            }
            Token::Word(word) if parentheses == 0 => statement.word(word),
            Token::Semicolon if !statement.in_body() => {
                statement.finish(in_transaction, &mut effects)?;
                statement = Statement::default();
                parentheses = 0;
            }
            Token::Open => parentheses = parentheses.saturating_add(1),
            Token::Close => parentheses = parentheses.saturating_sub(1),
            _ => {}
        }
    }
    statement.finish(in_transaction, &mut effects)?;
    Ok(effects)
}

#[derive(Default)]
struct Statement<'sql> {
    first: [Option<&'sql str>; 4],
    count: usize,
    trigger: Trigger,
    cases: usize,
    inserts: bool,
    with_kind_found: bool,
}

#[derive(Default)]
enum Trigger {
    #[default]
    None,
    Definition,
    Body,
}

impl<'sql> Statement<'sql> {
    fn in_body(&self) -> bool {
        matches!(self.trigger, Trigger::Body)
    }

    fn is(&self, index: usize, word: &str) -> bool {
        self.first[index].is_some_and(|found| found.eq_ignore_ascii_case(word))
    }

    fn word(&mut self, word: &'sql str) {
        if self.count < self.first.len() {
            self.first[self.count] = Some(word);
            self.count += 1;
        }
        if self.is(0, "CREATE")
            && (self.is(1, "TRIGGER")
                || ((self.is(1, "TEMP") || self.is(1, "TEMPORARY")) && self.is(2, "TRIGGER")))
        {
            self.trigger = Trigger::Definition;
        }
        if matches!(self.trigger, Trigger::Definition) && word.eq_ignore_ascii_case("BEGIN") {
            self.trigger = Trigger::Body;
        }
        if self.count == 1 {
            self.inserts =
                word.eq_ignore_ascii_case("INSERT") || word.eq_ignore_ascii_case("REPLACE");
        } else if self.is(0, "WITH")
            && !self.with_kind_found
            && ["INSERT", "REPLACE", "SELECT", "UPDATE", "DELETE"]
                .iter()
                .any(|kind| word.eq_ignore_ascii_case(kind))
        {
            self.with_kind_found = true;
            self.inserts =
                word.eq_ignore_ascii_case("INSERT") || word.eq_ignore_ascii_case("REPLACE");
        }
    }

    fn finish(&self, in_transaction: bool, effects: &mut Effects) -> Result<()> {
        let outer = self.is(0, "BEGIN") || self.is(0, "COMMIT") || self.is(0, "END");
        let rollback = self.is(0, "ROLLBACK");
        let rollback_to = self.is(1, "TO") || (self.is(1, "TRANSACTION") && self.is(2, "TO"));
        let savepoint = self.is(0, "SAVEPOINT") || self.is(0, "RELEASE") || rollback;
        if outer || (rollback && !rollback_to) || (savepoint && !in_transaction) {
            return Err(RemoteErrorKind::Unsupported.error());
        }
        effects.inserts |= self.inserts;
        Ok(())
    }
}

enum Token<'sql> {
    Word(&'sql str),
    Semicolon,
    Open,
    Close,
    Other,
}

struct Tokens<'sql> {
    sql: &'sql str,
    offset: usize,
}

impl<'sql> Iterator for Tokens<'sql> {
    type Item = Token<'sql>;

    fn next(&mut self) -> Option<Self::Item> {
        let bytes = self.sql.as_bytes();
        while self.offset < bytes.len() {
            let index = self.offset;
            let byte = bytes[index];
            self.offset += 1;
            if byte.is_ascii_whitespace() {
                continue;
            }
            if byte == b'-' && bytes.get(self.offset) == Some(&b'-') {
                while self.offset < bytes.len() && bytes[self.offset] != b'\n' {
                    self.offset += 1;
                }
                continue;
            }
            if byte == b'/' && bytes.get(self.offset) == Some(&b'*') {
                self.offset += 1;
                while self.offset < bytes.len() {
                    if bytes[self.offset - 1] == b'*' && bytes[self.offset] == b'/' {
                        self.offset += 1;
                        break;
                    }
                    self.offset += 1;
                }
                continue;
            }
            if matches!(byte, b'\'' | b'"' | b'`' | b'[') {
                let quote = if byte == b'[' { b']' } else { byte };
                while self.offset < bytes.len() {
                    let found = bytes[self.offset];
                    self.offset += 1;
                    if found == quote {
                        if quote != b']' && bytes.get(self.offset) == Some(&quote) {
                            self.offset += 1;
                        } else {
                            break;
                        }
                    }
                }
                return Some(Token::Other);
            }
            if byte.is_ascii_alphabetic() || byte == b'_' {
                while self.offset < bytes.len()
                    && (bytes[self.offset].is_ascii_alphanumeric() || bytes[self.offset] == b'_')
                {
                    self.offset += 1;
                }
                return Some(Token::Word(&self.sql[index..self.offset]));
            }
            return Some(match byte {
                b';' => Token::Semicolon,
                b'(' => Token::Open,
                b')' => Token::Close,
                _ => Token::Other,
            });
        }
        None
    }
}
