//! Fill empty company contact fields without replacing sales-owned information.
use crate::schema::{WRITE_BASE, counts};
use crate::sql::{OptionalExtension, params};
use crate::types::{DataOrigin, EngineError, Industry, Status};
use crate::{Engine, houjin, normalize, write};

/// A corporate-number match with verified business contacts or synthetic test values.
pub struct CompanyContactUpdate {
    number: String,
    phone: Option<String>,
    website: Option<String>,
    industry: Option<Industry>,
}

impl CompanyContactUpdate {
    /// Validate one CSV record before writing a batch. Empty fields preserve the old value.
    ///
    /// # Errors
    /// Returns a field error for an invalid corporate number, phone or website.
    pub fn new(
        number: &str,
        phone: &str,
        website: &str,
        industry: Option<Industry>,
    ) -> Result<Self, EngineError> {
        houjin::validate(number)?;
        let phone = normalize::phone(phone.trim());
        let website = website.trim();
        if phone.len() > 100
            || phone.chars().any(char::is_control)
            || (!phone.is_empty() && !phone.bytes().any(|b| b.is_ascii_digit()))
        {
            return Err(invalid("phone"));
        }
        if !website.is_empty() {
            let authority = website
                .strip_prefix("https://")
                .or_else(|| website.strip_prefix("http://"))
                .and_then(|url| url.split(['/', '?', '#']).next());
            if website.len() > 2000
                || website
                    .chars()
                    .any(|ch| ch.is_whitespace() || ch.is_control() || ch == '\\')
                || !authority.is_some_and(|host| !host.is_empty() && !host.contains('@'))
            {
                return Err(invalid("website"));
            }
        }
        Ok(Self {
            number: number.into(),
            phone: (!phone.is_empty()).then_some(phone),
            website: (!website.is_empty()).then(|| website.to_owned()),
            industry,
        })
    }
}

fn invalid(field: &str) -> EngineError {
    EngineError::InvalidInput {
        field: field.into(),
        reason: "invalid business contact data".into(),
    }
}

fn storage(error: crate::sql::Error) -> EngineError {
    EngineError::Storage(Box::new(error))
}

/// Counts of input rows that changed a company or made no changes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContactFillReport {
    /// Rows that filled at least one previously empty field.
    pub updated: u64,
    /// Unmatched, suppressed or already populated rows.
    pub skipped: u64,
}

impl Engine {
    /// Match active corporate entities within this database and fill missing values atomically.
    ///
    /// # Errors
    /// Rejects mixed data origins, excessive batches, low disk space or an unavailable database.
    pub fn fill_company_contacts(
        &mut self,
        origin: DataOrigin,
        rows: &[CompanyContactUpdate],
    ) -> Result<ContactFillReport, EngineError> {
        if origin != self.origin() {
            return Err(EngineError::OriginMismatch);
        }
        if rows.len() > 500 {
            return Err(invalid("batch_size"));
        }
        let estimate = rows.iter().fold(WRITE_BASE, |n, row| {
            let bytes = row.phone.as_ref().map_or(0, String::len)
                + row.website.as_ref().map_or(0, String::len);
            n.saturating_add(32_768 + 8 * bytes as u64)
        });
        self.db.write(estimate, |tx| {
            let mut result = ContactFillReport::default();
            let mut lookup = tx
                .prepare(
                    "SELECT id, status, has_phone FROM companies
                 WHERE corporate_number=?1 AND erased_at IS NULL
                 AND status<>5 AND is_sole_proprietor=0",
                )
                .map_err(storage)?;
            let mut update = tx
                .prepare(
                    "UPDATE companies SET phone=coalesce(phone,?1), website=coalesce(website,?2),
                 industry_code=coalesce(industry_code,?3), updated_at=?4
                 WHERE id=?5 AND ((phone IS NULL AND ?1 IS NOT NULL)
                 OR (website IS NULL AND ?2 IS NOT NULL)
                 OR (industry_code IS NULL AND ?3 IS NOT NULL))",
                )
                .map_err(storage)?;
            let now = write::now_secs();
            for row in rows {
                let matched: Option<(i64, u8, bool)> = lookup
                    .query_row([&row.number], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                    .optional()
                    .map_err(storage)?;
                let Some((id, status, had_phone)) = matched else {
                    result.skipped += 1;
                    continue;
                };
                let changed = update
                    .execute(params![
                        row.phone,
                        row.website,
                        row.industry.map(Industry::code),
                        now,
                        id,
                    ])
                    .map_err(storage)?;
                if changed == 0 {
                    result.skipped += 1;
                    continue;
                }
                if !had_phone && row.phone.is_some() {
                    let status = Status::from_code(status).ok_or(EngineError::Corrupt)?;
                    counts::apply_delta(tx, Some((status, false)), Some((status, true)))?;
                }
                result.updated += 1;
            }
            Ok(result)
        })
    }
}
