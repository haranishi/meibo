# Roadmap

## Overview

営業リスト向けの高速なオープンソースCRM（仮名 meibo）を、小さな仕様に分けて作る。最初は list-engine。数百万件の一覧を速く返す土台で、速さが差別化の主張になるため、画面より先に、測れる形で成立させる。他の仕様は、この検索口を呼ぶだけにする。

## Approach Decision

- **Chosen**: 既存のOSS CRMは改造せず、Rustで一から作る。保存はSQLite 1ファイル。
- **Why**: 速さが売りなので、索引の設計から握りたい。ライセンスを MIT OR Apache-2.0 に選べる。予備測定（`.agent-harness/runs/2026-10-22-sqlite-spike/`）で、合成445万社でも一覧の先頭が即時に返ることを確かめた。
- **Rejected alternatives**:
  - Twenty・EspoCRM・SuiteCRMの改造: AGPL系で、改造すると公開の義務を引き継ぐ。数百万件での速さは未測定のまま、他人の構造に頼ることになる。
  - サーバー型DB（PostgreSQLなど）: 営業個人には導入の負担が重い。445万社なら1ファイルで足りることを測定で見た。

## Scope

- **In**: 企業・担当者・活動・リストの保存と一覧、CSVの取込と書出、架電モード、画面、ローカルAPIとMCP、チーム利用、公的データの取込器。
- **Out**: データの同梱、スクレイピング、メール・SMSの一斉送信、有料課金、公開signup、自動化エンジン、BI。

## Constraints

- 試験と測定は合成データだけ。実データは権利が確認できるまで接続しない（`docs/legal/rights-check.md`）。
- Rust、MIT OR Apache-2.0、DCOを採用する。2026-10-26の指示で残機能とソース一般公開へ進む。名称は仮称、商標未確認を明示し、パッケージ名の予約とは区別する。
- 外向けの文書に競合名を出さない。「高速」は測った数字だけで言う。

## Boundary Strategy

- **Why this split**: 速さの門（list-engine）を最初に通せば、後続の仕様は同じ検索口を呼ぶだけで済む。画面を先に作ると、遅さの原因が画面か保存かを切り分けられない。
- **Shared seams to watch**: 絞り込みと並べ替えに使える列の許可リスト（画面・API・CSVが同じものを使う）、個人情報の印、活動の追記規則。

## Specs (dependency order)

- [x] list-engine -- 企業・担当者・活動・リストの保存と、数百万件の一覧クエリ（絞り込み・並べ替え・ページ・件数・社名検索）、合成データの生成、速さの門。Dependencies: none
- [x] custom-fields -- 利用者が項目を足し、必要な項目だけ速さを宣言（索引を作成）できる。list-engine は固定の項目だけを持つ。Dependencies: list-engine
- [x] csv-import-export -- CSV（UTF-8・Shift_JIS）の取込と書出、法人番号での重複検出、巨大ファイルの逐次処理、ローカルCLI。Dependencies: list-engine
- [x] call-mode -- 活動の記録と次回対応、キーボードだけで進める架電モード。Dependencies: list-engine
- [x] web-ui -- 初回の一覧・詳細・架電記録・CSV書出画面とローカル起動は完了（2026-10-23）。取引先/担当者編集、活動種別/取消、独立ToDo/予定/商談、画面CSV取込、保存ビューは実装済み。専用架電モードはcall-modeで完了。Dependencies: list-engine, call-mode, csv-import-export
- [x] api-mcp -- 画面用のloopback APIは追加済み。版付きAPIとstdio MCPをapi-mcpで完了。Dependencies: list-engine
- [x] public-data-importers -- 国税庁の法人番号など公的データの取込器。データは同梱しない。Dependencies: csv-import-export
- [x] team-access -- 複数の利用者、権限、監査ログ。Dependencies: web-ui, api-mcp

## 次の一手

2026-10-26にpublic-data-importers・custom-fields・call-mode・api-mcp・team-accessを実装・統合検証した。既存の完了仕様を引き継ぎ、公開前に全体検証・依存/ライセンス/秘密検査と権利記録を更新する。

## 2026-10-26の提供境界
無料・招待制ベータを既定とし、組織別DB、認証/権限、監査、HTTPS前段とVolume/復旧を追加する。実企業データは別保存先に取り込み、Gitや配布物へ含めない。ソース公開と外部サーバーへのSaaS配備は別の完了条件とする。外部domain/公開先が未指定なら、配備済みと報告しない。
