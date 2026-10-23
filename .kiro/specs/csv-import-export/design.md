# 既存coreを使うCSVとCLI

## 境界
`meibo-import` は `Read`/`Write` を受けるCSV変換。`meibo-app`（バイナリ `meibo`）は引数とファイルを扱い、この変換とcoreを呼ぶ。coreにCSVや通信の依存を入れない。保存形式の変更はない。

## 登録
coreへ `DuplicatePolicy::{Error,Skip}`、`ImportBatchReport { inserted, skipped }` と `Engine::import_companies(&[CompanyInput], DuplicatePolicy)` を追加する。バッチは最大500件。既存bulkのINSERT・coreの検証/空き見積り・FTS/件数更新を再利用し、活動や既存企業を更新しない。失敗にバッチ内の1始まり位置を添える。解析側は500件または8MiBでバッチを確定する。構文/I/O/変換失敗時の未確定分は登録しない。

## 書出
coreへ `Engine::visit_companies(&Filters, &Fields, visitor)` を追加する。単一の読み取りtransactionで既存一覧計画と個人情報マスクを再利用し、行を順次callbackへ渡す。callbackエラーでも読取transactionを解放する。exportは消去済みを常に省く。CLIはNamedTempFileとpersist_noclobberで上書き競合を防ぐ。

## CSV契約
見出しは順不同、必須は `origin,name,prefecture`。任意は `corporate_number,municipality,industry,phone,website,owner,status,is_sole_proprietor,source_id,source_as_of`。未知/重複見出しは拒否する。originは `synthetic`/`real`、statusは0〜5（既定0）、industryは1〜20、真偽はtrue/false/1/0（既定false）。空欄の任意項目は未指定。

1MiB以内のレコードだけを保持する厳密なバイト単位のパーサに、既存自作OSSのkamokuの引用符規則を移す。全体保持の既存パーサは移さない。Shift_JISの後続バイトはCSV区切り/引用符/改行と重ならないため、レコード内の各セルをencoding_rsで厳密に復号できる。外部のCSV crateも調べたが、寛容な引用符解析とレコード容量上限の追加処理が必要なため今回は使わない。encoding_rs 0.8.42は取得済みキャッシュを再利用する。

出力の数式保護は先頭空白を除いた文字の =,+,-,@ と先頭のタブ/CR/LFに対して行う。Shift_JISに変換できない文字を置換しない。保護のアポストロフィは自動で外さない。

## CLI契約
`meibo init DB --origin synthetic|real`、`import DB CSV [--encoding utf-8|shift-jis] [--duplicates error|skip]`、`export DB CSV [--encoding ...] [--include-personal] [--include-excluded]`、`list DB [--limit 1..200] [--search TERM]`、`verify DB`、`demo DB [--rows N]`。demoは合成だけで既定100社。

## 検証と資源
意味のある回帰とE2Eを追加し、fmt/clippy/全試験/MSRV/deny/links/trace/docs-checkを実施する。大きな測定はcoreの検索・保存形式を変更しない限り繰り返さない。外部SSDは空き約326MiBのため新規の大容量出力は作らず、取得済み依存と既存ビルド出力を増分利用する。独立レビューでCSV境界と原子性を確認する。

## 参考
公式の[encoding_rs Decoder](https://docs.rs/encoding_rs/0.8.42/encoding_rs/struct.Decoder.html)と[CSV ReaderBuilder](https://docs.rs/csv/1.4.0/csv/struct.ReaderBuilder.html)を2026-10-23に確認。全置換を許すencoding_rs_ioは採用しない。
