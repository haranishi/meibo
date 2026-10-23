# CSVとCLIの実装・検証

- [x] 1. coreのバッチ登録・snapshot visitorを追加し、重複・種別・容量・rollback・並行更新を回帰検証する。_Requirements: 12.1, 12.2, 12.3, 13.1_
- [x] 2. 逐次CSV、厳密な復号、ヘッダー/型変換、バッチ接続、書出を追加する。構文・上限・部分確定・個人情報・数式・変換失敗を検証する。_Requirements: 11.1, 11.2, 11.3, 12.1, 12.4, 13.2, 13.3_
- [x] 3. CLI、書出の一時ファイル、新規ファイル保護を追加し、合成データのE2Eを通す。_Requirements: 13.4, 14.1, 14.2_
- [x] 4. 関連する品質の門と独立レビューを実行し、仕様・README・Obsidianへ証拠と残件を残す。

## 実行記録
2026-10-23: 前回の土台完成と外部公開保留を引き継いで開始。同日の追加指示に従い、非公開リポジトリでcommit・push・PR作成・マージまで進める。実データ接続は行わない。過去の検証記録が参照する `.agent-harness/` の生ログは共有しない。

完了: `cargo xtask check-all` 成功（207成功・3件の既存大型測定はignored、fmt/clippy/links/deny/trace/docs-check通過）。基準68件の未対応0。`cargo +1.88 check --workspace --all-targets` 成功。変更コード・文書・新仕様のgitleaks検出0。独立レビューの未解決指摘0。記録は `.agent-harness/runs/2026-10-23-csv-import-export/`。445万社の公式測定と過去184件の変異試験は再実行していない。
