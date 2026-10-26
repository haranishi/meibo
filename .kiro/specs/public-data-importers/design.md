# 公的企業データ接続の設計

## Boundary Commitments
importが国税庁カタログを読み、coreの公開更新口を使う。正本DBをEngineで開かない。
coreはDB種別の整合を保ち、CRMの拡張表を両種別で利用可能にする。
appは実データの起動を明示オプションに限定し、既定の合成デモを保持する。

## 接続と更新
- rusqliteの読み取り専用接続でcompanies/source_batchesを参照する。
- source_type=nta_monthly_all/nta_daily、state=complete、signature_verified=1を検証する。
- 法人番号・legal_name・prefecture_codeと、source_batchesの公開出典/基準日だけを選ぶ。
- coreに法人番号検索と公的基本情報更新を追加。500件以下のバッチを原子的に処理する。
- 同じ番号の消去済みレコードは復活させない。既存企業の電話/HP/業種/担当営業は触らない。
- FTSと件数表は既存の書込処理を再利用する。営業状態を取込の値で上書きしない。
- 失敗時は確定済み件数のみを返し、供給元の行本文や私的パスを公開ログに出さない。

## File Structure Plan
- crates/core/src/public_data.rs: 公的情報のバッチupsertと試験。
- crates/core/src/schema.rs, crm.rs: 両DB種別のCRM拡張。
- crates/import/src/nta.rs: 供給元の検査と逐次取込。
- crates/app/src/args.rs, commands.rs: sync-nta、serve --allow-real。
- crates/app/src/hosted/cli.rs: 組織を検証し、共通保守lockの下でsync-ntaを実行する。
- crates/app/src/server/api.rs, import_ui.rs: 保存先の種別を利用する。
- crates/app/ui/app.js, index.html: 種別/出典の表示。
- docs/public-data.md: 提供条件、更新/部分失敗の扱い。

## 検証
両種別のCRM保存/再起動、旧DBの拡張/容量不足、法人番号再取込と営業状態保持、
不許可出典/混在/不正行の拒否、供給元のハッシュ不変、出典表示を確認する。
合成fixtureを用いた自動試験と、許可された実カタログの件数・品質確認を区別する。
