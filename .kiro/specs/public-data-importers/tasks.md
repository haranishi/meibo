# 公的データ接続の実装

2026-10-26本人の実行・公開依頼に基づく。独立タスクグラフ再審: PASS。

- [x] 1.1 実DBのCRM拡張と明示起動を統合する
  - 両種別で記録と再起動を確認し、旧DBの拡張前の容量検査と混在拒否を維持する。
  - _Boundary: Real CRM integration_ _Requirements: 1.1, 1.2, 1.3_
- [x] 2.1 許可した国税庁カタログの取込を統合する
  - importの直接依存・公開口・sync-nta CLIを追加する。
  - hostedでは組織を確定して共通保守lockの下で取り込み、結果を監査へ残す。
  - 500件バッチ、法人番号再取込、営業状態/消去保持、部分失敗と元DB不変を確認する。
  - _Boundary: NTA importer and core upsert integration_ _Depends: 1.1_
  - _Requirements: 2.1, 2.2, 2.3, 2.4_
- [x] 3.1 4媒体の出典表示と配布物を検証する
  - 画面/CSV/API/MCPから出典と加工を確認し、実データを配布物へ含めない。
  - 許可済み実カタログの件数と取込後の品質を照合する。
  - _Boundary: Attribution integration_ _Depends: 2.1, api-mcp 4.1_
  - _Requirements: 3.1, 3.2_

## 2026-10-26 最終統合の記録

2026-10-26: 旧DB容量/実origin CRM/公的基本情報upsert/営業記録保持/NULL出典拒否/日次overlay追加更新除外/消去hash再作成拒否を検証。実効4,450,944社を別CRMへ全件取込、件数一致・禁止3項目0・元DB hash不変・整合性PASS。tenant限定CLI取込と他組織0件/不正組織拒否も成功。画面/CSV/API/MCPの出典は各入口で確認。実データはGitに含めない。

全体289試験成功・大型測定3ignored、check-all/fmt/strict clippy/依存方針/オンラインadvisories/MSRV1.88成功。REUSE3.3準拠、秘密検査0。独立境界レビューAPPROVED。

- [x] 4.1 日次同期で無変更の公的情報を書き換えない
  - coreの既存upsertで厳密な無変更を検出し、skippedに含める。名称/地域/出典/基準日が変わるときは更新する。
  - 同値再実行の更新日時・営業記録保持と、同じ検索キーでも名称表記が違う場合の更新を合成試験で確認する。
  - 検証：core/import189件成功、既存測定3件ignored、strict clippyとdefault build成功。独立9件/旧実行物との差分/NULL値の照合APPROVED。
  - _Boundary: core public_data upsert, focused tests, public import documentation_
  - _Depends: 2.1_ _Requirements: 2.2, 2.3, 2.4, 2.5_
- [x] 4.2 更新済み公的名簿を保全した別CRMで同期・実画面確認する
  - 既存CRMを残したSSDコピーへ最新の署名済み名簿を同期し、再実行の追加/更新0、供給元hash不変、整合性と出典表示を確認する。
  - 供給元から消えた企業は営業履歴保持のため自動削除しない。current社数とCRM保存社数の差を説明する。
  - 検証：最新4,453,446社とCRMの全項目照合で不一致0、再同期0追加/0更新、元DB不変、営業項目変更0。独立APPROVED、3幅の実ブラウザ6確認も成功。SQLite更新後の再確認はremote-storage5.2で行う。
  - _Boundary: Local real-data operations and evidence_
  - _Depends: 4.1_ _Requirements: 1.1, 1.2, 1.3, 2.1, 2.2, 2.3, 2.5, 3.1, 3.2_
