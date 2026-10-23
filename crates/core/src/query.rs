//! 索引の連鎖を使う一覧、件数、詳細。
use rusqlite::{OptionalExtension, Row, params_from_iter, types::Value};

use crate::{
    schema::Db,
    shapes::{self, Shape},
    types::{
        Activity, ActivityId, ActivityKind, Company, CompanyId, Contact, ContactId, Counted,
        Cursor, DataOrigin, Detail, EngineError, Field, Fields, Filters, Industry, ListQuery,
        NextAction, Page, Region, Sort, Status, UserId,
    },
};

pub(crate) fn list(db: &Db, q: &ListQuery) -> Result<Page, EngineError> {
    let _snapshot = db.conn().unchecked_transaction().map_err(storage)?;
    validate_limit(q.limit)?;
    let plan = Plan::new(&q.filters, q.sort)?;
    let after = q.after.as_ref().map(|c| plan.decode(c)).transpose()?;
    let mut rows = Vec::new();
    let mut keys = Vec::new();
    for (sql, mut args) in plan.statements(after.as_deref(), "*", q.limit + 1) {
        // 各範囲は互いに素。必要な残り件数だけ読む。
        *args.last_mut().expect("limit") =
            Value::Integer(i64::from(q.limit + 1) - i64::try_from(rows.len()).expect("page size"));
        let mut stmt = db.conn().prepare(&sql).map_err(storage)?;
        let mut result = stmt.query(params_from_iter(args)).map_err(storage)?;
        while let Some(row) = result.next().map_err(storage)? {
            keys.push(
                plan.columns
                    .iter()
                    .map(|c| row.get::<_, Value>(*c))
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(storage)?,
            );
            rows.push(company_row(row, db.origin(), &q.fields).map_err(storage)?);
        }
        if rows.len() > q.limit as usize {
            break;
        }
    }
    let next = if rows.len() > q.limit as usize {
        rows.pop();
        keys.pop();
        keys.last().map(|key| plan.encode(key))
    } else {
        None
    };
    Ok(Page {
        rows,
        next,
        slow: plan.slow,
    })
}

pub(crate) fn visit_companies(
    db: &Db,
    filters: &Filters,
    fields: &Fields,
    mut visitor: impl FnMut(Company) -> Result<(), EngineError>,
) -> Result<u64, EngineError> {
    let _snapshot = db.conn().unchecked_transaction().map_err(storage)?;
    let mut plan = Plan::new(filters, filter_sort(filters))?;
    plan.condition.push_str(" AND erased_at IS NULL");
    let mut visited = 0;
    for (sql, mut args) in plan.statements(None, "*", 1) {
        // SQLiteの負のLIMITは上限なし。全体をVecや200件の別snapshotへ分けない。
        *args.last_mut().expect("limit") = Value::Integer(-1);
        let mut stmt = db.conn().prepare(&sql).map_err(storage)?;
        let mut rows = stmt.query(params_from_iter(args)).map_err(storage)?;
        while let Some(row) = rows.next().map_err(storage)? {
            visitor(company_row(row, db.origin(), fields).map_err(storage)?)?;
            visited += 1;
        }
    }
    Ok(visited)
}

pub(crate) fn cursor_after_offset(
    db: &Db,
    q: &ListQuery,
    offset: u64,
) -> Result<Option<Cursor>, EngineError> {
    validate_limit(q.limit)?;
    let plan = Plan::new(&q.filters, q.sort)?;
    if offset == 0 {
        return Ok(None);
    }
    let Ok(skip) = i64::try_from(offset - 1) else {
        return Ok(None);
    };
    let (sql, mut args) = plan.statement(&plan.columns.join(","), &[], 2);
    args.push(Value::Integer(skip));
    let mut stmt = db
        .conn()
        .prepare(&format!("{sql} OFFSET ?"))
        .map_err(storage)?;
    let mut rows = stmt.query(params_from_iter(args)).map_err(storage)?;
    let Some(row) = rows.next().map_err(storage)? else {
        return Ok(None);
    };
    let key = (0..plan.columns.len())
        .map(|i| row.get::<_, Value>(i))
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(storage)?;
    // 続きがない位置にはカーソルを返さない。
    if rows.next().map_err(storage)?.is_none() {
        return Ok(None);
    }
    Ok(Some(plan.encode(&key)))
}

