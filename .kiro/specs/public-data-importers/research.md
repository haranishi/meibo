# 調査と判断

2026-10-26。既存Engine/CSVとrusqlite 0.40.2を再利用する。
実データはサーバーだけでなくCRMの保存表/公開操作でも拒否されていたため、両層を変更する。
供給元カタログのcompanies/source_batches構造を読み取り専用で確認した。

国税庁の利用規約はPDL1.0に従い出典と加工表示を求める。
- https://www.houjin-bangou.nta.go.jp/riyokiyaku/
- https://www.digital.go.jp/resources/open_data/public_data_license_v1.0

元DBを直接CRMとして使う案は、スキーマ差と営業記録保護のため採用しない。
しょくばらぼのデータは既存の不使用方針を維持する。
全件の削除同期は営業対象を失わないよう今回の自動処理に含めない。
