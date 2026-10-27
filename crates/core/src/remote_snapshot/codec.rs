use std::{
    fmt,
    io::{self, Write},
    mem::size_of,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::de::{self, Deserialize, DeserializeSeed, MapAccess, SeqAccess, Visitor};

use super::{MAX_COLUMNS, MAX_DECODE_WORKSPACE, MAX_ROW_BYTES, Result, SnapshotError};
use crate::sql::types::Value;

fn column_count(count: usize) -> Result<()> {
    if (1..=MAX_COLUMNS).contains(&count) {
        Ok(())
    } else {
        Err(SnapshotError::LimitExceeded)
    }
}

struct Output {
    bytes: Vec<u8>,
}

impl Output {
    fn room(&self, length: usize) -> Result<usize> {
        self.bytes
            .len()
            .checked_add(length)
            .filter(|size| *size <= MAX_ROW_BYTES)
            .ok_or(SnapshotError::LimitExceeded)
    }
    fn raw(&mut self, bytes: &[u8]) -> Result<()> {
        self.write_all(bytes)
            .map_err(|_| SnapshotError::LimitExceeded)
    }
    fn string(&mut self, text: &str) -> Result<()> {
        if text.len() > MAX_ROW_BYTES {
            return Err(SnapshotError::LimitExceeded);
        }
        serde_json::to_writer(self, text).map_err(|_| SnapshotError::LimitExceeded)
    }
}

impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let required = self
            .room(bytes.len())
            .map_err(|_| io::Error::other("snapshot bound"))?;
        if required > self.bytes.capacity() {
            let capacity = required
                .max(self.bytes.capacity().saturating_mul(2))
                .clamp(128, MAX_ROW_BYTES);
            self.bytes
                .try_reserve_exact(capacity - self.bytes.len())
                .map_err(|_| io::Error::other("snapshot allocation bound"))?;
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Encode one ordered SQL row as canonical version-one JSON.
/// # Errors
/// Rejects nonfinite reals, too many columns or an encoded row above one MiB.
pub fn encode_row(values: &[Value]) -> Result<Vec<u8>> {
    column_count(values.len())?;
    let mut output = Output { bytes: Vec::new() };
    output.raw(b"{\"version\":1,\"values\":[")?;
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            output.raw(b",")?;
        }
        match value {
            Value::Null => output.raw(b"{\"type\":\"null\"}")?,
            Value::Integer(value) => {
                output.raw(b"{\"type\":\"integer\",\"value\":")?;
                output.string(&value.to_string())?;
                output.raw(b"}")?;
            }
            Value::Real(value) => {
                if !value.is_finite() {
                    return Err(SnapshotError::InvalidRow);
                }
                output.raw(b"{\"type\":\"real\",\"value\":")?;
                output.string(&format!("{:016x}", value.to_bits()))?;
                output.raw(b"}")?;
            }
            Value::Text(value) => {
                output.raw(b"{\"type\":\"text\",\"value\":")?;
                output.string(value)?;
                output.raw(b"}")?;
            }
            Value::Blob(value) => {
                output.raw(b"{\"type\":\"blob\",\"value\":")?;
                let length = base64::encoded_len(value.len(), true)
                    .and_then(|length| length.checked_add(2))
                    .ok_or(SnapshotError::LimitExceeded)?;
                output.room(length)?;
                output.string(&STANDARD.encode(value))?;
                output.raw(b"}")?;
            }
        }
    }
    output.raw(b"]}")?;
    Ok(output.bytes)
}

struct Budget {
    remaining: usize,
    exceeded: bool,
}

impl Budget {
    fn charge<E: de::Error>(&mut self, amount: usize) -> std::result::Result<(), E> {
        if let Some(remaining) = self.remaining.checked_sub(amount) {
            self.remaining = remaining;
            Ok(())
        } else {
            self.exceeded = true;
            Err(E::custom("snapshot workspace bound"))
        }
    }
    fn error(&self) -> SnapshotError {
        if self.exceeded {
            SnapshotError::LimitExceeded
        } else {
            SnapshotError::InvalidRow
        }
    }
}

#[derive(PartialEq)]
enum Field {
    Version,
    Values,
    Type,
    Value,
}