pub(crate) fn count(db: &Db, f: &Filters) -> Result<Counted, EngineError> {
    let plan = Plan::new(f, filter_sort(f))?;
    let quick = matches!(plan.number, 16 | 17 | 18 | 22);
    let (sql, args) = if quick {
        (
            format!(
                "SELECT coalesce(sum(n),0) FROM counts WHERE {}",
                plan.condition
            ),
            plan.args.clone(),
        )
    } else {
        (
            format!(
                "SELECT count(*) FROM companies{} WHERE {}",
                plan.index, plan.condition
            ),
            plan.args.clone(),
        )
    };
    let n = db
        .conn()
        .query_row(&sql, params_from_iter(args), unsigned)
        .map_err(storage)?;
    Ok(Counted { n, slow: plan.slow })
}

pub(crate) fn select_ids_sql(f: &Filters, limit: u32) -> Result<(String, Vec<Value>), EngineError> {
    if !(1..=100_000).contains(&limit) {
        return Err(invalid("limit", "must be 1..=100000"));
    }
    let plan = Plan::new(f, filter_sort(f))?;
    Ok(plan.statement("id", &[], limit))
}

pub(crate) fn explain(db: &Db, q: &ListQuery) -> Result<Vec<String>, EngineError> {
    validate_limit(q.limit)?;
    let plan = Plan::new(&q.filters, q.sort)?;
    let after = q.after.as_ref().map(|c| plan.decode(c)).transpose()?;
    let mut output = Vec::new();
    for (sql, args) in plan.statements(after.as_deref(), "*", q.limit + 1) {
        let mut stmt = db
            .conn()
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .map_err(storage)?;
        let rows = stmt
            .query_map(params_from_iter(args), |r| r.get::<_, String>(3))
            .map_err(storage)?;
        output.extend(
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(storage)?,
        );
    }
    Ok(output)
}

pub(crate) fn invalid(field: &str, reason: &str) -> EngineError {
    EngineError::InvalidInput {
        field: field.into(),
        reason: reason.into(),
    }
}
pub(crate) fn unsigned(row: &Row<'_>) -> rusqlite::Result<u64> {
    let n: i64 = row.get(0)?;
    u64::try_from(n).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(0, n))
}
pub(crate) fn storage(error: rusqlite::Error) -> EngineError {
    EngineError::Storage(Box::new(error))
}
pub(crate) fn validate_limit(limit: u32) -> Result<(), EngineError> {
    if (1..=200).contains(&limit) {
        Ok(())
    } else {
        Err(invalid("limit", "must be 1..=200"))
    }
}

fn filter_sort(f: &Filters) -> Sort {
    if f.next_action_before.is_some() {
        Sort::NextAction
    } else {
        Sort::ShapeDefault
    }
}

const P: &str = "prefecture_code";
const M: &str = "municipality_code";
const I: &str = "industry_code";
const S: &str = "status";
const T: &str = "has_phone";
const O: &str = "owner_id";
const N: &str = "next_action_at";
const A: &str = "last_activity_at";
const ID: &str = "id";

// 番号順は shapes::SHAPES と同じ。列はSQLの静的断片だけ。
const ORDERS: [&[&str]; 22] = [
    &[P, S, T, ID],
    &[P, S, T, ID],
    &[P, S, T, ID],
    &[P, I, S, T, ID],
    &[P, I, S, T, ID],
    &[P, I, S, T, ID],
    &[M, S, T, ID],
    &[M, S, T, ID],
    &[M, S, T, ID],
    &[M, I, S, T, ID],
    &[M, I, S, T, ID],
    &[M, I, S, T, ID],
    &[I, S, T, ID],
    &[I, S, T, ID],
    &[I, S, T, ID],
    &[S, T, ID],
    &[S, T, ID],
    &[T, S, ID],
    &[O, N, S, ID],
    &[N, S, ID],
    &[A, S, ID],
    &[ID],
];

