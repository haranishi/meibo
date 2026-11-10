# SQLite依存の脆弱性を公式修正版で解消する

2026-11-11の継続作業で、オンライン依存監査が[RUSTSEC-2026-0459](https://rustsec.org/advisories/RUSTSEC-2026-0459.html)を検出した。最低Rust1.88を維持できる公開済み修正版はなく、親が依頼範囲の必要な安全性修正として最低版の最小更新を決定した。

## 依存と互換性
- 4crateのrusqliteを公式0.41.1へ、lockのlibsqlite3-sysを0.39.1（SQLite3.54.1）へ更新する。bundled/backupとdefault版の通信禁止は維持する。
- [公式manifest](https://github.com/rusqlite/rusqlite/blob/v0.41.1/Cargo.toml)の最低要件に合わせ、workspace/CI/現行手順のRust最低版を1.89.0へ更新する。開発版1.99は維持する。過去の1.88検証記録は履歴として残す。
- `column_names()`のIterator化に対応し、SQL adapterの列名検査・row型・エラー境界を保つ。DB schema・snapshot形式・認証・業務ルールを変えない。
- 監査ignoreや独自vendor、最低版の無視、TLS弱体化、実データのGit格納は行わない。

## 完了の条件
実行中SQLiteの修正済み版を検査する回帰を旧版でREDにし、更新後の版・core/import・db・default/cloud・最低RustのCIとオンライン監査を確認する。既存の私有コピーを新しい実行物で開き、整合性・件数・営業情報保持と再起動・画面を再確認する。

公式資料：[rusqlite0.41.1](https://github.com/rusqlite/rusqlite/releases/tag/v0.41.1)、[SQLite3.54.1](https://sqlite.org/releaselog/3_54_1.html)。公開配備や新規課金はこの更新に含めない。
