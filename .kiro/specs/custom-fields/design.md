# 独自項目の設計

## Boundary Commitments
core/custom_fieldsは定義・型検証・値・専用検索を所有する。既存一覧クエリへ任意SQLを混ぜない。
app/server/custom_fieldsとui/custom-fieldsはHTTP変換と取引先の操作欄を担当する。

## 保存と互換性
定義/値の2表をcoreのCRM拡張と同じtransactionで初期化し、版をmetaに持つ。
型はenum、値は型別の列。field_idと値とcompany_idの複合索引を使う。
索引指定のない検索は遅い条件と明記する。名前をSQL識別子に挿入しない。
定義100件/DB、名前100文字、テキスト4000文字、検索200件を上限とする。
値の消去を既存eraseへ接続し、CSVと既定detailへ値を追加しない。

## File Structure Plan
- crates/core/src/custom_fields.rs: 定義/値/検索と単体試験。
- crates/core/src/lib.rs, schema.rs, erase.rs: 登録・初期化・消去の統合。
- crates/app/src/server/custom_fields.rs: 厳密なHTTP入力と応答。
- crates/app/ui/custom-fields.js, custom-fields.css: 項目定義と値の編集。
- crates/app/src/server.rs, ui/app.js, ui/index.html: 入口と画面の接続。

## UX
営業担当が取引先を開き、自社の補足情報を追加して次の対応に使う。
戦略は取引先情報の補完、要件は定義/値/解除、構造は詳細タブ内の独自項目、
骨格は既存詳細欄の後に一覧と編集、表層は既存の文字/余白/保存エラー表示を継承する。

## 検証
種類不一致・NaN/無限・不在/消去済・重複・上限・再起動・削除の分離・SQL引用文字・
索引付き一致検索・個人項目の非公開既定を試験する。画面で作成/保存/失敗/再読込を確認する。