#[derive(Clone)]
struct Plan {
    number: u8,
    slow: bool,
    index: String,
    columns: &'static [&'static str],
    desc: bool,
    condition: String,
    args: Vec<Value>,
    fixed: Vec<(&'static str, Vec<Value>)>,
    before: Option<i64>,
    exclude: bool,
}
impl Plan {
    fn new(f: &Filters, sort: Sort) -> Result<Self, EngineError> {
        let mut predicates = Vec::new();
        let mut args = Vec::new();
        let mut fixed = Vec::new();
        let (regions_p, regions_m) = region_values(&f.region)?;
        let mixed = !regions_p.is_empty() && !regions_m.is_empty();
        if mixed {
            let p = membership(P, &regions_p, &mut args);
            let m = membership(M, &regions_m, &mut args);
            predicates.push(format!("({p} OR {m})"));
        }
        for (col, mut values) in [
            (P, if mixed { vec![] } else { regions_p }),
            (M, if mixed { vec![] } else { regions_m }),
            (
                I,
                f.industry
                    .iter()
                    .map(|i| Value::Integer(i64::from(i.code())))
                    .collect(),
            ),
            (
                S,
                f.status
                    .iter()
                    .map(|s| Value::Integer(i64::from(s.code())))
                    .collect(),
            ),
            (
                T,
                f.has_phone
                    .map(|t| vec![Value::Integer(i64::from(t))])
                    .unwrap_or_default(),
            ),
            (O, f.owner.iter().map(|o| Value::Integer(o.0)).collect()),
        ] {
            values.sort_by(value_cmp);
            values.dedup();
            if !values.is_empty() {
                predicates.push(membership(col, &values, &mut args));
                fixed.push((col, values));
            }
        }
        if let Some(n) = f.next_action_before {
            predicates.push(format!("{N} <= ?"));
            args.push(Value::Integer(n));
        }
        let exclude = f.status.is_empty() && !f.include_excluded;
        if exclude {
            predicates.push("status <> 5".into());
        }
        let shape = shapes::classify(f, &sort);
        let (number, slow, index, columns) = match shape {
            Shape::Declared(n) => {
                let spec = &shapes::SHAPES[usize::from(n - 1)];
                (
                    n,
                    false,
                    if n == 22 {
                        String::new()
                    } else {
                        format!(" INDEXED BY {}", spec.index.to_ascii_lowercase())
                    },
                    ORDERS[usize::from(n - 1)],
                )
            }
            Shape::Slow => match sort {
                Sort::ShapeDefault => (23, true, String::new(), &[ID][..]),
                Sort::NextAction => (24, true, String::new(), &[N, ID][..]),
                Sort::LastActivityDesc => (25, true, String::new(), &[A, ID][..]),
            },
        };
        Ok(Self {
            number,
            slow,
            index,
            columns,
            desc: sort == Sort::LastActivityDesc,
            condition: if predicates.is_empty() {
                "1".into()
            } else {
                predicates.join(" AND ")
            },
            args,
            fixed,
            before: f.next_action_before,
            exclude,
        })
    }

    fn statement(
        &self,
        projection: &str,
        extra: &[(String, Vec<Value>)],
        limit: u32,
    ) -> (String, Vec<Value>) {
        let mut condition = self.condition.clone();
        let mut args = self.args.clone();
        for (sql, values) in extra {
            condition.push_str(" AND ");
            condition.push_str(sql);
            args.extend_from_slice(values);
        }
        let order = self
            .columns
            .iter()
            .map(|c| {
                if self.desc {
                    format!("{c} DESC")
                } else {
                    (*c).into()
                }
            })
            .collect::<Vec<String>>()
            .join(",");
        args.push(Value::Integer(i64::from(limit)));
        (
            format!(
                "SELECT {projection} FROM companies{} WHERE {condition} ORDER BY {order} LIMIT ?",
                self.index
            ),
            args,
        )
    }

    fn statements(
        &self,
        after: Option<&[Value]>,
        projection: &str,
        limit: u32,
    ) -> Vec<(String, Vec<Value>)> {
        let multi = if self.slow {
            None
        } else {
            self.fixed.iter().find(|(_, v)| v.len() > 1)
        };
        let groups: Vec<Option<Value>> = multi.map_or_else(
            || vec![None],
            |(_, values)| values.iter().cloned().map(Some).collect(),
        );
        let mut result = Vec::new();
        for group in groups {
            let extra = Vec::new();
            let mut group_plan = self.clone();
            let mut cursor = after;
            if let (Some((col, _)), Some(value)) = (multi, group) {
                if let Some(key) = after {
                    let pos = self
                        .columns
                        .iter()
                        .position(|c| c == col)
                        .expect("declared multiple column");
                    match value_cmp(&value, &key[pos]) {
                        std::cmp::Ordering::Less => continue,
                        std::cmp::Ordering::Greater => cursor = None,
                        std::cmp::Ordering::Equal => (),
                    }
                }
                group_plan.set_group(col, value);
            }
            if let Some(key) = cursor {
                for j in (0..self.columns.len()).rev() {
                    // 宣言形の等号列は単一の値に固定済み。
                    if !self.slow && self.fixed.iter().any(|(c, _)| *c == self.columns[j]) {
                        continue;
                    }
                    let mut branch = extra.clone();
                    for (col, value) in self.columns[..j].iter().zip(&key[..j]) {
                        if !self.slow && self.fixed.iter().any(|(c, _)| c == col) {
                            continue;
                        }
                        branch.push((format!("{col} IS ?"), vec![value.clone()]));
                    }
                    let col = self.columns[j];
                    let value = &key[j];
                    let comparison = if *value == Value::Null {
                        if self.desc {
                            continue;
                        }
                        (format!("{col} IS NOT NULL"), vec![])
                    } else if self.desc {
                        (format!("({col} < ? OR {col} IS NULL)"), vec![value.clone()])
                    } else {
                        (format!("{col} > ?"), vec![value.clone()])
                    };
                    // 宣言形の列はNOT NULLか条件でNULLを除外済み。
                    let comparison = if self.desc && !self.slow {
                        (format!("{col} < ?"), vec![value.clone()])
                    } else {
                        comparison
                    };
                    branch.push(comparison);
                    result.push(group_plan.statement(projection, &branch, limit));
                }
            } else {
                result.push(group_plan.statement(projection, &extra, limit));
            }
        }
        result
    }

