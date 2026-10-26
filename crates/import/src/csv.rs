//! Strict streaming CSV grammar, adapted from the self-owned kamoku parser.
use std::io::{BufRead, BufReader, Read};

pub(crate) const MAX_RECORD_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_COLUMNS: usize = crate::row::COLUMNS.len();

pub(crate) struct Record {
    pub row: u64,
    pub cells: Vec<Vec<u8>>,
}

#[derive(Debug)]
pub(crate) struct ReadError {
    pub row: u64,
    pub reason: &'static str,
}

#[derive(Clone, Copy)]
enum State {
    Start,
    Unquoted,
    Quoted,
    Closed,
}

pub(crate) struct CsvReader<R> {
    input: BufReader<R>,
    prefix: Vec<u8>,
    prefix_at: usize,
    row: u64,
    skip_lf: bool,
}

impl<R: Read> CsvReader<R> {
    pub fn new(input: R, utf8: bool) -> Result<Self, ReadError> {
        let mut reader = Self {
            input: BufReader::new(input),
            prefix: Vec::new(),
            prefix_at: 0,
            row: 1,
            skip_lf: false,
        };
        if utf8 {
            for _ in 0..3 {
                if let Some(byte) = reader.input_byte()? {
                    reader.prefix.push(byte);
                } else {
                    break;
                }
            }
            if reader.prefix == [0xef, 0xbb, 0xbf] {
                reader.prefix.clear();
            }
        }
        Ok(reader)
    }

    fn error(&self, reason: &'static str) -> ReadError {
        ReadError {
            row: self.row,
            reason,
        }
    }

    fn input_byte(&mut self) -> Result<Option<u8>, ReadError> {
        loop {
            match self.input.fill_buf() {
                Ok(buffer) => {
                    let next = buffer.first().copied();
                    if next.is_some() {
                        self.input.consume(1);
                    }
                    return Ok(next);
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return Err(self.error("input I/O failed")),
            }
        }
    }

    fn byte(&mut self) -> Result<Option<u8>, ReadError> {
        if let Some(&byte) = self.prefix.get(self.prefix_at) {
            self.prefix_at += 1;
            Ok(Some(byte))
        } else {
            self.input_byte()
        }
    }

    fn finish(&mut self, mut cells: Vec<Vec<u8>>, field: Vec<u8>) -> Record {
        cells.push(field);
        let row = self.row;
        self.row += 1;
        Record { row, cells }
    }

    pub fn next(&mut self) -> Result<Option<Record>, ReadError> {
        let mut cells = Vec::new();
        let mut field = Vec::new();
        let mut state = State::Start;
        let mut bytes = 0;
        loop {
            let byte = self.byte()?;
            if self.skip_lf {
                self.skip_lf = false;
                if byte == Some(b'\n') {
                    continue;
                }
            }
            let Some(byte) = byte else {
                if matches!(state, State::Quoted) {
                    return Err(self.error("unclosed quoted field"));
                }
                return Ok((bytes != 0).then(|| self.finish(cells, field)));
            };
            bytes += 1;
            if bytes > MAX_RECORD_BYTES {
                return Err(self.error("record exceeds 1 MiB"));
            }
            if matches!(state, State::Quoted) {
                if byte == b'"' {
                    state = State::Closed;
                } else {
                    field.push(byte);
                }
                continue;
            }
            match byte {
                b'"' => match state {
                    State::Start => state = State::Quoted,
                    State::Closed => {
                        field.push(b'"');
                        state = State::Quoted;
                    }
                    _ => return Err(self.error("quote inside an unquoted field")),
                },
                b',' => {
                    if cells.len() + 1 >= MAX_COLUMNS {
                        return Err(self.error("record exceeds 15 columns"));
                    }
                    cells.push(std::mem::take(&mut field));
                    state = State::Start;
                }
                b'\r' | b'\n' => {
                    self.skip_lf = byte == b'\r';
                    return Ok(Some(self.finish(cells, field)));
                }
                _ if matches!(state, State::Closed) => {
                    return Err(self.error("unexpected byte after closing quote"));
                }
                _ => {
                    field.push(byte);
                    state = State::Unquoted;
                }
            }
        }
    }
}
