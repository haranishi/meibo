# タスク

- [x] 1. 仕様を作る（要件・設計・タスク）
- [x] 2. 往復を数える試験を足し、改修前の回数を記録する
- [x] 3. 第1段
  - [x] 3.1 準備結果の保持、読み取り1往復、まとめ読み
  - [x] 3.2 準備確認を1往復にする
  - [x] 3.3 件数のまとめを2往復以内にする
  - [x] 3.4 固定の応答を gate の外へ出し、部品をブラウザに保存させる
- [x] 4. 第2段
  - [x] 4.1 書き込み禁止の範囲（fence）
  - [x] 4.2 速い経路と最終操作時刻の60秒間隔
  - [x] 4.3 keep-alive の故障試験を先に作り、通ったら読み取りの接続再利用を有効にする
- [ ] 5. 検証する（check-all、check-cloud、実装と別系統のレビュー）
- [x] 6. 合成データ版を配備し直し、同じ手順で測って記録する

## 検証の記録

2026-11-27（実装担当・手元の sqld と試験用の代理サーバーで確認。配備と提供者での測定は未実施）
- 往復回数は `crates/app/tests/remote_round_trips.rs` で数えた（control と tenant の合計）。改修前は `git archive 7321ed8` を一時領域へ展開し、同じ試験ファイルだけを足して測った。

| 要求 | 改修前 | 改修後 |
|---|---:|---:|
| POST /auth/login | 74 | 39 |
| GET /（ログイン後の最初） | 59 | 21 |
| GET /api/summary | 78 | 4 |
| GET /api/summary（2回目） | 78 | 3 |
| GET /api/companies（一覧） | 40 | 8 |
| 検索 | 40 | 8 |
| 取引先詳細 | 44 | 12 |
| work-items | 42 | 10 |
| opportunities | 44 | 12 |
| 活動の記録（POST） | 58 | 26 |
| GET /login | 22 | 0 |
| GET /health | 22 | 6 |

- 4.3: `crates/db/tests/remote_keepalive.rs` の3故障（応答前の切断・待機中の切断・受信直後の切断）で、同じ往復が保存先へ2回届かないこと、書き込みと取引の各段階が毎回新しい接続であることを確認してから `REUSE_READ_CONNECTIONS=true` にした。false でも同じ試験が通る。
- 要件3.6（session cookie のない要求の429）は token bucket と単体試験で実装した。
- 初回: `cargo xtask check-cloud` 594件成功、`cargo xtask check-all` 411件成功（どちらも失敗0）。どちらも fmt・clippy・依存検査を含めて成功。Mac の一時領域は symlink を含むため、TMPDIR に実体のある SSD の場所を指定した。
- 5 の別系統レビューと 6 の配備・測定は未実施。

2026-11-27 レビュー指摘の修正（要件3.6・4.4・6.1・6.2の更新に合わせた）
- 速い経路は、通信が原因でない失敗（500、tenant を開けない等）を排他つきの経路でやり直す。fence 中は結果不明の印を付けない。tenant の識別不一致で固まらず、排他つきの経路と同じ応答になる試験を追加。
- 速い経路の通信失敗は gate を取らず、応答後に serve ループを抜けてプロセスが非0で終わる。
- 429: 認証に成功した session の digest を64件まで覚え、その cookie は枠を使わない。偽の cookie が429になり、確認済み session は枠が尽きても通る試験を追加。時計が戻った後に枠が回復する単体試験を追加。
- 接続の再利用は fence 中の往復だけにした。keep-alive 試験で「fence の外では毎回新しい接続」も確認。
- `/oauth/activity-return` は no-store のまま。`/api/activity-sync` は速い経路に入れない。
- 状態0〜5・電話の有無・次回予定（過去・未来・なし）・除外を含む合成データ400社で、summary の1文と条件ごとの count が一致する試験を追加（`crates/core/tests/remote_domain.rs`）。
- 往復回数は上の表と同じ値を測り直して確認した。`check-cloud` 597件成功・`check-all` 411件成功（失敗0、無視は4と3）。
- 2026-11-27: 別の担当が同じ環境で `check-cloud`（597件成功・失敗0・無視4）と `check-all`（411件成功・失敗0・無視3）を実行し直し、往復回数の表の値も確認した。
- 2026-11-27: レビューは実装とは別の Opus セッション（読み取りのみ）で指摘6件、確認者の指摘3件を加えて8項目を修正した。他社モデルのレビューは利用上限で未実施（タスク5は未完了）。
- 2026-11-27: 合成データ版を配備し直した（設定は不変・`/health` 200・未認証401・ログに鍵らしき値なし）。以下は自宅回線から測った中央値（ミリ秒・合成1,000社）。改修前は2026-11-26の同じ手順の値。

