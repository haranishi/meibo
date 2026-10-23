# CSVで企業を出し入れする

CSVとCLIは、Rustコードを書かずに保存・検索の土台を試すための入口です。開発・検証は合成データだけで行っています。画面はまだありません。

## 起動と最初の操作

リポジトリのルートで実行します。データを外部へ送信しません。

```sh
cargo run -p meibo-app -- demo demo.db --rows 100
cargo run -p meibo-app -- list demo.db --limit 20
cargo run -p meibo-app -- list demo.db --search 合成
cargo run -p meibo-app -- export demo.db companies.csv
cargo run -p meibo-app -- init imported.db --origin synthetic
cargo run -p meibo-app -- import imported.db companies.csv
cargo run -p meibo-app -- verify imported.db
```

demoは既存ファイルを上書きしません。企業と活動を合成しますが、CSVに出すのは企業だけです。書出時には除外企業と個人事業を既定で省くため、CSVの件数はdemoの件数と一致するとは限りません。

## 見出しと文字コード

見出しは英語の固定名です。順番は自由で、必須は `origin,name,prefecture`。任意は `corporate_number,municipality,industry,phone,website,owner,status,is_sole_proprietor,source_id,source_as_of`。未知の列や同じ見出しの重複は拒否します。

```csv
origin,name,prefecture,corporate_number,phone
synthetic,株式会社合成見本,13,6600000000000,0300000000
```

originは `synthetic` または `real` で、保存先と一致する必要があります。サンプルは合成です。都道府県は1〜47、市区町村は先頭0を保つ5桁、業種は1〜20、statusは未着手0・対応中1・商談2・受注3・見送り4・除外5です。個人事業の印はtrue/falseまたは1/0。任意欄の空欄は未指定です。

入力はUTF-8（BOM有無）、出力はBOMつきUTF-8が既定です。`--encoding shift-jis` でWindows-31Jを選べます。不正なバイトや、書出先の文字コードで表せない文字はエラーにします。文字を置換して続行しません。引用符内の改行・カンマ・引用符の二重化に対応します。1レコードは最大1MiBです。

## 途中で止まったとき

最大500件または文字列8MiBを区切りに確定します。失敗したバッチは全体を取り消しますが、前のバッチは残ります。エラーにはレコード番号（見出しが1）と確定済み件数が出ます。ファイル全体を取り消す取込ではありません。

法人番号が重複したら既定で止まります。`--duplicates skip` を指定すると、すでにある企業を変更せず読み飛ばします。同じファイルの重複も同じ規則です。法人番号のない同名企業は別企業なので、再取込すると重複して増えます。

## 書出の情報とファイル保護

書出全体は同じ読み取り時点にそろいます。`--include-personal` で個人事業の行を、`--include-excluded` で除外企業を含めます。消去済み企業、担当者、活動、メモは書き出しません。

既存ファイルは上書きせず、書出に失敗した一時ファイルは消します。Unixでは出力ファイルは所有者だけが読める権限で作ります。社名などが表計算ソフトの数式として実行されないよう、危険な開始文字にはアポストロフィを付けます。この保護は再取込でもそのまま残ります。

CSVは企業を移すためのスナップショットで、担当者・活動・次回対応を復元するバックアップではありません。完全な複製にはcoreのbackupを使います。CSVも個人情報を含む可能性があるため、プロジェクトでは既定でGitの対象外です。
