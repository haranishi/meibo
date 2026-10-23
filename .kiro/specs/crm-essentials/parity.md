# 営業の日常操作の比較と不足

調査日: 2026-10-23。本人の「Salesforceと機能/UIを見比べ、不足を足す」という指示の実装用メモ。比較対象はLightningの取引先・取引先責任者・活動・商談・リスト操作。全製品・全エディションの同等性を約束するものではない。公式資料が示す操作と、現コードでできる操作を区別する。

## 現在地と優先順位

ホーム、業務ナビ、密な一覧、主要情報、詳細/関連タブ、活動欄という画面の骨格は実装済み。次は「開いて確認できる」から「日常の仕事を完了できる」へ進める。P0は最初の実用範囲、P1はそれに続く操作性改善。

| 領域 | 公式資料の基本操作 | meiboの実装状態 | 追加範囲・依存 | 優先 |
| --- | --- | --- | --- | --- |
| 取引先 | 新規・編集、関連担当者・活動を一か所で扱う [S1] | 新規と閲覧あり。coreの`update_company`はあるが編集API/画面なし | 主要情報の「編集」、検証、重複エラー、保存後反映。状態変更は活動経由を維持 | P0 |
| 担当者 | 取引先に属する連絡先を登録・参照 [S1][S2] | 関連欄はマスクした閲覧のみ。coreは追加/消去あり、更新なし | 追加・編集・消去と対象企業の検査。個人情報は明示取得し、既定マスクと消去規則を維持 | P0 |
| 活動入力 | 架電・タスク・予定等をタイムラインで確認 [S3] | coreは架電/訪問/メール記録/メモ/状態変更/取消。APIは架電に固定 | 種類・対象担当者・日時を選択。取消は削除でなく既存取消活動を追記。メールは送信せず実施記録 | P0 |
| ToDo | 期限を持つタスク、完了、今日/期限超過 [S3][S4] | 企業ごとの次回対応日時だけ。件名・独立ID・完了履歴なし | 独立タスクを新設し、企業に複数登録、完了/再開、期限順一覧とホームへ反映。schema移行が必要 | P0 |
| 予定 | 会議等の予定を活動と合わせて表示 [S3] | 開始/終了・件名を持つ予定なし | 予定を新設し、企業への関連、開始/終了、編集、予定一覧/カレンダー。外部同期・繰返しは後続 | P1 |
| 商談 | 企業に紐づく商談名、金額、完了予定日、段階を更新 [S5] | 企業の「商談/受注」statusのみ。商談レコードなし | 独立商談を新設し、企業に複数登録、一覧/詳細/編集。金額・通貨・段階・日付の型と集計規則を定義 | P0 |
| 商談の進捗 | Pathと段階別Kanban、カード移動で段階更新 [S6] | 企業の状態を表示するPathのみ。Kanbanなし | 商談を作った後に段階別ボード、件数/金額集計、カードとキーボード両方の移動。保存失敗時に表示を戻す | P0 |
| CSV取込 | ファイル選択、項目対応、追加/更新の指定 [S7] | CLIの逐次取込は完成。画面は書出だけ。既存行更新は非対応 | まず固定13列のテンプレート/文字コード/重複方針/結果/確定済み件数を画面化。列対応や更新取込は別仕様 | P0 |
| 保存ビュー | 条件に名前を付けたリストビューを作成 [S8] | 固定プリセットだけ。coreのlistは作成時のIDを固定する別機能 | 動的な条件・列・順序の保存/復元/名前変更/削除。個人のローカル設定から開始し、固定リストと混同しない | P1 |
| 一覧の列・ソート | 表示列の選択と列見出しで並べ替え [S8] | 固定8列。coreの順序は既定/次回対応/最終活動の3種類 | 既存3順序のAPI/画面化、列の表示切替・並び順保存。任意列ソートは索引/カーソル/性能を先に設計 | P1 |
| 検索と絞込 | リストの検索/条件で対象を狭める [S6][S8] | 社名検索と絞込は併用不可。切替時に解除を通知 | 併用するならcore検索へ条件を追加し、件数/ページング/性能を検証。画面だけで現ページを絞らない | P1 |
| ホームと関連情報 | 今日の予定/タスク、最近の情報、関連商談等 [S3][S9] | 状態件数・次回対応・最近開いた企業あり | 新しいタスク/予定/商談の実レコードへ接続。機能追加に合わせナビと関連タブを拡張し、架空の売上は置かない | P0 |

## 推奨するPRの区切り