impl<'de> Deserialize<'de> for Field {
    fn deserialize<D: de::Deserializer<'de>>(decoder: D) -> std::result::Result<Self, D::Error> {
        struct Key;
        impl Visitor<'_> for Key {
            type Value = Field;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("snapshot field")
            }
            fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<Field, E> {
                match value {
                    "version" => Ok(Field::Version),
                    "values" => Ok(Field::Values),
                    "type" => Ok(Field::Type),
                    "value" => Ok(Field::Value),
                    _ => Err(E::custom("invalid snapshot field")),
                }
            }
        }
        decoder.deserialize_identifier(Key)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Null,
    Integer,
    Real,
    Text,
    Blob,
}

impl<'de> Deserialize<'de> for Kind {
    fn deserialize<D: de::Deserializer<'de>>(decoder: D) -> std::result::Result<Self, D::Error> {
        struct Tag;
        impl Visitor<'_> for Tag {
            type Value = Kind;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("snapshot tag")
            }
            fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<Kind, E> {
                match value {
                    "null" => Ok(Kind::Null),
                    "integer" => Ok(Kind::Integer),
                    "real" => Ok(Kind::Real),
                    "text" => Ok(Kind::Text),
                    "blob" => Ok(Kind::Blob),
                    _ => Err(E::custom("invalid snapshot tag")),
                }
            }
        }
        decoder.deserialize_str(Tag)
    }
}

struct CellString<'a> {
    kind: Kind,
    budget: &'a mut Budget,
}

impl<'de> DeserializeSeed<'de> for CellString<'_> {
    type Value = Value;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        decoder: D,
    ) -> std::result::Result<Value, D::Error> {
        decoder.deserialize_str(self)
    }
}

impl Visitor<'_> for CellString<'_> {
    type Value = Value;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("typed snapshot string")
    }
    fn visit_str<E: de::Error>(self, text: &str) -> std::result::Result<Value, E> {
        match self.kind {
            Kind::Integer => {
                if text.len() > 20 {
                    return Err(E::custom("invalid snapshot integer"));
                }
                let value = text
                    .parse::<i64>()
                    .map_err(|_| E::custom("invalid snapshot integer"))?;
                if value.to_string() != text {
                    return Err(E::custom("invalid snapshot integer"));
                }
                Ok(Value::Integer(value))
            }
            Kind::Real => {
                if text.len() != 16
                    || !text
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                {
                    return Err(E::custom("invalid snapshot real"));
                }
                let bits = u64::from_str_radix(text, 16)
                    .map_err(|_| E::custom("invalid snapshot real"))?;
                let value = f64::from_bits(bits);
                if !value.is_finite() {
                    return Err(E::custom("invalid snapshot real"));
                }
                Ok(Value::Real(value))
            }
            Kind::Text => {
                self.budget.charge(text.len())?;
                let mut value = String::new();
                value.try_reserve_exact(text.len()).map_err(|_| {
                    self.budget.exceeded = true;
                    E::custom("snapshot allocation bound")
                })?;
                value.push_str(text);
                Ok(Value::Text(value))
            }
            Kind::Blob => {
                self.budget
                    .charge(base64::decoded_len_estimate(text.len()))?;
                STANDARD
                    .decode(text)
                    .map(Value::Blob)
                    .map_err(|_| E::custom("invalid snapshot blob"))
            }
            Kind::Null => Err(E::custom("invalid snapshot null")),
        }
    }
}

struct Cell<'a>(&'a mut Budget);

impl<'de> DeserializeSeed<'de> for Cell<'_> {
    type Value = Value;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        decoder: D,
    ) -> std::result::Result<Value, D::Error> {
        self.0.charge(128)?;
        decoder.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for Cell<'_> {
    type Value = Value;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("typed snapshot cell")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<Value, A::Error> {
        if map.next_key::<Field>()? != Some(Field::Type) {
            return Err(de::Error::custom("invalid snapshot cell"));
        }
        let kind = map.next_value::<Kind>()?;
        let value = if kind == Kind::Null {
            Value::Null
        } else {
            if map.next_key::<Field>()? != Some(Field::Value) {
                return Err(de::Error::custom("invalid snapshot cell"));
            }
            map.next_value_seed(CellString {
                kind,
                budget: self.0,
            })?
        };
        if map.next_key::<Field>()?.is_some() {
            return Err(de::Error::custom("invalid snapshot cell"));
        }
        Ok(value)
    }
}