    fn set_group(&mut self, col: &str, value: Value) {
        let mut offset = 0;
        for (column, values) in &mut self.fixed {
            if *column == col {
                let predicate = format!("{col} IN ({})", vec!["?"; values.len()].join(","));
                // INと等号の重複はSQLiteが一時ソートを選ぶため置換する。
                self.condition = self.condition.replace(&predicate, &format!("{col} IN (?)"));
                self.args
                    .splice(offset..offset + values.len(), [value.clone()]);
                *values = vec![value];
                return;
            }
            offset += values.len();
        }
    }

    fn encode(&self, key: &[Value]) -> Cursor {
        let values = key
            .iter()
            .map(|v| match v {
                Value::Integer(n) => n.to_string(),
                Value::Text(s) => s.clone(),
                Value::Null => "n".into(),
                _ => unreachable!("index key"),
            })
            .collect::<Vec<_>>()
            .join(",");
        Cursor {
            token: format!("v1:{}:{values}", self.number),
        }
    }

    fn decode(&self, cursor: &Cursor) -> Result<Vec<Value>, EngineError> {
        let bad = || {
            invalid(
                "after",
                "invalid cursor or cursor does not match filters and sort",
            )
        };
        let prefix = format!("v1:{}:", self.number);
        let text = cursor.token.strip_prefix(&prefix).ok_or_else(bad)?;
        let parts: Vec<_> = text.split(',').collect();
        if parts.len() != self.columns.len() {
            return Err(bad());
        }
        let mut values = Vec::new();
        for (col, part) in self.columns.iter().zip(parts) {
            let value = if *col == M {
                if part.len() != 5 || !part.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(bad());
                }
                Value::Text(part.into())
            } else if part == "n" && self.slow && *col == N {
                Value::Null
            } else {
                let n = part.parse::<i64>().map_err(|_| bad())?;
                if (*col == P && !(1..=47).contains(&n))
                    || (*col == S && !(0..=5).contains(&n))
                    || (*col == T && !(0..=1).contains(&n))
                    || (*col == I && !(1..=20).contains(&n))
                {
                    return Err(bad());
                }
                Value::Integer(n)
            };
            if self
                .fixed
                .iter()
                .any(|(c, allowed)| c == col && !allowed.contains(&value))
            {
                return Err(bad());
            }
            if self.exclude && *col == S && value == Value::Integer(5) {
                return Err(bad());
            }
            if *col == N
                && self.before.is_some_and(|n| {
                    value == Value::Null || value_cmp(&value, &Value::Integer(n)).is_gt()
                })
            {
                return Err(bad());
            }
            values.push(value);
        }
        Ok(values)
    }
}

fn region_values(regions: &[Region]) -> Result<(Vec<Value>, Vec<Value>), EngineError> {
    let mut regions_p = Vec::new();
    let mut regions_m = Vec::new();
    for r in regions {
        match r {
            Region::Prefecture(p) if (1..=47).contains(p) => {
                regions_p.push(Value::Integer(i64::from(*p)));
            }
            Region::Municipality(m) if m.len() == 5 && m.bytes().all(|b| b.is_ascii_digit()) => {
                regions_m.push(Value::Text(m.clone()));
            }
            _ => {
                return Err(invalid(
                    "region",
                    "prefecture must be 1..=47; municipality must be five ASCII digits",
                ));
            }
        }
    }
    Ok((regions_p, regions_m))
}

fn membership(col: &str, values: &[Value], args: &mut Vec<Value>) -> String {
    args.extend_from_slice(values);
    if values.len() == 1 {
        format!("{col} = ?")
    } else {
        format!("{col} IN ({})", vec!["?"; values.len()].join(","))
    }
}
fn value_cmp(a: &Value, b: &Value) -> std::cmp::Ordering {
    match (a, b) {
        (Value::Null, Value::Null) => std::cmp::Ordering::Equal,
        (Value::Null, _) => std::cmp::Ordering::Less,
        (_, Value::Null) => std::cmp::Ordering::Greater,
        (Value::Integer(a), Value::Integer(b)) => a.cmp(b),
        (Value::Text(a), Value::Text(b)) => a.cmp(b),
        _ => unreachable!("homogeneous index keys"),
    }
}