1. **取引先と担当者の更新**: 企業編集、担当者の追加/編集/消去、明示取得、API境界とフォーム。既存情報を誤って空値で上書きしない検証を含む。
2. **活動と仕事の予定**: 活動種別/担当者/取消、独立ToDoの期限・完了・再開、予定の開始/終了、今日/期限超過・カレンダー。新domainのschema移行と既存DB保持を検証する。
3. **商談と進捗**: 商談の保存・一覧/詳細/編集、企業の関連商談、Path/Kanban、段階更新・金額集計。企業statusを商談レコードの代わりに使わない。
4. **画面からのCSV取込**: 既存取込器の再利用、容量と同一Originの制限、確定/失敗件数、途中失敗の案内。アップロードのために巨大ファイル全体をメモリへ載せない。
5. **一覧の作業効率**: 保存ビュー、対応済みソート、列設定、検索/条件の明確な状態表示。検索併用・任意列ソートは性能根拠を揃えてから有効化する。

各PRで仕様→実装→該当テスト/ビルド→実ブラウザ→commit/push/PR/merge→反映確認を閉じる。大きければ同じ領域を保存層/画面に分割する。新domainは既存schema version 1からの移行を用意し、同じデモDBを再生成しない。

## 今回の境界と後続

- 先にローカルの単一利用者で営業の基本操作を成立させる。MCP・チーム認証/権限・共有ビュー・外部メール送信・外部カレンダー同期は別仕様。顧客窓口、商品/見積/請求、承認/自動化、自由なレポート、任意項目もこの最初の範囲へ混ぜない。
- 連絡先を既定で一律表示しない。企業の除外/消去、活動追記、CSV保護、loopback限定、入力のHTML無解釈を各機能へ引き継ぐ。
- 企業数拡大は元の`company-data-platform`の別作業。合成デモの件数追加では達成としない。出典・法人番号の重複・基準日と実件数を照合し、CRMへの実データ接続は別判断。
- 比較は機能と操作構造の参考。名称/ブランドはmeibo、自前の実装とし、外向けREADMEへ競合比較を転記しない。

## ローカル実装の根拠

- [Engine](../../../crates/core/src/engine.rs): `update_company`、`add_contact`、`erase_contact`、`record_activity`、`create_list`。担当者更新・商談/タスク/予定のAPIはない。
- [型](../../../crates/core/src/types.rs): `ActivityKind`、`ContactInput`、`NextAction`、`Sort`、`Filters`、条件を持たない`SearchOptions`。[schema](../../../crates/core/src/schema.rs): version 1。
- [HTTP](../../../crates/app/src/server.rs)・[API](../../../crates/app/src/server/api.rs): 公開経路と架電固定の入力。[UI](../../../crates/app/ui/app.js): プリセット、固定表、関連担当者、活動入力。
- [現UI仕様](../web-ui/requirements.md)・[再構成](../web-ui/workspace-redesign.md)・[CSV仕様](../csv-import-export/requirements.md): 今回拡張する契約。調査はコード読取であり、新機能の実装/受入を意味しない。

## 一次資料

- S1 [Work with an Account Record](https://trailhead.salesforce.com/content/learn/modules/account-record-view-step-by-step/work-with-an-account-record)
- S2 [Business Accounts and Contacts](https://trailhead.salesforce.com/content/learn/modules/accounts_contacts_lightning_experience/accounts-and-contacts-lightning)
- S3 [Help Your Reps Use Activities](https://trailhead.salesforce.com/content/learn/modules/sales_admin_maximize_productivity/sales_admin_maximize_productivity_unit_1)
- S4 [Complete Tasks and Events](https://help.salesforce.com/s/articleView?id=000385307&language=en_US&type=1)
- S5 [Explore Sales Features](https://trailhead.salesforce.com/content/learn/modules/lex_implementation_basics/lex_implementation_basics_explore)
- S6 [Visualize Success with Path and Kanban](https://trailhead.salesforce.com/content/learn/modules/leads_opportunities_lightning_experience/visualize-success-with-path-and-kanban)
- S7 [Import Data](https://trailhead.salesforce.com/content/learn/modules/lex_implementation_data_management/lex_implementation_data_import)
- S8 [Create and Customize List Views](https://trailhead.salesforce.com/content/learn/modules/lex_customization/lex_customization_list)
- S9 [Get Started with Salesforce](https://trailhead.salesforce.com/content/learn/modules/lex_salesforce_basics/lex_salesforce_basics_getting_started)
