# Research: remote-storage

2026-10-26。cc-sddの既存仕様を引き継ぐcomplex integrationとして調査した。

## Summary
Turso CloudのlibSQL engineは既存SQLと互換性がある。新Rust engineはFTS5・STORED生成列・更新可能なWITHOUT ROWIDに不足があり、今回は対象外とする。同期APIを小さなadapterで共有し、手元版のrusqlite経路を保つ。

## Research Log
- [公式Rust SDK](https://docs.turso.tech/sdk/rust/quickstart)と[公式HTTP仕様](https://docs.turso.tech/sdk/http/reference)を参照した。対象DBはlibSQLで、通信には公式Hrana型と現行HTTP clientを使う。new Turso engineへの変更はしない。
- [Cloud制約](https://docs.turso.tech/cloud/limitations): application_id/user_versionは読取り専用、busy_timeout/journal_modeはローカルと異なる。remoteのmarker/版はmeta表へ置く。
- [新engineの互換性](https://github.com/tursodatabase/turso/blob/main/COMPAT.md): SQLite FTS5は未対応、生成列はvirtualのみ、WITHOUT ROWIDの更新等に制約。現在の業務SQLを移す対象にはしない。
- 実際のlibSQL sqldへloopbackのHTTPで接続し、新規合成DBへ既存DDL31文を適用した。日本語FTS5 trigram、STORED列の更新、append-only trigger、同pipeline内のBEGIN IMMEDIATE/ROLLBACKが成立した。application_id更新はSQL_PARSE_ERRORとして拒否された。サーバーは終了済み。
- [export公式](https://docs.turso.tech/cli/db/export): exportしたsnapshotは最新変更を含まない場合がある。未同期の提供者snapshotだけを「保存成功」と扱わない。
- 現在のcoreはDb::write、snapshot、Row::get、cached Statement、last_insert_rowidへ依存する。業務SQLを全てRemoteEngineへ複製すると二重保守になる。
- 現在のhostedはcontrol/組織別fileとfilesystem lockを使う。別process/再作成ではローカルlockを共有できないため、controlの非失効gateへ置き換える。

## Architecture Pattern Evaluation
| 案 | 判断 |
|---|---|
| 接続URLの差替えだけ | rusqliteのfile/WAL/PRAGMA/backup前提が残るため不可 |
| FUSE上のSQLiteや定期file複製 | 確定済みデータの耐久性と排他を保証できず不採用 |
| 業務APIの別言語再実装 | UI/API/認証/業務の重複が大きく不採用 |
| optional同期adapterとbackend policy | 現在のSQL/APIを再利用し、remote固有の保存/障害/運用だけ明示できるため採用 |

## Design Decisions
Connection/Statement/Row/Transactionの既存使用範囲をadapterへ置く。公式libsql-hranaの型でv2/pipelineを送受信し、業務SQLやprotocolの型を複製しない。全件scanは500行以下のkeysetへ分け、単一応答は受信時点で上限を守る。ローカル既定経路は変更しない。

通常要求と運用は非失効のcontrol gateを共有する。TTL自動takeoverは使わず、確定結果や以前の処理の終了を確認できなければ占有を残す。可用性より更新の重複・不整合防止を優先する初期ベータの判断である。

保存はgate下での型付き行の逐次snapshotとする。任意SQLの保存/再実行をせず、既知schema・行数・hash・再構成したFTS/件数/foldを照合する。復旧先は新規の空DB一式に限定し、全体readyになるまで公開しない。

## Risks and Revalidation
旧libsql 0.10.0 SDKはh2 0.3系等の依存監査を通過できず不採用とした。TLS差替えだけでは解決しない。turso_serverless 0.1.3もpublic client注入がなく、error bodyとcursor全行を無制限にbufferする経路があるため採らない。以前のSDK remote/tls・meter付きconnector案はこの判断で置き換える。

公式libsql-hrana =0.10.0とreqwest =0.13.5（defaultなし、json/native-tls-no-alpn、async/shared Tokio）はRust1.88でcompileし、online cargo-denyのadvisories/licenses/sourcesを通過した。HTTP1、redirectなし、retryなし、TLS1.2以上を明示し、HTTP2・旧Hyper0.14/TLS依存を持ち込まない。統合後にも同じ検査を行う。

実sqldでv2のbatonを別roundtripのBEGIN/INSERT/SELECT/ROLLBACKに引き継ぎ、同じtransactionを確認した。Sequenceへ既存schema全文3696bytesを渡してtriggerもそのまま適用し、trigger違反後のROLLBACKも確認した。SQLのセミコロンを分割する実装は不要である。v2 GetAutocommitは拒否されるため使わない。

現行Turso Cloudの合成DBでもSequenceと約11.75秒のtransaction確定を確認した。古いHTTP文書の5秒を現在環境の総時間上限とは扱わず、この観測だけから500行importの成立も断定しない。後続の実環境検証で確認する。

ネットワーク遅延のある数百万行のscan/importをローカルの測定値と同一視しない。SDKが全行をbufferする経路はページ化し、未検証の速度を主張しない。Cloudflareの実環境保持と公開ゲートは下流仕様で確認する。
