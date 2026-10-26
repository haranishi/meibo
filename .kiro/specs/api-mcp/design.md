# APIとMCPの設計

## 1. Boundary Commitments

Public APIはv1のpath/query/bodyを検証し、既存server::apiの営業処理へ変換する。MCPはstdio framing、JSON-RPC、version、toolの入出力だけを所有する。HTTPの認証/権限/Host/Origin/サイズは共通server境界に残す。MCPにHTTPサーバーを起動させない。

## 2. 共通操作とHTTP契約

- `server/public_api.rs::dispatch(&mut Engine, &Input, path, query) -> Option<Reply>` を既存共通dispatch内から呼ぶ。v1以外はNone。
- GET `/api/v1/companies`: 既存 `api::companies` を再利用。`q/limit/after/view/status/prefecture/has_phone/sort`、未知・重複は拒否。応答は `{rows,next,count}`。
- GET `/api/v1/companies/{id}`: 既存 `api::detail` を再利用。queryは既存 `include_contacts=1/include_private=1` の明示指定だけ。既定は個人項目を開示しない。
- POST `/api/v1/companies/{id}/activities`: 既存 `api::activity` を再利用。入力は `note/status/next_action_at/kind/contact_id/at`。省略/NULLの意味は既存処理を維持。
- pathのIDは正整数。未知path=404、既知pathへの不適切method=405。エラーは既存 `{error}` とstatusを維持する。
- Activity追記は非冪等。自動retryしない。応答不明時は詳細履歴を確認する。exactly-onceを約束しない。
- 公的データの `source_id/source_as_of/attribution/modification_notice` はpublic-data-importersが既存company serializerへ追加する。v1/MCPはその同一結果をそのまま返し、別の出典判定や再取得を作らない。

## 3. MCP

- `mcp::serve(db: &Path, allow_write: bool)` が指定DBを開き、stdin/stdout lockを使う。標準出力に起動表示を出さない。
- stdioはUTF-8/改行区切り。1行1MiBまでを `BufRead::fill_buf/consume` で読み、超過行は改行まで捨てる。応答は1行のJSONのみ、EOFで終了。
- `server/discover`と、必須 `_meta` のprotocolVersion=`2026-07-28`/clientCapabilities objectを持つ要求は現行版。結果は `resultType=complete`、serverInfo metadata、一覧/discoverには `ttlMs=0/cacheScope=private`。
- 2025-11-25互換は `initialize`→`notifications/initialized`→業務要求。initializeの未対応要求版には2025-11-25を返す。現行版の要求を従来版sessionへ黙って降格しない。
- `ping`、`tools/list`、`tools/call`。toolは `list_companies/search_companies/get_company` と、明示allow-write時だけ `record_activity`。
- `public_api::call_tool` がtool引数を厳密に確認し、v1経路へ変換。HTTP用Inputから任意header/host/pathをtool引数へ公開しない。
- protocol errorは -32700/-32600/-32601/-32602、未対応版は -32022とsupported/requested。通知には応答せず、副作用のある通知は実行しない。業務失敗はtool resultの `isError=true`。
- read toolにreadOnlyHint、書込toolにdestructiveHint=false/idempotentHint=false/openWorldHint=false。書込toolはactivitiesを追記するだけ。

## 4. File Structure Plan

| ファイル | 役割 |
|---|---|
| `crates/app/src/server/public_api.rs` | HTTP v1とMCP共通操作、営業APIへの変換、境界単体試験 |
| `crates/app/src/mcp.rs` | stdio入口とmessage reader |
| `crates/app/src/mcp/protocol.rs` | version/JSON-RPC/通知/応答 |
| `crates/app/src/mcp/tools.rs` | tool schemasと共通操作呼出 |
| `crates/app/tests/mcp.rs` | CLI子プロセスのframing/EOF/永続化試験 |
| `docs/api.md`, `docs/mcp.md` | 互換契約・使い方・非冪等性と接続境界 |

既存main/args/commands/serverのmod登録・CLI分岐・v1分岐は親が統合する。Cargo依存はserde_jsonと既存Engineだけを使い、SDK追加を避ける。

## 5. 検証

要件1.1〜2.3: 合成DBでpagination/検索/詳細の個人情報既定/活動保存/不正入力/HTTP Host/Originとmethodを検証。NTA由来を示す合成fixtureで出典・基準日・加工表示を両入口から確認する。
要件3.1〜3.7: 現行metadata/未対応版/従来initialize順序/通知/parse error/unknown method/上限超過後の復帰/EOF/stdoutの純粋性/tool annotations/書込offを試験。
要件4.1: 既存app試験、fmt/clippy、CLI子プロセス試験を再実行。実DBと本番HTTPを検証済みとは書かない。
