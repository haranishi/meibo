//! 組織DBに保存する共有架電条件。既存の固定リストと異なり、開くたびに対象を検索する。
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    Engine,
    query::{self, invalid, storage},
    schema::WRITE_BASE,
    sql::{Connection, OptionalExtension, params},
    types::{
        CompanyId, Cursor, Detail, EngineError, Filters, ListQuery, Region, SearchOptions,
        SearchPage, Sort, Status,
    },
};

/// 一覧のURLと保存済みリストで共有する条件。カーソルと表示件数は含まない。
pub type CallingConditions = BTreeMap<String, String>;

struct Scope<'a> {
    filters: Filters,
    sort: Sort,
    term: &'a str,
    empty: bool,
}

impl<'a> Scope<'a> {
    fn parse(conditions: &'a CallingConditions, now: i64) -> Result<Self, EngineError> {
        let bad = || invalid("conditions", "invalid calling list conditions");
        if conditions.keys().any(|k| {
            !matches!(
                k.as_str(),
                "view" | "q" | "sort" | "status" | "prefecture" | "has_phone"
            )
        }) {
            return Err(bad());
        }
        let get = |name: &str| conditions.get(name).map(String::as_str);
        let mut filters = Filters::default();
        match get("view").unwrap_or("all") {
            "all" => {}
            "untouched" => filters.status = vec![Status::Untouched],
            "negotiating" => filters.status = vec![Status::Negotiating],
            "has_phone" => filters.has_phone = Some(true),
            "scheduled" => filters.next_action_before = Some(i64::MAX),
            "due" => filters.next_action_before = Some(now),
            _ => return Err(bad()),
        }
        let mut empty = false;
        if let Some(value) = get("status") {
            let status = value
                .parse::<u8>()
                .ok()
                .filter(|v| *v <= 4)
                .and_then(Status::from_code)
                .ok_or_else(bad)?;
            empty = !filters.status.is_empty() && !filters.status.contains(&status);
            filters.status = vec![status];
        }
        if let Some(value) = get("prefecture") {
            let code = value
                .parse::<u8>()
                .ok()
                .filter(|v| (1..=47).contains(v))
                .ok_or_else(bad)?;
            filters.region.push(Region::Prefecture(code));
        }
        if let Some(value) = get("has_phone") {
            let has_phone = value.parse::<bool>().map_err(|_| bad())?;
            empty |= filters.has_phone.is_some_and(|preset| preset != has_phone);
            filters.has_phone = Some(has_phone);
        }
        let sort = match get("sort").unwrap_or("default") {
            "default" if filters.next_action_before.is_none() => Sort::ShapeDefault,
            "default" | "next_action" => Sort::NextAction,
            "last_activity" => Sort::LastActivityDesc,
            _ => return Err(bad()),
        };
        Ok(Self {
            filters,
            sort,
            term: get("q").unwrap_or("").trim(),
            empty,
        })
    }
}

/// 名前・条件と楽観的同時更新用の版。IDは同じ組織のDB内で一意。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallingList {
    /// 削除後も再利用しない識別子。
    pub id: u32,
    /// 更新成功ごとに増える版。
    pub revision: u32,
    /// 画面に表示する名称。
    pub name: String,
    /// 共有する条件と並び順。
    pub conditions: CallingConditions,
}

// meta は全ての旧DBにあり、Tursoのsnapshotと復元の対象にも含まれる。
const CATALOG_KEY: &str = "calling_lists_v1";
const MAX_LISTS: usize = 100;
const MAX_CATALOG_BYTES: usize = 256 * 1024;

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    last_id: u32,
    lists: Vec<CallingList>,
}

fn validate_definition(name: &str, conditions: &CallingConditions) -> Result<(), EngineError> {
    if name.trim().is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
        return Err(invalid("name", "invalid calling list name"));
    }
    let scope = Scope::parse(conditions, 0)?;
    if let Some(q) = conditions.get("q") {
        if q.chars().count() > 200 || q.chars().any(char::is_control) {
            return Err(invalid("q", "invalid calling list query"));
        }
        if !scope.term.is_empty() {
            crate::normalize::search_term(scope.term)?;
        }
    }
    Ok(())
}

fn catalog(conn: &Connection) -> Result<Catalog, EngineError> {
    let raw: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key=?1", [CATALOG_KEY], |r| {
            r.get(0)
        })
        .optional()
        .map_err(storage)?;
    let Some(raw) = raw else {
        return Ok(Catalog::default());
    };
    if raw.len() > MAX_CATALOG_BYTES {
        return Err(EngineError::Corrupt);
    }
    let data: Catalog = serde_json::from_str(&raw).map_err(|_| EngineError::Corrupt)?;
    // IDs are appended monotonically. Deletion preserves their order.
    if data.lists.len() > MAX_LISTS
        || data.lists.windows(2).any(|w| w[0].id >= w[1].id)
        || data.lists.iter().any(|l| {
            l.id == 0
                || l.id > data.last_id
                || l.revision == 0
                || validate_definition(&l.name, &l.conditions).is_err()
        })
    {
        return Err(EngineError::Corrupt);
    }
    Ok(data)
}

