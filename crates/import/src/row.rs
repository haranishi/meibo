use encoding_rs::SHIFT_JIS;
use meibo_core::types::{CompanyInput, DataOrigin, Industry, Status, UserId};

use crate::Encoding;

pub(crate) const COLUMNS: [&str; 13] = [
    "origin",
    "corporate_number",
    "name",
    "prefecture",
    "municipality",
    "industry",
    "phone",
    "website",
    "owner",
    "status",
    "is_sole_proprietor",
    "source_id",
    "source_as_of",
];

pub(crate) fn decode(cells: Vec<Vec<u8>>, encoding: Encoding) -> Result<Vec<String>, &'static str> {
    cells
        .into_iter()
        .map(|cell| match encoding {
            Encoding::Utf8 => String::from_utf8(cell).map_err(|_| "invalid UTF-8 byte sequence"),
            Encoding::ShiftJis => SHIFT_JIS
                .decode_without_bom_handling_and_without_replacement(&cell)
                .map(std::borrow::Cow::into_owned)
                .ok_or("invalid Shift_JIS byte sequence"),
        })
        .collect()
}

pub(crate) struct Headers {
    positions: [Option<usize>; 13],
    width: usize,
}

impl Headers {
    pub fn new(cells: &[String]) -> Result<Self, &'static str> {
        let mut positions = [None; 13];
        for (position, cell) in cells.iter().enumerate() {
            let index = COLUMNS
                .iter()
                .position(|known| cell == known)
                .ok_or("unknown or empty header")?;
            if positions[index].replace(position).is_some() {
                return Err("duplicate header");
            }
        }
        if [0, 2, 3]
            .into_iter()
            .any(|index| positions[index].is_none())
        {
            return Err("required headers are origin, name and prefecture");
        }
        Ok(Self {
            positions,
            width: cells.len(),
        })
    }

    pub fn company(&self, cells: &[String]) -> Result<CompanyInput, &'static str> {
        if cells.len() != self.width {
            return Err("column count differs from header");
        }
        let value =
            |index: usize| self.positions[index].map_or("", |position| cells[position].as_str());
        let optional = |index: usize| {
            let text = value(index);
            (!text.is_empty()).then(|| text.to_owned())
        };
        let origin = match value(0) {
            "synthetic" => DataOrigin::Synthetic,
            "real" => DataOrigin::Real,
            _ => return Err("origin must be synthetic or real"),
        };
        let prefecture = value(3)
            .parse()
            .map_err(|_| "prefecture must be an integer from 1 to 47")?;
        let industry = if value(5).is_empty() {
            None
        } else {
            Some(
                value(5)
                    .parse()
                    .ok()
                    .and_then(Industry::from_code)
                    .ok_or("industry must be an integer from 1 to 20")?,
            )
        };
        let owner = if value(8).is_empty() {
            None
        } else {
            Some(UserId(
                value(8).parse().map_err(|_| "owner must be an integer")?,
            ))
        };
        let status = if value(9).is_empty() {
            Status::Untouched
        } else {
            value(9)
                .parse()
                .ok()
                .and_then(Status::from_code)
                .ok_or("status must be an integer from 0 to 5")?
        };
        let is_sole_proprietor = match value(10) {
            "" | "false" | "0" => false,
            "true" | "1" => true,
            _ => return Err("is_sole_proprietor must be true, false, 1 or 0"),
        };
        Ok(CompanyInput {
            origin,
            corporate_number: optional(1),
            name: value(2).to_owned(),
            prefecture,
            municipality: optional(4),
            industry,
            phone: optional(6),
            website: optional(7),
            owner,
            status,
            is_sole_proprietor,
            source_id: optional(11),
            source_as_of: optional(12),
        })
    }
}
