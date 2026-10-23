# list-engine 設計

## Overview
営業リストの企業を数百万社持っても、一覧・件数・社名検索・活動の記録が速い土台（`core` クレート）を作る。保存はSQLite 1ファイル。速さは索引の設計で決まるため、速さを宣言する22の形（`shapes.md`）ごとに索引を固定し、実行計画と測定の両方で守る。判断の根拠と実測は `research.md`、公開型と操作の期待結果は `api.md`。
**目標**: 要件1〜10の実現。画面・取込・APIが同じ検索口と追加口を呼べること。**非目標**: 画面、CSV、API・MCP、認証、利用者定義の項目、企業の統合、実データの取込。

## Boundary Commitments
### This Spec Owns
型と保存形式（版・移行・複製・整合性）、速さを宣言する形の目録と実行、件数、社名検索、活動の書き込みと状態の畳み込み、リストと一括操作、個人情報の印と消去、合成データ、速さの門。
### Out of Boundary
HTTP・CLIの引数・画面、CSVの解釈、名寄せの細かい規則（csv-import-export）、権利の判断（`docs/legal`）。
### Allowed Dependencies
`rusqlite`（bundled・backup）、`unicode-normalization`、`thiserror`、`fs4`。xtask は `serde`・`serde_json`。試験は `proptest`・`tempfile`。ネットワークの部品は `deny.toml` で禁止。
### Revalidation Triggers
速さを宣言する形の変更、保存形式の版、個人情報の印の変更、目標値の変更、種別（合成・実データ）の規則の変更は、下流の仕様の再検証を要する。

## Architecture
依存は `types` → `houjin`・`normalize`・`shapes` → `schema` → `write`・`query`・`search`・`bulk` → `engine` の向きだけ。`synth` は `bulk`、`bench` は `engine` だけを使い、`core` は `xtask` を知らない。
**主な判断**
1. **形と索引**: 形ごとに索引を固定（`INDEXED BY`）し、`ORDER BY` は索引の列を省かず並べる。`INDEXED BY` は使えない索引でも全走査として通るので、守るのは実行計画の文字列の検査と測定である。
2. **状態は活動の畳み込み**: 状態を変える操作はすべて活動（一括変更も1社1件の状態変更の活動）。状態・次回対応日・最終活動日は、取り消されていない活動を記録順（id）に適用した値で、基準は登録時の状態・次回対応日なし・最終活動日0。追加は差分、取り消しだけ畳み直す。適用規則と操作表は `api.md`。除外から戻すのは `release_exclusion` だけ。
3. **件数表**: 状態×電話の12行。登録・電話の変更・状態の変更・消去・大量追加で、変更前後の組の差分を同じ確定で反映する（`apply_count_delta` の1か所）。全件・状態別・電話別（除外を引く）はここから出し、`verify` が再集計と照合する。
4. **社名**: 正規化キー（NFKC・かなをカタカナへ・小文字）。3文字以上は全文索引（trigram）、2文字以下は `instr` 走査。全文索引は外部内容表で、登録・改名・消去は同じ確定の中で旧キーの `'delete'` →新キーの挿入、大量追加は終了時に `'rebuild'`。
5. **耐久性**: WAL＋NORMAL が既定で、FULL を選べる。待ち5秒は10msの保証ではなく、読み取り中の確定時間は門が測る。
6. **追記専用と消去**: 活動の更新は、恒久の1行表 `erase_gate` に行があるときの、`note` の NULL 化と `note_erased_at`（NULLから値への1回だけ）に限って許す。消去の処理が同じ確定の中で行を入れ、消し、行を消す（確定後は誰からも見えない）。消す列と残す列は `api.md`。
7. **`compact`**: 全文索引の `'rebuild'` → `VACUUM`（SQLite公式の記述で最大2倍の空きが要るので、保存先の2倍を確かめる）→ `wal_checkpoint(TRUNCATE)`。チェックポイントが忙しければ `CompactIncomplete` を返す。移行前の複製には消去が及ばないので文書に書く。
**Technology Stack**: Rust 2024（MSRV 1.88）／rusqlite 0.40.2＋同梱SQLite 3.53.2／unicode-normalization 0.1.26／fs4 0.13.1／thiserror 2。

