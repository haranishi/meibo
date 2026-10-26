//! 営業リストの型と保存・検索の土台。
mod bulk;
pub mod crm;
pub mod custom_fields;
pub mod engine;
mod erase;
pub mod houjin;
mod lists;
pub mod normalize;
pub mod public_data;
mod query;
mod schema;
mod search;
pub mod shapes;
pub mod synth;
pub mod types;
mod write;

pub use engine::{Engine, LoadReport};
pub use schema::{Durability, Integrity, OpenOptions, SpaceProbe};
pub use types::{DuplicatePolicy, ImportBatchError, ImportBatchReport};
