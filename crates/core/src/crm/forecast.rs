use super::{Engine, EngineError, MAX_AMOUNT_JPY, ensure_sales_ops, invalid, storage, valid_date};
use crate::{
    schema::WRITE_BASE,
    sql::{Connection, params},
    types::UserId,
};

/// The explicit owner and the company owner are independent.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpportunityOwnerChange {
    /// Preserve the existing assignment on edit (inherit on creation).
    #[default]
    Keep,
    /// Follow future changes to the company's owner.
    Inherit,
    /// Explicitly leave this opportunity unassigned.
    Unassigned,
    /// Assign this opportunity to a salesperson.
    Assign(UserId),
}

/// Additional fields accepted by the opportunity editor; old clients omit both.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpportunityForecastPatch {
    /// Optional change of salesperson.
    pub owner: OpportunityOwnerChange,
    /// Omit to preserve, set to None to clear, or enter 0 through 100.
    #[allow(clippy::option_option)]
    pub probability: Option<Option<u8>>,
}

impl OpportunityForecastPatch {
    pub(super) fn save(self, conn: &Connection, id: i64) -> Result<(), EngineError> {
        if self == Self::default() {
            return Ok(());
        }
        let (owner_override, owner_id) = match self.owner {
            OpportunityOwnerChange::Keep | OpportunityOwnerChange::Inherit => (false, None),
            OpportunityOwnerChange::Unassigned => (true, None),
            OpportunityOwnerChange::Assign(UserId(id)) if id > 0 => (true, Some(id)),
            OpportunityOwnerChange::Assign(_) => {
                return Err(invalid("owner_id", "expected a positive ID"));
            }
        };
        if self.probability.flatten().is_some_and(|value| value > 100) {
            return Err(invalid("probability", "expected zero through one hundred"));
        }
        ensure_sales_ops(conn)?;
        conn.execute(
            "INSERT INTO crm_opportunity_ownership(opportunity_id,owner_override,owner_id,probability) VALUES(?1,?2,?3,?4)
             ON CONFLICT(opportunity_id) DO UPDATE SET
                owner_override=CASE WHEN ?5 THEN excluded.owner_override ELSE owner_override END,
                owner_id=CASE WHEN ?5 THEN excluded.owner_id ELSE owner_id END,
                probability=CASE WHEN ?6 THEN excluded.probability ELSE probability END",
            params![id, owner_override, owner_id, self.probability.flatten(),
                self.owner != OpportunityOwnerChange::Keep, self.probability.is_some()],
        ).map_err(storage)?;
        Ok(())
    }
}

/// These fragments are selected together inside the caller's read transaction.
pub(super) struct ForecastProjection {
    pub join: &'static str,
    pub owner: &'static str,
    pub inherited: &'static str,
    pub probability: &'static str,
}

impl ForecastProjection {
    pub(super) fn for_version(version: u32) -> Self {
        if version >= 3 {
            Self {
                join: "LEFT JOIN crm_opportunity_ownership f ON f.opportunity_id=o.id",
                owner: "CASE WHEN f.owner_override=1 THEN f.owner_id ELSE c.owner_id END",
                inherited: "coalesce(f.owner_override,0)=0",
                probability: "f.probability",
            }
        } else {
            Self {
                join: "",
                owner: "c.owner_id",
                inherited: "1",
                probability: "NULL",
            }
        }
    }
}

impl Engine {
    /// Set a salesperson's target for one month, or clear it with None. Zero is a target.
    /// # Errors
    /// Returns an error for an invalid month, owner, amount or failed write.
    pub fn crm_set_sales_target(
        &mut self,
        month: &str,
        owner: UserId,
        amount: Option<i64>,
    ) -> Result<(), EngineError> {
        if !valid_date(&format!("{month}-01")) {
            return Err(invalid("month", "expected YYYY-MM"));
        }
        if owner.0 <= 0 {
            return Err(invalid("owner_id", "expected a positive ID"));
        }
        if amount.is_some_and(|value| !(0..=MAX_AMOUNT_JPY).contains(&value)) {
            return Err(invalid("amount_jpy", "expected a nonnegative safe integer"));
        }
        self.db.write(WRITE_BASE + 4096, |tx| {
            ensure_sales_ops(tx)?;
            if let Some(amount) = amount {
                tx.execute(
                    "INSERT INTO crm_sales_targets(month,owner_id,amount_jpy) VALUES(?1,?2,?3)
                     ON CONFLICT(month,owner_id) DO UPDATE SET amount_jpy=excluded.amount_jpy",
                    params![month, owner.0, amount],
                )
            } else {
                tx.execute(
                    "DELETE FROM crm_sales_targets WHERE month=?1 AND owner_id=?2",
                    params![month, owner.0],
                )
            }
            .map_err(storage)?;
            Ok(())
        })
    }
}