pub(crate) fn company_row(
    row: &Row<'_>,
    origin: DataOrigin,
    fields: &Fields,
) -> rusqlite::Result<Company> {
    let sole = row.get("is_sole_proprietor")?;
    let personal = |field: Field, col: &str| {
        if !field.is_personal_for(sole) || fields.contains(&field) {
            row.get(col)
        } else {
            Ok(None)
        }
    };
    let status = Status::from_code(row.get("status")?).ok_or(rusqlite::Error::InvalidQuery)?;
    let industry = row
        .get::<_, Option<u8>>("industry_code")?
        .map(|c| Industry::from_code(c).ok_or(rusqlite::Error::InvalidQuery))
        .transpose()?;
    Ok(Company {
        id: CompanyId(row.get("id")?),
        origin,
        corporate_number: row.get("corporate_number")?,
        name: personal(Field::Name, "name")?,
        prefecture: row.get("prefecture_code")?,
        municipality: row.get("municipality_code")?,
        industry,
        phone: personal(Field::Phone, "phone")?,
        website: personal(Field::Website, "website")?,
        owner: row.get::<_, Option<i64>>("owner_id")?.map(UserId),
        status,
        is_sole_proprietor: sole,
        source_id: row.get("source_id")?,
        source_as_of: row.get("source_as_of")?,
        created_at: row.get("created_at")?,
        erased_at: row.get("erased_at")?,
        last_activity_at: row.get("last_activity_at")?,
        next_action_at: row.get("next_action_at")?,
    })
}

pub(crate) fn company_detail(
    db: &Db,
    id: CompanyId,
    fields: &Fields,
) -> Result<Detail, EngineError> {
    let _snapshot = db.conn().unchecked_transaction().map_err(storage)?;
    let company = db
        .conn()
        .query_row("SELECT * FROM companies WHERE id=?", [id.0], |r| {
            company_row(r, db.origin(), fields)
        })
        .optional()
        .map_err(storage)?
        .ok_or(EngineError::NotFound)?;
    let mut stmt = db
        .conn()
        .prepare("SELECT * FROM contacts WHERE company_id=? ORDER BY created_at DESC,id DESC")
        .map_err(storage)?;
    let contacts = stmt
        .query_map([id.0], |r| {
            let erased_at: Option<i64> = r.get("erased_at")?;
            let personal = |field, col: &str| {
                if erased_at.is_none() && fields.contains(&field) {
                    r.get(col)
                } else {
                    Ok(None)
                }
            };
            Ok(Contact {
                id: ContactId(r.get("id")?),
                company: id,
                name: personal(Field::ContactName, "name")?,
                phone: personal(Field::ContactPhone, "phone")?,
                email: personal(Field::ContactEmail, "email")?,
                created_at: r.get("created_at")?,
                erased_at,
            })
        })
        .map_err(storage)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(storage)?;
    let mut stmt = db
        .conn()
        .prepare("SELECT * FROM activities WHERE company_id=? ORDER BY at DESC,id DESC")
        .map_err(storage)?;
    let activities = stmt
        .query_map([id.0], |r| {
            let kind =
                ActivityKind::from_code(r.get("kind")?).ok_or(rusqlite::Error::InvalidQuery)?;
            let status_after = r
                .get::<_, Option<u8>>("status_after")?
                .map(|s| Status::from_code(s).ok_or(rusqlite::Error::InvalidQuery))
                .transpose()?;
            let next_action = match r.get::<_, u8>("next_action_kind")? {
                0 => NextAction::Keep,
                1 => NextAction::Clear,
                2 => NextAction::Set(r.get("next_action_at")?),
                _ => return Err(rusqlite::Error::InvalidQuery),
            };
            Ok(Activity {
                id: ActivityId(r.get("id")?),
                company: id,
                contact: r.get::<_, Option<i64>>("contact_id")?.map(ContactId),
                kind,
                at: r.get("at")?,
                note: r.get("note")?,
                status_after,
                next_action,
                reverses: r.get::<_, Option<i64>>("reverses_id")?.map(ActivityId),
                note_erased_at: r.get("note_erased_at")?,
            })
        })
        .map_err(storage)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(storage)?;
    Ok(Detail {
        company,
        contacts,
        activities,
    })
}

#[cfg(test)]
pub(crate) mod tests;
