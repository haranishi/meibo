# Design: 遠隔保存の応答を速くする

## 方針
遅さは往復回数×1回の重さ。回数を減らし（第1段）、読み取りだけ排他の書き込みと接続の張り直しを省く（第2段）。
書き込みの経路は「安全側の今の形」を残す。

## 第1段（安全性は不変）
| 変更 | 内容 | 場所 |
|---|---|---|
| 準備結果の保持 | `RemoteConnection` ごとに、成功した Describe をSQL本文で保持（件数上限つき）。接続を作り直せば消える | `crates/db/src/remote.rs` |
| 読み取り1往復 | Describe が readonly かつ手元の字句検査で SELECT（WITH…SELECT を含み PRAGMA は除く）の自動確定は、baton なしの `[Execute, Close]`。外部キー設定は書き込み側だけ | `remote/{state,sql}.rs` |
| まとめて読む | 読み取り専用の複数文を1 pipeline で送り、全結果を検査する API。手元版は順に実行 | `crates/db` |
| 準備確認 | `validate_control` の metadata 読み取りと schema 照合を1 pipeline にする。確認項目は同じ | `hosted/remote/store.rs` |
| 件数 | 通常の件数のまとめは1文。多数の条件を指定した場合は列数の上限を避けるため500件ずつに分け、元の順序で `count` と同じ値を返す | `crates/core`, `server/api.rs` |
| 固定の応答 | uncertain/poisoned の確認後、`/login`・公開の固定部品・OAuth戻り先・受付フォームの固定部品は gate の外で返す。`/health` は変えない | `hosted/http.rs` |
| 部品の保存 | 公開の固定部品には内容のハッシュを ETag として付け、`no-cache` で再検証する。ログイン画面、OAuth の戻り先、データを含む応答には `no-store` を付ける | `hosted/{assets,public_entry,http}.rs` |

## 第2段（承認済み）
### 書き込み禁止の範囲（read fence）
- fence 中の業務 SQL は、単独の SELECT と手元で判定できる文だけを Describe（まとめ読みの `Check` を含む）に送る。実行には保存先の readonly 判定も必要。書き込みの拒否を fence に記録する。
- fence 内で始めた DEFERRED の読み取り取引は確定できる。範囲外で始めた取引や途中で範囲外の操作を挟んだ取引の COMMIT は、ROLLBACK を1回行ってから拒否する。`execute_batch` と IMMEDIATE/EXCLUSIVE の開始も拒否する。
- 拒否は接続を失効させない。結果不明の分類（`SystemIoFailure` など）を使わない。
- control と、その要求で使う全 tenant 接続に掛ける。

### 速い経路
1. 遠隔保存の GET と `read_only_post` が対象。
2. fence を掛け、1 pipeline で「gate の行がない」「control が ready で識別が一致」を確認する。gate があれば503を返す。
3. 条件がそろえば既存の `dispatch_checked` を実行する。
4. 通信・プロトコルの失敗は最初に判定し、結果不明で停止する。取引の後始末で失った応答もこの判定に含める。
5. 書き込みを拒否した記録があれば応答を捨て、排他つきで処理し直す。session の削除、`last_seen` の更新、拒否の監査が該当する。

経路の許可表は持たない。書き込みは fence が止めるので、読み取りだと思った経路が書いても安全側に倒れる。

### 最終操作時刻
認証の問い合わせで `last_seen` と `meta.instance` も読み、往復をまとめる。遠隔保存では `at - last_seen >= 60` のときだけ更新する。有効条件（30分）は変えない。

