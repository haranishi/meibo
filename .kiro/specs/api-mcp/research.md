# API/MCPの調査と判断

確認日: 2026-10-26。コード調査と公式規格に基づく。

## 既存資産

- Engineの一覧・検索・詳細・活動追記と、server/api.rsの入力検証・個人情報既定・エラー応答を再利用できる。
- 共通HTTP境界はloopback Host/同一Origin/専用header/サイズ制限を持つ。v1にも維持し、hosted認証はteam-accessで前段に置く。
- Activity追記は非冪等。新たな非原子的dedupeを足さず、呼出側に自動retryしない契約を公開する。

## 公式資料

- [2026-07-28 versioning](https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning): 現行版は初期化不要で、要求ごとのversion/metadataとserver/discoverを使う。2025-11-25以前は別の互換動作。
- [stdio transport](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/stdio): 改行で区切るJSON-RPC、stdoutはprotocolだけ、stdin終了でshutdown。
- [Base protocol](https://modelcontextprotocol.io/specification/2026-07-28/basic/index): protocolVersionとclientCapabilitiesは毎要求必須。不足は-32602。
- [Schema reference](https://modelcontextprotocol.io/specification/2026-07-28/schema): discover/listはresultType/ttlMs/cacheScope、unsupported version=-32022。
- [Tools](https://modelcontextprotocol.io/specification/2026-07-28/server/tools): 引数schema、tool注釈、業務失敗isErrorと通信エラーの区別。
- [2025-11-25 lifecycle](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle): initialize/initializedの互換順序。

## Design Decisions

1. 2026-07-28を現行として扱い、2025-11-25も互換実装する。旧版initializeだけの実装は採らない。
2. 既存依存serde_jsonと標準ioで小さなstdio adapterを作る。tools以外を提供しないため、async runtimeと包括SDKを増やすより境界が狭い。
3. HTTP/MCPの営業操作をPublic APIへ集約する。MCPが独自にEngineの型検証や個人情報選択を再実装する案は採らない。
4. tool一覧はwrite capabilityにより固定。read-onlyプロセスでは書込toolの列挙・呼出を拒否する。
5. 参照スキル: search-first/API interface/security-and-hardening。入力上限、非冪等性、未知引数、秘密の非出力を検証する。

## 未確認

実クライアント各製品との接続、遠隔MCP HTTP、本番組織の認可はこの仕様の検証対象に含めない。
