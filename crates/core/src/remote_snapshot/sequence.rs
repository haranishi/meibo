use std::fmt;

use serde::de::{self, DeserializeSeed, MapAccess, Visitor};

use super::{Result, SnapshotError};
use crate::sql::Connection;

const NAME: &str = "custom_field_definitions";
const MAX_SEQUENCE_BYTES: usize = 128;

/// Validated high-water for the sole CRM AUTOINCREMENT table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrmSequences {
    high_water: i64,
}

impl CrmSequences {
    /// Last allocated custom-field identifier, including deleted identifiers.
    #[must_use]
    pub const fn high_water(&self) -> i64 {
        self.high_water
    }
}

fn validated(high_water: i64, maximum: i64) -> Result<CrmSequences> {
    if maximum < 0 || high_water < maximum {
        return Err(SnapshotError::InvalidSequence);
    }
    Ok(CrmSequences { high_water })
}

/// Read the known CRM sequence without altering the database.
/// The caller supplies the read transaction or operation gate used by its snapshot.
/// # Errors
/// Rejects unknown, duplicate, missing, mistyped or inconsistent source entries.
pub fn read_crm_sequences(connection: &Connection) -> Result<CrmSequences> {
    let maximum: Option<i64> = connection
        .query_row("SELECT max(id) FROM custom_field_definitions", [], |row| {
            row.get(0)
        })
        .map_err(|_| SnapshotError::InvalidSequence)?;
    let mut statement=connection.prepare(
        "SELECT typeof(name)='text' AND name='custom_field_definitions', typeof(seq)='integer', CASE WHEN typeof(seq)='integer' THEN seq END FROM sqlite_sequence LIMIT 2"
    ).map_err(|_|SnapshotError::InvalidSequence)?;
    let mut rows = statement
        .query([])
        .map_err(|_| SnapshotError::InvalidSequence)?;
    let high_water = if let Some(row) = rows.next().map_err(|_| SnapshotError::InvalidSequence)? {
        let known: bool = row.get(0).map_err(|_| SnapshotError::InvalidSequence)?;
        let integer: bool = row.get(1).map_err(|_| SnapshotError::InvalidSequence)?;
        if !known || !integer {
            return Err(SnapshotError::InvalidSequence);
        }
        row.get::<_, i64>(2)
            .map_err(|_| SnapshotError::InvalidSequence)?
    } else {
        if maximum.is_some() {
            return Err(SnapshotError::InvalidSequence);
        }
        0
    };
    if rows
        .next()
        .map_err(|_| SnapshotError::InvalidSequence)?
        .is_some()
    {
        return Err(SnapshotError::InvalidSequence);
    }
    validated(high_water, maximum.unwrap_or(0))
}

/// Encode the explicit CRM sequence map embedded in a version-one manifest.
/// # Errors
/// Returns a fixed diagnostic if the sequence state is invalid.
pub fn encode_crm_sequences(state: CrmSequences) -> Result<Vec<u8>> {
    validated(state.high_water, 0)?;
    Ok(format!("{{\"{NAME}\":\"{}\"}}", state.high_water).into_bytes())
}

struct Name;
impl<'de> DeserializeSeed<'de> for Name {
    type Value = ();
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        decoder: D,
    ) -> std::result::Result<(), D::Error> {
        decoder.deserialize_identifier(self)
    }
}
impl Visitor<'_> for Name {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("known CRM sequence")
    }
    fn visit_str<E: de::Error>(self, name: &str) -> std::result::Result<(), E> {
        if name == NAME {
            Ok(())
        } else {
            Err(E::custom("unknown snapshot sequence"))
        }
    }
}

struct Number;
impl<'de> DeserializeSeed<'de> for Number {
    type Value = i64;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        decoder: D,
    ) -> std::result::Result<i64, D::Error> {
        decoder.deserialize_str(self)
    }
}
impl Visitor<'_> for Number {
    type Value = i64;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("sequence decimal string")
    }
    fn visit_str<E: de::Error>(self, text: &str) -> std::result::Result<i64, E> {
        if text.len() > 19 {
            return Err(E::custom("invalid snapshot sequence"));
        }
        let number = text
            .parse::<i64>()
            .map_err(|_| E::custom("invalid snapshot sequence"))?;
        if number < 0 || number.to_string() != text {
            return Err(E::custom("invalid snapshot sequence"));
        }
        Ok(number)
    }
}

struct SequenceMap;
impl<'de> DeserializeSeed<'de> for SequenceMap {
    type Value = i64;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        decoder: D,
    ) -> std::result::Result<i64, D::Error> {
        decoder.deserialize_map(self)
    }
}
impl<'de> Visitor<'de> for SequenceMap {
    type Value = i64;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("explicit CRM sequence map")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<i64, A::Error> {
        if map.next_key_seed(Name)?.is_none() {
            return Err(de::Error::custom("missing snapshot sequence"));
        }
        let number = map.next_value_seed(Number)?;
        if map.next_key_seed(Name)?.is_some() {
            return Err(de::Error::custom("duplicate snapshot sequence"));
        }
        Ok(number)
    }
}

/// Decode the exact CRM sequence map and compare it with the current maximum id.
/// # Errors
/// Rejects unknown, duplicate, missing, negative, overflowing or too-small values.
pub fn decode_crm_sequences(bytes: &[u8], current_max_id: i64) -> Result<CrmSequences> {
    if bytes.len() > MAX_SEQUENCE_BYTES {
        return Err(SnapshotError::InvalidSequence);
    }
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let value = SequenceMap
        .deserialize(&mut decoder)
        .and_then(|value| decoder.end().map(|()| value))
        .map_err(|_| SnapshotError::InvalidSequence)?;
    let state = validated(value, current_max_id)?;
    if encode_crm_sequences(state)? != bytes {
        return Err(SnapshotError::InvalidSequence);
    }
    Ok(state)
}