struct ExtraColumn<'a>(&'a mut Budget);

impl<'de> DeserializeSeed<'de> for ExtraColumn<'_> {
    type Value = ();
    fn deserialize<D: de::Deserializer<'de>>(self, _: D) -> std::result::Result<(), D::Error> {
        self.0.exceeded = true;
        Err(de::Error::custom("snapshot column bound"))
    }
}

struct Values<'a> {
    expected: usize,
    budget: &'a mut Budget,
}

impl<'de> DeserializeSeed<'de> for Values<'_> {
    type Value = Vec<Value>;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        decoder: D,
    ) -> std::result::Result<Vec<Value>, D::Error> {
        decoder.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for Values<'_> {
    type Value = Vec<Value>;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("bounded snapshot values")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Vec<Value>, A::Error> {
        let capacity = self
            .expected
            .checked_mul(size_of::<Value>())
            .ok_or_else(|| de::Error::custom("snapshot capacity bound"))?;
        self.budget.charge(capacity)?;
        let mut values = Vec::new();
        values.try_reserve_exact(self.expected).map_err(|_| {
            self.budget.exceeded = true;
            de::Error::custom("snapshot allocation bound")
        })?;
        for _ in 0..self.expected {
            let value = seq
                .next_element_seed(Cell(self.budget))?
                .ok_or_else(|| de::Error::custom("missing snapshot column"))?;
            values.push(value);
        }
        seq.next_element_seed(ExtraColumn(self.budget))?;
        Ok(values)
    }
}

struct Row<'a> {
    expected: usize,
    budget: &'a mut Budget,
}

impl<'de> DeserializeSeed<'de> for Row<'_> {
    type Value = Vec<Value>;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        decoder: D,
    ) -> std::result::Result<Vec<Value>, D::Error> {
        decoder.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for Row<'_> {
    type Value = Vec<Value>;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("version-one snapshot row")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<Vec<Value>, A::Error> {
        if map.next_key::<Field>()? != Some(Field::Version) || map.next_value::<u64>()? != 1 {
            return Err(de::Error::custom("invalid snapshot version"));
        }
        if map.next_key::<Field>()? != Some(Field::Values) {
            return Err(de::Error::custom("invalid snapshot values"));
        }
        let values = map.next_value_seed(Values {
            expected: self.expected,
            budget: self.budget,
        })?;
        if map.next_key::<Field>()?.is_some() {
            return Err(de::Error::custom("invalid snapshot row"));
        }
        Ok(values)
    }
}

/// Decode one canonical row using a caller's trusted expected column count.
/// # Errors
/// Rejects invalid bytes, types, counts or canonical forms with fixed diagnostics.
/// Column and workspace bounds are checked before collection or string allocation.
pub fn decode_row(bytes: &[u8], expected_columns: usize) -> Result<Vec<Value>> {
    column_count(expected_columns)?;
    if bytes.len() > MAX_ROW_BYTES {
        return Err(SnapshotError::LimitExceeded);
    }
    let mut budget = Budget {
        remaining: MAX_DECODE_WORKSPACE,
        exceeded: false,
    };
    // Reserve logical working space for the input, serde's escaped-string scratch,
    // canonical output and temporary Base64 encoding before any typed allocation.
    let fixed = bytes
        .len()
        .checked_add(3 * MAX_ROW_BYTES)
        .ok_or(SnapshotError::LimitExceeded)?;
    budget.remaining = budget
        .remaining
        .checked_sub(fixed)
        .ok_or(SnapshotError::LimitExceeded)?;
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let values = Row {
        expected: expected_columns,
        budget: &mut budget,
    }
    .deserialize(&mut decoder)
    .and_then(|values| decoder.end().map(|()| values))
    .map_err(|_| budget.error())?;
    if encode_row(&values)? != bytes {
        return Err(SnapshotError::InvalidRow);
    }
    Ok(values)
}