### 接続の再利用
- HTTPクライアントを2つ持つ。`fresh` は現状のまま（idle 0）。`reuse` は idle 1・待機5秒以下。どちらも `retry(never)`・redirect 禁止・同じTLS条件。
- `reuse` は、呼び出し側が「データを変えない」と指定した往復だけが使う（読み取り1往復、Describe、まとめ読み、準備確認）。書き込み、取引の全段階、gate の取得と解放は `fresh`。
- 根拠（hyper-util 0.1.21 のソースで確認）: 再送は、再利用した接続で **hyper が未送信の要求を返した場合**（`take_message`）と、送信前の checkout 失敗だけ。reqwest 0.13.5 はこの設定を公開していない。依存を更新するときは再確認する。
- 試験が通らなければ `reuse` を使わない（要件6.4）。
- 新しく増える危険: 保存先が閉じた待機接続へ読み取りを送ると失敗し、停止になる。待機を短くして起きにくくする。再送はしない。

## 試験
- 往復の計数: 試験の代理サーバー（`crates/db/tests/support/proxy.rs`）に接続数と往復の計数、keep-alive、要件6.3の3故障を足す。
- app: 固定の応答は保存先へ0往復／速い経路は書き込み0／60秒超で排他つきに落ちて更新／gate 保持で503／失効 session の削除／権限拒否の監査／検索POST／件数の一致。
- 往復の目安: 速い経路の件数のまとめは5往復以下。

## 変えないもの
gate の形と復旧手順、結果不明の扱い、控えと復旧の形式、control の schema、Cloud Run の設定。

## 実装で設計から変えた点（2026-11-27）
いずれも安全側に倒した。要件は変えていない。
- 取引の外では Describe を `[Describe, Close]` の1往復で送る。CREATE/ALTER/DROP と `execute_batch` の前に保持済みの結果を捨てる。取引中に DDL を実行した後は、ROLLBACK で列構成が戻る可能性があるため、取引終了まで新しい結果の保持を休止する。
- 準備確認の schema 照合は保持した結果を使うと確認が減る。`read_batch` の `Check` は毎回 Describe を送り、`BEGIN DEFERRED`〜`COMMIT` を同じ pipeline に入れて同じ時点を読む。
- fence は接続ごとでなく、スレッドごとの RAII（`ReadFence::begin`）にした。要求の途中で開く tenant 接続も、開いた時点から fence の中に入るため。fence の拒否で tenant を開けなかったときは、結果不明の印を付けない（送信していないため）。
- 固定部品は HTML 内の参照先を保てる ETag 方式にした。`If-None-Match` の `*`、または一覧中のタグが弱い比較で一致すれば304を返す。`/login` と OAuth の戻り先（`/oauth/activity-return` と `.js`）は gate の外で、ETag を付けずに `no-store` で返す。配備時のログイン方式の確認にもこの指示が必要になる。
- 速い経路の確認（gate・control）と session の読み取りは別の往復のまま（2往復）。session の読み取りは `meta.instance` を同じ文で読む（遠隔保存だけ）。
- 再利用する接続の待機は4秒。切り替えは `meibo_db::REUSE_READ_CONNECTIONS` の1か所。再利用できるのは、同じスレッドで `ReadFence` を持っている間（速い経路）の読み取りだけ。排他つきの経路・CLI・書き込みは毎回新しい接続。速い経路は `read_ready` の前に fence を始める。
- 速い経路で接続が失効したら、破棄中の接続も含め、最外の fence が終わるまで通信・プロトコルの失敗を記録する。結果不明の応答を返し、再送と gate 取得を行わずにプロセスを非0で終了する（要件4.4）。照合不一致など通信以外の失敗は排他つきで処理し直す。`/api/activity-sync` と `/health` は速い経路の対象外。
- 要件3.6 の判定順は境界検査、結果不明・接続失効、匿名要求の枠、Access 検査。枠はプロセスごとに毎秒5件、最大100件で、超過分は保存先を読まずに429を返す。`/health` は対象外。時計が戻ったら補充せず、要求時刻に基準を合わせる。
- ログイン成功時と session 認証成功時に cookie の digest を最大64件まで記録し、認証失敗とログアウトで取り除く。記録済みの cookie は匿名枠を免除する。ただし公開フォームと `/auth/login` は session を確かめずに保存先を使うため、cookie の有無を問わず枠を使う。
