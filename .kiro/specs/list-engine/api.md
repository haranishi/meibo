# 公開型と操作の契約（design.md の付属資料）

時刻はすべて UNIX 秒（`i64`）。文字列の日付は `YYYY-MM-DD`。固定コードは後から振り直さない。

## 値の型
- ID: `CompanyId` `ContactId` `ActivityId` `ListId` `UserId`（`i64`）。`Industry`: 日本標準産業分類の大分類 A〜T（コード 1〜20）。
- `Status`: 未着手0・対応中1・商談2・受注3・見送り4・除外5。`ActivityKind`: 架電1・訪問2・メール3・メモ4・状態変更5・取り消し9。`DataOrigin`: 合成 / 実データ。
- `Region`: `Prefecture(1..=47)` / `Municipality(JIS 5桁)`。`NextAction`: `Keep`（そのまま）/ `Clear`（解除）/ `Set(時刻)`。

## 入力
- `CompanyInput { origin, corporate_number?, name, prefecture, municipality?, industry?, phone?, website?, owner?, status, is_sole_proprietor, source_id?, source_as_of? }`。登録は `register_company(c)`、更新は `update_company(id, c)`（同じ型）。`status` は登録時だけ指定でき、更新で現在値と違えば `StatusViaActivity` で拒否する。`origin` は保存先の種別と照合する。
- `ContactInput { company, name, phone?, email? }`
- `ActivityInput { company, contact?, kind, at, note?, status_after?, next_action, reverses? }`。`reverses` は取り消しのときだけ。
- `Filters { region?, industry?, status?, has_phone?, owner?, next_action_before?, include_excluded }`（`region` `industry` `status` `owner` は複数値）、`Sort { ShapeDefault, NextAction, LastActivityDesc }`（`ShapeDefault` は形1〜18・22、形19・20は `NextAction`、形21は `LastActivityDesc` を指定する）、`ListQuery { filters, sort, after?, limit(≤200), fields }`、`BulkChange { owner: OwnerChange, status?, include_excluded }`（`OwnerChange`: `Keep` / `Clear` / `Set(UserId)`）。

- 入力の検査: 都道府県は1〜47、市区町村はJIS 5桁の数字、日付は `YYYY-MM-DD`、`limit` は1〜200。名称が空は `EmptyName`、ほかの違反は `InvalidInput { field, reason }`。前後の空白は除き、空の電話・ホームページ・メールは未指定として扱う。保存する文字列（企業の名称・電話・ホームページ・出典の識別子、担当者の氏名・電話・メール、活動のメモ、リストの名前）に NUL（U+0000）を含む入力は `InvalidInput { field, .. }` で拒否する（全文索引が NUL で解析を打ち切り、検索と件数が食い違うため。検索語の NUL は、どの社名にも当たらない語として扱う）。
- 時刻の検査: 活動の日時 `at` と、次回対応日の設定 `Set(t)` は0以上。負は `InvalidInput { field: "at" \| "next_action", .. }` で拒否する（最終活動日の「なし」を0で持つので、負の日時と区別できなくなる）。
- 空きの確認（10.3）: 書き込みはすべて、確定を始める前に、書き込む量から見積もった必要量（企業・担当者・活動は入力の文字列の長さ、リストと一括変更は人数、ほかは一定の余裕）を保存先の空きと照らし、足りなければ何も書かずに `InsufficientSpace` を返す。大量追加と `compact` は専用の見積り。
- 名前の入口（3.6）: 画面・API・MCP が文字列から項目・並び・絞り込みを選ぶための入口を `types` が持つ。`Field::from_name` `Sort::from_name` `FilterKey::from_name`（名前は小文字の snake_case）と、絞り込みの項目と演算の組を検査する `check_filter(key, op)`。許可は 地域/業種/状態/担当営業 = `eq` `in`、電話の有無 = `eq`、次回対応日 = `lte`。許可されていない名前や演算は `FieldNotAllowed { allowed }`（`allowed` は使える名前の完全な一覧、演算は `key:op` の形）で拒否する。列挙型で受ける公開操作は、型として表せない値を受け付けない。

## 出力
- `Page { rows, next?, slow }`、`Counted { n, slow }`、`SearchPage { rows, next?, n? }`（`n` は先頭ページで `SearchOptions.with_count` を真にしたときだけ。先頭50社と件数を別々の目標で測れるようにする）、`Detail { company, contacts, activities（新しい順）}`、`VerifyReport { integrity_ok, counts_match, folds_match }`。
- `Fields`: 個人情報の項目は、明示して選んだときだけ入る。

## 活動の適用規則（`record_activity`）
1. 1回の確定の中で、活動の追記・企業の更新・件数表の更新を行う。適用は記録順（id）。
2. 状態は `status_after` があればその値、なければ据え置き。次回対応日は `next_action` のとおり（そのまま・解除・設定）。
3. 最終活動日は、取り消されていない架電・訪問・メール・メモの日時の最大（なければ0）。状態変更と取り消しは数えない。過去の日時の追記でも最大で決まる。
4. 取り消しは、同じ企業の、取り消しでも取り消し済みでもない活動だけを指せる。それ以外は `InvalidCancel`。取り消しの記録後に、残る活動を記録順に畳み直す。
5. 企業が除外のとき、`status_after` が除外以外の活動と一括変更は `ExcludedCompany`（全体を拒否）。状態を変えない活動は記録できる。`release_exclusion` は状態変更の活動として記録する。
6. `upsert_company` は活動を作らない。登録時の状態が基準になる。

## 操作表（期待結果）
| # | 操作 | 状態 | 最終活動日 | 次回対応日 |
|--:|---|---|---|---|
| 1 | 登録（未着手） | 未着手 | 0 | なし |
| 2 | 架電 at=100, 対応中, Set(500) | 対応中 | 100 | 500 |
| 3 | メモ at=50, 指定なし | 対応中 | 100 | 500 |
| 4 | 状態変更 商談, Clear | 商談 | 100 | なし |
| 5 | 2を取り消す | 商談 | 50 | なし |
| 6 | 4を取り消す | 未着手 | 50 | なし |
| 7 | 5の取り消しを取り消す / 2を再び取り消す / 他企業の活動を取り消す | いずれも `InvalidCancel` | — | — |
| 8 | 除外の企業へ 状態変更 | `ExcludedCompany`（記録しない） | — | — |
| 9 | 除外の企業へ メモ（状態の指定なし） | 記録できる（状態は除外のまま） | — | — |

## 消去（`erase_*`）
- **担当者**: 氏名・電話・メールをNULLにし、消去日時を入れる。活動の `contact` は残す。`wipe_notes` なら、紐づく活動の `note` をNULLにし、`note_erased_at`（NULLから値への1回だけ）を入れる。すでに消去済みの担当者へ `wipe_notes` つきで求められたときも、残っているメモを消す（担当者の消去日時は最初のまま。消すものが残っていなければ何も変えない）。
- **個人事業の企業**: 状態変更の活動（除外、次回対応日は解除）を1件追記してから、名称・名称キーを「(消去済み)」、法人番号・電話・ホームページ・担当営業・業種・出典をNULL、所在地を0にし、担当者を上のとおり消去し、全活動の `note` を消す。残るのは、内部ID・個人事業の印・登録日・消去日時と、活動の日時・種類・状態と次回対応日の指定。全文索引は旧キーを `'delete'` して「(消去済み)」を入れ、件数表を差分更新する。
- 変えられる列と手順は、設計の判断6（`erase_gate`）のとおり。物理的な消去は `compact`（判断7）。
