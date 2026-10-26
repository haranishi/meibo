# API/MCPのタスクと検証

2026-10-26本人の継続実装・一般公開指示を承認根拠とする。独立タスクグラフ再審PASS後に確定。

- [x] 1.1 共有サーバー公開口とCLI接続の土台を用意する（親担当）
  - server Input/Reply/helpers/dispatchをcrate内へ公開し、v1分岐とMCP module/commandを登録する。
  - 既存HTTP境界の内側でv1入口を呼び、`mcp DB [--allow-write]`からstdio入口へ到達できる状態にする。
  - _Boundary: Shared app integration_
  - _Requirements: 2.1, 2.2, 3.4, 3.5_
- [x] 2.1 版付き営業APIを実装する
  - 既存一覧/検索/詳細/活動処理を再利用し、method/ID/query/個人情報既定を回帰試験で確認する。
  - 出典・基準日・加工表示を既存serializerから同じ結果として返す。
  - _Depends: 1.1_
  - _Boundary: Public API_
  - _Requirements: 1.1, 1.2, 1.3, 1.4, 1.5, 1.6, 2.1, 2.2, 2.3_
- [x] 3.1 MCP stdioとtoolを実装する
  - 2026-07-28のmetadataと2025-11-25互換初期化、通知、エラー、入力上限を試験する。
  - 書込off時はtoolの列挙/直接実行を拒否し、営業操作はv1と同じ結果になる。
  - _Depends: 2.1_
  - _Boundary: MCP_
  - _Requirements: 3.1, 3.2, 3.3, 3.4, 3.5, 3.6, 3.7_
- [x] 4.1 CLI/HTTPを統合して保存と出典を確認する（親と担当）
  - CLI子プロセスのstdout/EOF、HTTP境界と権限、保存後の再open、両入口の出典を合成DBで確認する。
  - 機械検査の結果と未検証の範囲を記録し、実クライアント/本番接続を未確認のまま合格扱いしない。
  - _Depends: 3.1_
  - _Boundary: API/MCP integration_
  - _Requirements: 1.1, 1.2, 1.3, 1.4, 1.5, 1.6, 2.1, 2.2, 2.3, 3.1, 3.2, 3.3, 3.4, 3.5, 3.6, 3.7, 4.1_

## 実装記録

親が共通接続を担当し、担当エージェントは2.1→3.1をmanual modeで実行する。独立コードレビューは統合後に親が行う。

2026-10-26 局所実装・試験: API unit6、MCP unit7、MCP CLI子プロセス4が成功。APIの最初の5試験は未実装stubで全失敗→実装後成功。MCPの実装前実行は隣接moduleの未完成でcompile停止し、挙動のREDは未確認。現行/従来version、metadata、通知の非実行、write-off、1MiB上限後の復帰、stdout/EOF、活動再openを確認した。

出典検証: NTA由来を示す合成fixtureで、HTTP v1とtoolの詳細JSONが一致し、source_id/source_as_of/attribution/modification_noticeを返す。CLIのMCP結果でも同じ4項目を確認した。

統合途中の確認: app unit43件が成功。後続の既存CLI再import試験は新しいCSV列と13列上限の不一致で停止（親担当へ連絡）。ClippyのAPI/MCP指摘は修正済みだが、全体は隣接custom_fieldsの2指摘が残る時点で未合格。チェックボックスは独立レビューと親の最終統合検証のあとに確定する。

未検証: 実AIクライアント各製品との接続、遠隔MCP、本番HTTP/本番組織の権限、実企業データの取得。実データを試験へ使用していない。

## 2026-10-26 最終統合の記録

2026-10-26: API6/MCP7/stdio子プロセス4試験成功。現行2026-07-28と2025-11-25互換、書込off/個人情報既定/非冪等追記/サイズ/未知入力/出典一致を確認。実AIクライアントと遠隔transportは未検証。

全体289試験成功・大型測定3ignored、check-all/fmt/strict clippy/依存方針/オンラインadvisories/MSRV1.88成功。REUSE3.3準拠、秘密検査0。独立境界レビューAPPROVED。
