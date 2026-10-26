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
