# meibo（仮名）— 営業リスト向けの、速い CRM

> 状態: 開発中で、リポジトリは非公開です。名前は仮です。商標とパッケージ名の確認が済むまで外部公開しません。

営業リストから新規開拓する個人や小チームのためのCRMです。Rustの保存・検索ライブラリ、CSV取込・書出のCLI、合成デモを操作するローカル画面があります。SQLiteの1ファイルで動きます。検索・記録の性能は、下記の合成データによる測定で確認しています。

## 試してみる

```sh
cargo run -p meibo-app -- demo demo.db --rows 100
cargo run -p meibo-app -- serve demo.db --port 4317
```

表示された `http://127.0.0.1:4317` をブラウザで開くと、ホーム・取引先・次回対応を切り替え、一覧の表示条件・取引先レコード・架電記録・新規登録・CSV書出を試せます。画面は合成デモDBだけで起動します。[起動と停止](docs/local-ui.md)。CLIでも操作できます。

```sh
cargo run -p meibo-app -- list demo.db --limit 20
cargo run -p meibo-app -- export demo.db companies.csv
cargo run -p meibo-app -- verify demo.db
```

CSVの見出し・文字コード・取込が途中で止まった場合の扱いは [CSVの使い方](docs/csv.md) にあります。

## 速さの考え方

速さを約束する絞り込みの形を22個に決めました。形ごとに索引を固定し、実行計画の検査と測定の両方で守ります。形の外の条件も使えますが、結果に「遅い条件」の印が付きます。理由と捨てた案は [docs/architecture.md](docs/architecture.md)。

- 目標値は [要件](.kiro/specs/list-engine/requirements.md)、実測は [docs/benchmarks.md](docs/benchmarks.md) にあります。実測は測定の記録から自動で作り、記録にない数字は文書に書けません。
- 目標に届かない条件は、緩めずにそのまま載せます。要件を変えたときは、経緯を [amendments.md](.kiro/specs/list-engine/amendments.md) に残します。
- 測定は合成データです。実データの性能ではありません。

## データと個人情報の扱い

- データは同梱しません。試験と測定は合成データだけを使います。合成の法人番号は、国税庁が未使用としている番号帯から作ります。
- 保存先に「合成」か「実データ」かを記録し、食い違う追加を断ります。
- 担当者の氏名・連絡先と、個人事業の名称・電話・ホームページは、取り出すときに明示して選ばない限り出ません。消去の求めに応じて消せます。保存ファイルから物理的に消すのは `compact` です。移行前の複製には消去が及びません。
- 外部への送信は行いません。ローカル画面は127.0.0.1でHTTPを受信します。利用状況の送信も更新の確認もしません。
- 保存先を開くと、WAL の書き戻し専用のスレッドを1本起動します（`OpenOptions` の `background_checkpoint` で止められます）。書き込みの呼び出しから重い書き戻しを外すためです。
- 電源断では直近の確定を失うことがあります。詳しくは [DISCLAIMER.md](DISCLAIMER.md)。

## まだないもの

画面からのCSV取込、専用の架電モード、汎用API・MCP、認証と権限、利用者が定義する項目、企業の統合。実データは接続していません。メールなどを送信する機能は対象外です。

## 開発

```sh
cargo test                                    # 全試験（数分かかります）
cargo xtask check-all                         # fmt・clippy・試験・リンク・依存の検査・要件の追跡
cargo xtask-release bench --profile dense     # 速さの門（445万社。最適化ビルドだけ）
```

Rust 1.88以上が必要です。開発・CIは1.99.0、最低対応版は1.88.0で確認します。初回はCargoが依存を取得します。取得後は `--offline` で実行できます。検査と貢献の手順は [CONTRIBUTING.md](CONTRIBUTING.md) にあります。

## 文書の地図

| 知りたいこと | 場所 |
|---|---|
| 要件・設計・タスク | [.kiro/specs/list-engine/](.kiro/specs/list-engine/) |
| CSV・CLIの使い方と仕様 | [docs/csv.md](docs/csv.md)、[.kiro/specs/csv-import-export/](.kiro/specs/csv-import-export/) |
| ローカル画面の起動 | [docs/local-ui.md](docs/local-ui.md)、[.kiro/specs/web-ui/](.kiro/specs/web-ui/) |
| 設計の判断と捨てた案、既知の限界 | [docs/architecture.md](docs/architecture.md) |
| 実測 | [docs/benchmarks.md](docs/benchmarks.md) |
| データの権利の確認 | [docs/legal/rights-check.md](docs/legal/rights-check.md) |
| 検証結果 | 各仕様の `tasks.md`、[測定記録](docs/benchmarks/latest.json) |

過去の文書に出てくる `.agent-harness/` はローカルの実行記録です。生ログ・作業コピー・調査資料はリポジトリに含めません。

## ライセンスと貢献

コードは MIT OR Apache-2.0、文書は CC BY 4.0 です。貢献は DCO（コミットに `Signed-off-by`）で受けます。脆弱性の報告は [SECURITY.md](SECURITY.md) を参照してください。
