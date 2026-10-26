# ローカルMCPから営業データを使う

`meibo mcp DB` は指定したCRMの保存先を読み、MCPクライアントと標準入出力で通信します。HTTPサーバーは起動しません。
OS上でそのDBを開ける利用者の権限で動作します。接続設定には自分が利用を認めたDBだけを指定してください。

```json
{
  "mcpServers": {
    "meibo": {
      "command": "/absolute/path/to/meibo",
      "args": ["mcp", "/absolute/path/to/crm.db"]
    }
  }
}
```

## 使えるtool

| tool | 引数と結果 |
|---|---|
| `list_companies` | 任意のlimit/after/view/status/prefecture/has_phone/sort。`{rows,next,count}` |
| `search_companies` | 必須query、任意limit/after。社名部分一致 |
| `get_company` | 必須の正整数id。任意include_contacts/include_privateをbooleanで明示 |
| `record_activity` | 書込起動時だけ。必須company_id/note、任意kind/status/next_action_at/contact_id/at |

一覧・検索の上限は1〜200です。未知引数と型違いは拒否します。担当者連絡先と個人事業の個人項目は既定で非表示です。
公的データの供給元・基準日・出典/加工表示はHTTP v1と同じ企業結果に含まれます。
企業詳細の活動メモはクライアントへ渡されます。接続先AIサービスでのデータ利用条件は利用者が確認してください。

## 活動の記録を有効にする

起動引数へ `--allow-write` を加えると `record_activity` を提供します。指定しない場合、tool一覧と直接の呼出の両方で拒否します。
追記した活動の取消や除外解除は、このMCP toolには含めません。外部への架電・メール送信も行いません。
活動追記は非冪等です。応答不明時は自動再送せず `get_company` の履歴で結果を確認してください。

## 通信契約

- 現行2026-07-28: 毎要求のparams._metaへ `io.modelcontextprotocol/protocolVersion` とobject型の `io.modelcontextprotocol/clientCapabilities` が必要です。`server/discover` で機能を取得できます。initializeは不要です。
- 互換2025-11-25: `initialize` → `notifications/initialized` の後にtoolを呼びます。initializeで別の版を要求すると2025-11-25を提案します。
- `ping/tools/list/tools/call` を実装します。tool一覧はwrite権限により固定で、現行版のcache hintは `ttlMs=0/cacheScope=private` です。
- 1行1件のJSON-RPC、入力は1MiBまでです。標準出力にはJSON-RPCだけを書き、入力終了で終了します。
- 不正JSON=-32700、不正要求=-32600、未知method=-32601、不正params=-32602、現行版の未対応version=-32022です。業務上の失敗はtool結果の `isError=true` で返します。
- 通知へ応答せず、業務toolを通知として送っても実行しません。遠隔HTTP transportと独自AI推論は提供しません。

通信規格は [MCP stdio](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/stdio) と [versioning](https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning) を参照してください。
