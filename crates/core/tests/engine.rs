//! 公開Engineを通した保存・操作・検索の結合試験。

#[cfg(feature = "cloud")]
use meibo_db as sql;
#[cfg(not(feature = "cloud"))]
use rusqlite as sql;

#[path = "engine/erase.rs"]
mod erase;
#[path = "engine/import.rs"]
mod import;
#[path = "engine/injection.rs"]
mod injection;
#[path = "engine/lists.rs"]
mod lists;
#[path = "engine/ops.rs"]
mod ops;
#[path = "engine/origin.rs"]
mod origin;
#[path = "engine/paging.rs"]
mod paging;
#[path = "engine/plan.rs"]
mod plan;
#[path = "engine/storage.rs"]
mod storage;
#[path = "engine/support.rs"]
mod support;
#[path = "engine/synth.rs"]
mod synth;
#[path = "engine/visit.rs"]
mod visit;