## File Structure Plan
- `Cargo.toml` `rust-toolchain.toml` `deny.toml` `REUSE.toml` `LICENSE-*` `.gitignore`: 作業場の土台
- `crates/core/src/`: `lib.rs` `types.rs`（ID・状態・種別・印・誤り）`houjin.rs` `normalize.rs` `schema.rs`（表・索引・移行・複製・整合性・空き）`shapes.rs`（22形の目録と分類）`query.rs` `search.rs` `write.rs`（企業・担当者・活動・リスト・消去・件数表）`bulk.rs` `synth.rs` `engine.rs`
- `crates/core/tests/`: 要件ごとの試験（`houjin` `paging` `plan_guard` `activity_fold` `crash` `migration` `injection` `origin` `erase`）
- `crates/xtask/src/`: `main.rs` `bench.rs` `docs_check.rs` `mutate.rs` `links.rs` `trace.rs`（kintaiから移植）
- `docs/benchmarks.md` `docs/architecture.md` `docs/legal/rights-check.md`、`.agent-harness/contracts/meibo-dev-workflow-v1.yaml`

## Requirements Traceability
| 要件 | 担う部品 |
|---|---|
| 1.1〜1.8 | `houjin` `normalize` `write` `schema`（UNIQUEと更新時の検証） |
| 2.1〜2.3 | `write` `query`（詳細は活動の新しい順） |
| 3.1〜3.5 | `shapes` `query` `schema`（索引K1〜K10）、計画の検査、速さの門 |
| 3.6 | `types`（列挙型と、文字列から選ぶ入口 `from_name` `check_filter`） |
| 4.1〜4.7 | `normalize` `search` |
| 5.1〜5.8 | `write`（畳み込み・取り消し）`schema`（追記専用・WAL・複製）、文書 |
| 6.1〜6.5 | `write` `query`（除外の既定と保護） |
| 7.1〜7.5 | `types`（印）`write`（消去）`engine`（通信なし）、`deny.toml` |
| 8.1〜8.3 | `synth` `houjin` `bulk` `engine`（種別の申告と照合） |
| 9.1〜9.5 | `xtask bench` `docs_check` |
| 10.1〜10.5 | `schema` `engine` `query`（値は束縛のみ） |

## Components and Interfaces
```rust
pub struct Engine; // create(path, origin, opt) / open(path, opt)。版・種別・整合性を確かめる。書き込みは `&mut self`、読み取りは `&self`
fn register_company(&mut self, c: CompanyInput) -> Result<CompanyId>;  // origin を申告。状態は登録時だけ指定できる
fn update_company(&mut self, id: CompanyId, c: CompanyInput) -> Result<()>;  // 状態は変えられない
fn add_contact(&mut self, c: ContactInput) -> Result<ContactId>;        // 2.1
fn record_activity(&mut self, a: ActivityInput) -> Result<ActivityId>;  // 状態変更・取り消しも同じ口
fn release_exclusion(&mut self, id: CompanyId, to: Status) -> Result<ActivityId>;   // 6.5
fn company_detail(&self, id: CompanyId, f: &Fields) -> Result<Detail>;          // 2.2
fn list(&self, q: &ListQuery) -> Result<Page>;     // Page { rows, next: Option<Cursor>, slow: bool }
fn count(&self, f: &Filters) -> Result<Counted>;   // Counted { n: u64, slow: bool }
fn search_name(&self, term: &str, o: &SearchOptions) -> Result<SearchPage>;     // fields・除外・続き・件数の要否
fn create_list(&mut self, name: &str, from: &Filters, limit: u32) -> Result<ListId>;
fn delete_list(&mut self, id: ListId) -> Result<()>;                                // 6.3
fn bulk_update(&mut self, list: ListId, ch: BulkChange) -> Result<u64>;             // 6.2、6.5
fn erase_contact(&mut self, id: ContactId, wipe_notes: bool) -> Result<()>;         // 7.3
fn erase_company(&mut self, id: CompanyId) -> Result<()>;  // 個人事業の印があるときだけ。7.5
fn compact(&mut self) -> Result<()>;                        // 判断7
fn verify(&self) -> Result<VerifyReport>;               // 整合性と件数表の再集計
```
絞り込み・並べ替え・項目は列挙型で受け、文字列から作らない（3.6、10.5）。個人情報の項目は `Fields` で選ばれたときだけ含める（7.1）。誤りは `EngineError`（`InvalidCorporateNumber` `DuplicateCorporateNumber{existing}` `EmptyName` `InvalidInput{field,reason}` `NotFound` `ForeignContact` `FieldNotAllowed{allowed}` `InvalidSearchTerm` `ExcludedCompany` `InvalidCancel` `StatusViaActivity` `OriginMismatch` `InsufficientSpace` `NeedsMigration` `UnsupportedVersion{found,supported}` `Corrupt` `CompactIncomplete` `Storage`）。書き込みは1回の確定（`BEGIN IMMEDIATE`）にまとめ、失敗で何も残さない。

