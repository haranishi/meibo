//! Preflight JSON without collecting arrays/maps before official typed decode.
//! `serde_json` may allocate its bounded scratch buffer for escaped strings.
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use std::fmt;

use super::{failure, options::MAX_BODY};
use crate::Result;

const MAX_ROWS: usize = 10_000;
const MAX_COLUMNS: usize = 2_000;
// Conservative allowance per JSON node covers enum/Vec/map representation and
// capacity growth in the official protocol structs. Strings charge their bytes
// separately. The typed response never shares memory with the raw body.
const NODE_BYTES: usize = 128;

#[derive(Clone, Copy, PartialEq)]
enum Field {
    Other,
    Rows,
    Columns,
}

struct Budget {
    remaining: usize,
    rows: usize,
}

impl Budget {
    fn charge<E: de::Error>(&mut self, amount: usize) -> std::result::Result<(), E> {
        self.remaining = self
            .remaining
            .checked_sub(amount)
            .ok_or_else(|| E::custom("remote response workspace limit"))?;
        Ok(())
    }
}

struct Seed<'a> {
    budget: &'a mut Budget,
    field: Field,
}

impl<'de> DeserializeSeed<'de> for Seed<'_> {
    type Value = ();

    fn deserialize<D: de::Deserializer<'de>>(self, de: D) -> std::result::Result<(), D::Error> {
        self.budget.charge(NODE_BYTES)?;
        de.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Seed<'_> {
    type Value = ();

    fn expecting(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.write_str("bounded JSON response")
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_unit<E: de::Error>(self) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<(), E> {
        self.budget.charge(value.len())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<(), A::Error> {
        let mut items = 0_usize;
        while let Some(()) = seq.next_element_seed(Seed {
            budget: self.budget,
            field: Field::Other,
        })? {
            items = items
                .checked_add(1)
                .ok_or_else(|| de::Error::custom("remote response item limit"))?;
            if self.field == Field::Rows {
                self.budget.rows = self
                    .budget
                    .rows
                    .checked_add(1)
                    .ok_or_else(|| de::Error::custom("remote response row limit"))?;
                if self.budget.rows > MAX_ROWS {
                    return Err(de::Error::custom("remote response row limit"));
                }
            }
            if self.field == Field::Columns && items > MAX_COLUMNS {
                return Err(de::Error::custom("remote response column limit"));
            }
        }
        Ok(())
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<(), A::Error> {
        while let Some(field) = map.next_key_seed(Key(self.budget))? {
            map.next_value_seed(Seed {
                budget: self.budget,
                field,
            })?;
        }
        Ok(())
    }
}

struct Key<'a>(&'a mut Budget);

impl<'de> DeserializeSeed<'de> for Key<'_> {
    type Value = Field;
    fn deserialize<D: de::Deserializer<'de>>(self, de: D) -> std::result::Result<Field, D::Error> {
        de.deserialize_str(self)
    }
}

impl Visitor<'_> for Key<'_> {
    type Value = Field;
    fn expecting(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.write_str("JSON key")
    }
    fn visit_str<E: de::Error>(self, key: &str) -> std::result::Result<Field, E> {
        self.0.charge(
            key.len()
                .checked_add(NODE_BYTES)
                .ok_or_else(|| E::custom("remote response key limit"))?,
        )?;
        Ok(match key {
            "rows" => Field::Rows,
            "cols" => Field::Columns,
            _ => Field::Other,
        })
    }
}

pub(super) fn preflight(bytes: &[u8]) -> Result<()> {
    let mut budget = Budget {
        remaining: MAX_BODY,
        rows: 0,
    };
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    Seed {
        budget: &mut budget,
        field: Field::Other,
    }
    .deserialize(&mut decoder)
    .and_then(|()| decoder.end())
    .map_err(|_| failure("invalid or excessive remote database response"))
}

#[cfg(test)]
mod tests {
    use super::preflight;

    #[test]
    fn row_column_and_decoded_workspace_limits_precede_typed_decode() {
        for (field, accepted, rejected) in [("rows", 10_000, 10_001), ("cols", 2_000, 2_001)] {
            let json = |count| format!(r#"{{"{field}":[{}]}}"#, vec!["[]"; count].join(","));
            assert!(preflight(json(accepted).as_bytes()).is_ok());
            assert!(preflight(json(rejected).as_bytes()).is_err());
        }
        let body = format!(r#"{{"other":[{}]}}"#, vec!["[]"; 300_000].join(","));
        assert!(body.len() < 1024 * 1024);
        assert!(preflight(body.as_bytes()).is_err());
    }

    #[test]
    fn escaped_keys_strings_nested_rows_and_trailing_json_are_counted() {
        assert!(preflight(br#"{"r\u006fws":[["\u3057\u3051\u3093"]]}"#).is_ok());
        let rows = format!(
            r#"[{{"rows":[{}]}},{{"rows":[{}]}}]"#,
            vec!["[]"; 5_000].join(","),
            vec!["[]"; 5_001].join(",")
        );
        assert!(preflight(rows.as_bytes()).is_err());
        assert!(preflight(b"{} {}").is_err());
        assert!(preflight(&[b'['; 200]).is_err());
    }
}