impl Engine {
    /// 一覧・架電の全件を同じ条件で検索し、先頭ページに総件数を返す。
    /// # Errors
    /// 不正な条件・カーソル・保存先の読取失敗を返す。
    pub fn calling_page(
        &self,
        conditions: &CallingConditions,
        after: Option<Cursor>,
        limit: u32,
        now: i64,
    ) -> Result<SearchPage, EngineError> {
        query::validate_limit(limit)?;
        let scope = Scope::parse(conditions, now)?;
        if scope.empty {
            if !scope.term.is_empty() {
                crate::normalize::search_term(scope.term)?;
            }
            if after.is_some() {
                return Err(invalid("after", "empty conditions have no cursor"));
            }
            return Ok(SearchPage {
                rows: vec![],
                next: None,
                n: Some(0),
            });
        }
        if !scope.term.is_empty()
            && scope.filters == Filters::default()
            && scope.sort == Sort::ShapeDefault
        {
            return self.search_name(
                scope.term,
                &SearchOptions {
                    after,
                    limit,
                    fields: vec![],
                    include_excluded: false,
                    with_count: true,
                },
            );
        }
        query::calling_page(
            &self.db,
            &ListQuery {
                filters: scope.filters,
                sort: scope.sort,
                after,
                limit,
                fields: vec![],
            },
            scope.term,
        )
    }

    /// キューに入っていた企業が今も同じ条件なら、既定の個人情報マスクで詳細を返す。
    /// # Errors
    /// 不正な条件や保存先の読取失敗を返す。
    pub fn call_candidate(
        &self,
        id: CompanyId,
        conditions: &CallingConditions,
        now: i64,
    ) -> Result<Option<Detail>, EngineError> {
        let scope = Scope::parse(conditions, now)?;
        if scope.empty {
            return Ok(None);
        }
        query::call_candidate(&self.db, id, &scope.filters, scope.term)
    }

    /// 組織DBに保存した架電条件を返す。未作成のDBは空の一覧を返す。
    /// # Errors
    /// 保存先の読取失敗や保存形式の破損を返す。
    pub fn calling_lists(&self) -> Result<Vec<CallingList>, EngineError> {
        Ok(catalog(self.db.conn())?.lists)
    }

    fn edit_calling_catalog<T>(
        &mut self,
        edit: impl FnOnce(&mut Catalog) -> Result<T, EngineError>,
    ) -> Result<T, EngineError> {
        self.db.write(WRITE_BASE + MAX_CATALOG_BYTES as u64 * 4, |tx| {
            let mut data = catalog(tx)?;
            let result = edit(&mut data)?;
            let raw = serde_json::to_string(&data).map_err(|_| EngineError::Corrupt)?;
            if raw.len() > MAX_CATALOG_BYTES { return Err(invalid("lists", "calling list storage limit reached")); }
            tx.execute("INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![CATALOG_KEY, raw]).map_err(storage)?;
            Ok(result)
        })
    }

    /// 現在の一覧条件を名前付きで作成するか、IDと版が一致する定義を更新する。
    /// # Errors
    /// 入力不正・上限超過・更新競合・保存失敗を返す。
    pub fn save_calling_list(
        &mut self,
        existing: Option<(u32, u32)>,
        name: &str,
        conditions: CallingConditions,
    ) -> Result<CallingList, EngineError> {
        validate_definition(name, &conditions)?;
        let name = name.trim();
        self.edit_calling_catalog(|data| {
            if let Some((id, expected)) = existing {
                let index = data.position(id, expected)?;
                let item = &mut data.lists[index];
                item.revision = item.revision.checked_add(1).ok_or(EngineError::Corrupt)?;
                name.clone_into(&mut item.name);
                item.conditions = conditions;
                return Ok(item.clone());
            }
            if data.lists.len() >= MAX_LISTS {
                return Err(invalid("lists", "calling list limit reached"));
            }
            data.last_id = data.last_id.checked_add(1).ok_or(EngineError::Corrupt)?;
            let item = CallingList {
                id: data.last_id,
                revision: 1,
                name: name.to_owned(),
                conditions,
            };
            data.lists.push(item.clone());
            Ok(item)
        })
    }

    /// 指定した版の架電リストを削除する。
    /// # Errors
    /// 存在しないID・更新競合・保存失敗を返す。
    pub fn delete_calling_list(&mut self, id: u32, expected: u32) -> Result<(), EngineError> {
        self.edit_calling_catalog(|data| {
            let index = data.position(id, expected)?;
            data.lists.remove(index);
            Ok(())
        })
    }
}

impl Catalog {
    fn position(&self, id: u32, expected: u32) -> Result<usize, EngineError> {
        let index = self
            .lists
            .iter()
            .position(|item| item.id == id)
            .ok_or_else(|| {
                if id > 0 && id <= self.last_id {
                    EngineError::RevisionConflict
                } else {
                    EngineError::NotFound
                }
            })?;
        if self.lists[index].revision != expected {
            return Err(EngineError::RevisionConflict);
        }
        Ok(index)
    }
}
