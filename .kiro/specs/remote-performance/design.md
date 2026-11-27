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
| 件数 | 件数のまとめを1〜2文にする。意味は既存の `count` と一致させ、手元版でも同じ値を試験する | `crates/core`, `server/api.rs` |
| 固定の応答 | uncertain/poisoned の確認後、`/login`・公開の固定部品・OAuth戻り先・受付フォームの固定部品は gate の外で返す。`/health` は変えない | `hosted/http.rs` |
| 部品の保存 | 内容hashを付けたURLに `private, max-age, immutable`。hashが一致しない要求と、HTML・API は no-store。URL書き換えが広がりすぎる場合は ETag と 304 でもよい | `hosted/{assets,public_entry,http}.rs` |

## 第2段（承認済み）
### 書き込み禁止の範囲（read fence）
- 接続に「この範囲では書かない」状態を持たせる（RAIIで必ず戻す）。範囲内の読み取り専用でない文、`execute_batch`、IMMEDIATE/EXCLUSIVE の開始は、**送信前に**拒否して接続に印を残す。DEFERRED の読み取り取引は許す。
- 拒否は接続を失効させない。結果不明の分類（`SystemIoFailure` など）を使わない。
- control と、その要求で使う全 tenant 接続に掛ける。

### 速い経路
1. 遠隔保存の GET と `read_only_post` が対象。
2. 1 pipeline で「gate の行がない」「control が ready で識別が一致」を確認する。session の読み取りを同じ往復にまとめてよい。gate があれば改修前と同じ 503。
3. fence を掛けて既存の `dispatch_checked` を実行する。
4. fence の印があれば応答を捨て、改修前の排他つき経路で最初から処理する。session の削除、`last_seen` の更新、拒否の監査はここへ落ちる。エラーの変換に頼らず、印で判定する。
5. 通信失敗・失効は改修前と同じ `uncertain` の扱い。

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
- Describe は SQL を実行しないので、取引の外では外部キー設定なしの `[Describe, Close]` 1往復にした。保持した Describe は、スキーマを変え得る文（CREATE/ALTER/DROP と `execute_batch`）の前に捨てる。
- 準備確認の schema 照合は保持した結果を使うと確認が減る。`read_batch` の `Check` は毎回 Describe を送り、`BEGIN DEFERRED`〜`COMMIT` を同じ pipeline に入れて同じ時点を読む。
- fence は接続ごとでなく、スレッドごとの RAII（`ReadFence::begin`）にした。要求の途中で開く tenant 接続も、開いた時点から fence の中に入るため。fence の拒否で tenant を開けなかったときは、結果不明の印を付けない（送信していないため）。
- 部品の保存は、URL の書き換えが HTML 全体に広がるので、内容 hash の ETag と `no-cache`・304 にした。`/health` は今までどおり gate を通す。 OAuth の戻り先（`/oauth/activity-return` と `.js`）は gate の外で返すが、no-store のままで ETag を付けない。
- 速い経路の確認（gate・control）と session の読み取りは別の往復のまま（2往復）。session の読み取りは `meta.instance` を同じ文で読む（遠隔保存だけ）。
- 再利用する接続の待機は4秒。切り替えは `meibo_db::REUSE_READ_CONNECTIONS` の1か所。再利用できるのは、同じスレッドで `ReadFence` を持っている間（速い経路）の読み取りだけ。排他つきの経路・CLI・書き込みは毎回新しい接続。速い経路は `read_ready` の前に fence を始める。
- 速い経路で通信に失敗して結果不明になったら（要件4.4）、再送せず結果不明を返し、gate は取らない。その応答を送った後に serve ループがエラーで抜け、プロセスは非0で終わる。通信が原因でない失敗（500 になるエラー・応答、tenant を開けない等）は応答にせず、排他つきの経路で処理し直す。fence を持つ間は tenant を開けなくても結果不明の印を付けない。`/api/activity-sync` と `/health` は速い経路に入れない。
- 要件3.6: 遠隔保存では、このプロセスで認証に成功した session を持たない要求（cookie なし・未確認・偽物・失効。`/health` を除く）を、境界の検査の直後、保存先にも Access の検査にも触れる前に、サービス全体で1つの token bucket（毎秒5件・最大100件）で数え、超えた分を429で断る。認証に成功した session の digest は最大64件まで覚え（両方の経路で記録し、認証に失敗・ログアウトしたら外す）、その cookie は枠を使わない。時刻は要求の `at`（秒）で、時計が戻ったら補充せずに基準を合わせ直す。プロセスごとの数なので、複数インスタンスでは合計がその倍数になる。
