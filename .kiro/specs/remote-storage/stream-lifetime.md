# Hrana stream寿命の互換補正

2026-10-27、実Tursoはtransaction外でSQLを保持しないFK設定pipelineの後に`baton=null`を返した。手元sqldはbatonを返していたため、実接続では初期確認の段階で拒否していた。業務schemaは未作成のままである。

[公式HTTP v2仕様](https://github.com/tursodatabase/libsql/blob/main/docs/HTTP_V2_SPEC.md)はnull batonを終了済みstreamとして扱い、stored SQLのscopeをsingle streamと定める。実提供者でも、SQLを保持したstreamはFK設定・BEGIN/COMMIT後も継続し、明示Close後に終了すると確認した。

## 保存方針
既存のFK読戻しSQLを一つの内部sql_idでStoreSqlし、実際の読戻しもそのidで行う。使わないdummy SQLは保存しない。idはadapter内部だけに閉じ、利用者のSQL設定にしない。
StoreSql・FK有効化・読戻しの成功を全て確認してから業務SQLを送る。保持中のbatonは引き続き必須で、nullやvariant不一致を受け入れない。
暗黙transactionを追加せず、autocommit・BEGIN mode・commit/rollback・lastrowid・Close・OutcomeUnknownの既存契約を保つ。TLS・応答上限・期限・自動再送禁止・秘密を伏せる診断も変えない。

## 検証
production setup OFFで従来の要求を送り、実提供者の自動終了を再現するgated fixtureが実Connection queryを失敗させる。ON実装・flag除去後は同じ試験が成功することを確認する。
StoreSql拒否・誤variant・FKの不一致では業務SQLを送らない。DB/core/appの既存transaction・fault・走査・認証/gateを回帰検証する。
実提供者では最初に新接続で空schemaを照合し、修正済みcoreの作成・合成データ確定・Close・新規接続読取り・verifyを実施する。実企業データは使わず、提供者の成功を最終配備artifactの成功へ置き換えない。