## Data Models
`meta` `companies` `contacts` `activities`（追記専用。状態変更後の値と次回対応日の指定、取り消し先を持つ）`lists` `list_members` `counts` `erase_gate` `name_fts`。法人番号と電話は文字列。`has_phone` は電話から導く生成列。最終活動日の「なし」は0。
| 索引 | 列（最後はすべて id） |
|---|---|
| K1 / K2 | 都道府県, 業種, 状態, 電話 / 都道府県, 状態, 電話 |
| K3 / K4 | 市区町村, 業種, 状態, 電話 / 市区町村, 状態, 電話 |
| K5 / K6 / K7 | 業種, 状態, 電話 / 状態, 電話 / 電話, 状態 |
| K8 / K9 / K10 | 担当営業, 次回対応日, 状態（担当営業ありだけ）/ 次回対応日, 状態（次回対応日ありだけ）/ 最終活動日, 状態 |
各形の索引・並び・計画の期待・複数値・NULL・カーソルは `shapes.md`。形は `shapes::classify` が分類し、該当しなければ `slow: true`。

## Testing Strategy
- 計画の検査: 10万社の保存先で、22形の先頭ページと深いページの `EXPLAIN QUERY PLAN` を `shapes.md` の期待と照合する（3.1〜3.4）。複数値の例も含む。
- 参照実装との差分: メモリ上の単純な実装（試験内、Rust）と、乱数の絞り込み・社名検索（1〜2文字、全角半角、かな）・ページ送りを突き合わせる（3.3、4.x）。
- 性質試験: 検査用数字、畳み込み（差分＝畳み直し、唯一の活動の取り消し、二重取り消しの拒否）、件数表＝再集計、全文索引＝社名キー（改名・消去・失敗後）（1.1、5.4）。
- 障害: 失敗の注入で原子性（5.2、6.2）、子プロセスの強制終了（5.7）、移行の失敗と復元（10.2）、壊れた複製（10.4）、空きの注入（10.3）。
- 個人情報: 既定で含めない。`compact` 後に、保存先とWALのファイルに消去した文字列が残らず、消去した社名の検索が0件（7.x）。
- 変異試験: 純粋な論理（`houjin` `normalize` `shapes` 畳み込み）の生き残り0。
- 速さの門: 445万社×疎・密、22形×代表値、複数値、深いページ、代表的な遅い条件、読み取りを続ける別プロセスの下の活動記録（9.x）。

## Open Questions / Risks
索引10本＋UNIQUE 1本・キャッシュ既定128MiB。社名検索の除外の適用が件数の目標を超えるときは、件数表から引く方式へ変える。FULL の確定時間（macOS）は未測定で、門で測って文書化する。
