# 営業ワークスペースへ構成を改める

## 本人の指示と判断
2026-10-23、本人が現画面ではSalesforceと大きく異なり使われないため、もっと近づけるよう指示した。既存の「画面は似せない」という既定を更新し、Lightningの情報構造と操作配置を参照する。meiboの名称・自作コード・既存DBを保つ。配色だけの変更では受入条件を満たさない。

## ユーザーストーリーとUX5階層
営業担当が、ホームで本日の対応を把握し、取引先一覧の表示条件を切り替え、企業を開いて履歴と次の行動を一か所で確認・更新できるようにする。
戦略: 営業の作業場として使える。要件: 業務ナビ・一覧ビュー・独立したレコード画面・新規登録。構造: ホーム/取引先/次回対応と企業レコード。骨格: 共通検索とナビ、一覧ツールバーと密な表、レコード上部の主要情報、下部の詳細/関連と活動。表層: 白/薄い青灰/青の業務UI、13〜14px中心、控えめな角丸と罫線。モバイルは横幅に応じて列と領域を縦へ並べる。

## 受入条件
正本は `requirements.md` の要件18。以下にAPI・タスク・参照を具体化する。

## APIの追加契約
GET `/api/summary` に `status_counts:[0..5の件数]`、`scheduled`、`due` を追加する。既存のtotal/active/with_phone/originは維持する。
GET `/api/companies` に `view=all|untouched|negotiating|has_phone|scheduled|due` と任意の `status=0..4`、`prefecture=1..47`、`has_phone=true|false` を追加する。scheduled/dueはNextAction順、それ以外はShapeDefault。qと非allのview/絞込併用は400で明示的に拒否。countは先頭ページで一致件数、続きではnull可。消去済み/除外の扱いは既定を維持する。
POST `/api/companies`: `{name,prefecture,corporate_number?,phone?,website?}`。originはサーバーのSynthetic、状態はUntouched、個人事業ではない。成功201 `{id}`、重複409、不正400。既存のHTTP境界とcore検証を使う。今回のDBや既存行を再生成しない。

## タスクと受入の証拠
- [x] A. APIのビュー/絞込/集計/新規登録を追加し、実際の件数・重複・入力失敗・HTTP境界を回帰検証する。
- [x] B. 共通シェル・一覧・ホーム・全幅レコード・新規登録を実装する。既存APIの契約を引き継ぐ。
- [x] C. ブラウザでビュー/絞込/検索/ページ保持/レコードタブ/保存/登録/重複/CSVと3幅を確認し、画像で構成の変化を評価する。
- [x] D. DBを保持して同じローカルURLへ反映し、記録と再開メモを更新する。

完了: app20試験・全体clippy・Rust1.88型検査・fmt・gitleaksを通過。実ブラウザで全操作と通信失敗/通信遅延時の再試行・切替抑止を確認。独立UI18/20・体験18/20、0点なし。証拠は `.agent-harness/runs/2026-10-23-workspace-redesign/`。反映時の企業100件・活動57件を保持し、127.0.0.1:4317を新UIへ切り替えた。

## 参照（公式）
[リスト・ホーム・レコードの配置](https://trailhead.salesforce.com/content/learn/modules/lex_salesforce_basics/lex_salesforce_basics_getting_started)、[レコードページの構成](https://trailhead.salesforce.com/en/modules/lightning_app_builder/units/lightning_app_builder_recordpage)、[リストビューの操作](https://help.salesforce.com/s/articleView?id=xcloud.basics_navigate_list_views.htm&language=en_US&type=5)。2026-10-23確認。公式のロゴ・画像・コードを製品へ転用しない。