| 操作 | 改修前（回数） | 改修後（回数） |
|---|---:|---:|
| ログイン画面 | 684（3） | 39（3） |
| ログイン | 2303（1） | 1076（6） |
| 画面の本体 | 2004（1） | 49（6） |
| 画面の部品1個 | 1094（3） | 84（3） |
| 件数のまとめ | 2212（10） | 59（20） |
| 一覧の先頭25社 | 1271（10） | 201（20） |
| 検索（3文字以上／2文字） | 1306／1310（10） | 238／208（20） |
| 取引先を開く | 1344（10） | 236（20） |
| 予定／商談の取得 | 1347／1312（10） | 201／234（20） |
| 活動の記録の保存 | 1768（10） | 899（20） |

- 配備直後の最初の1回はログイン2.4秒・画面の本体1.9秒（組織DBを初めて開くため）。停止状態からの起動直後は未測定。
- 保存の目標（0.3〜0.4秒）には届かなかった。書き込みは往復ごとに新しい接続を使う今の形（要件6.2）のためとみられる。

### 2026-11-27 追加レビューの修正

| 番号 | 修正箇所と判定 | 追加・更新した回帰試験 |
|---|---|---|
| 1 | `db/src/remote.rs` と `app/src/hosted/http.rs`。接続の破棄中に応答を失っても fence に記録し、排他取得前に停止する | `failure_lasts_until_outer_fence_ends`、`lost_rollback_stops_without_gate` |
| 2 | `db/src/remote.rs`。単独の SELECT だけを Describe とまとめ読みの Check に送る。実行時は readonly も確認する | `fence_checks_sql_before_sending` |
| 3 | `app/src/hosted/http.rs`。公開フォームの GET・POST とログインには匿名枠を適用する | `public_routes_use_anonymous_quota` |
| 4 | 同ファイル。配備時のログイン検査に合わせて `/login` を no-store に戻し、304の確認には固定部品 `/login.css` を使う | `gate_free_reads_write_nothing_and_fall_back_for_writes`（更新） |
| 5 | 同ファイル。ログイン成功時に session を記録し、直後の認証要求を匿名枠から免除する | `new_cookie_skips_empty_bucket`、`unverified_cookies_are_limited_and_verified_sessions_are_not`（更新） |
| 6 | 同ファイル。結果不明と接続失効を429の前に判定する | `lost_rollback_stops_without_gate`、`poison_precedes_quota` |
| 7 | `db/src/remote/state.rs`。範囲外で始めた取引と範囲外の操作を挟んだ取引は、fence 中の確定時に1回だけ取り消す | `fenced_commit_rolls_back_prior_writes` |
| 8 | `db/src/remote.rs` と `db/src/remote/state.rs`。取引中に DDL を実行したら、終了まで列構成の保持を休止する | `ddl_describes_expire_on_rollback` |
| 10 | `core/src/query.rs`。件数を500条件ずつ集計する。試験は2001条件の順序と、準備済みの500条件が1往復で済むことを照合する | `counts_chunk_at_500` |
| 11 | `app/src/hosted/http.rs`。固定部品は `If-None-Match` のワイルドカード、一覧、弱いタグに対応する | `if_none_match_uses_weak_comparison` |

パスは `crates/` 基準。準備確認・session 認証・組織DB照合・通常の画面と検索の読み取り SQL はソース上で単独の SELECT と確認した。取引開始時の外部キー設定は従来どおり新しい接続で行う。
試験ファイルは `db/tests/remote_keepalive.rs`、`db/tests/remote_transactions.rs`、`app/src/hosted/remote/runtime_tests.rs`、`app/tests/remote_hosted.rs`、`core/tests/remote_domain.rs`。DB と HTTP の単体試験は実装ファイル内に置いた。`core/src/engine.rs` の公開 API の説明にも500件の区切りを記載した。

9の初回 Describe は合意どおり維持した。変更した Rust ファイルの `rustfmt --check` と `git diff --check` は成功。今回のビルドと試験は呼び出し側で実行する。上の測定値と合格件数は前回の記録。
