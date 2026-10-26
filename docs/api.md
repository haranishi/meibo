# 営業API v1

画面と同じ企業検索・詳細取得・活動記録を、版付きの入口で利用できます。レスポンスはJSONです。
ローカルの `serve` は127.0.0.1専用です。hostedではログインした利用者の組織と権限を共通入口で検査します。
APIの引数からDBパスや組織を選ぶことはできません。独立したAPIキー認証は提供していません。

## 一覧と社名検索

`GET /api/v1/companies`

| 条件 | 意味 |
|---|---|
| `limit` | 1〜200、既定50 |
| `after` | 前の応答の `next` をURLエンコードして指定 |
| `q` | 社名の部分一致。ビュー・絞込・明示ソートとの併用不可 |
| `view` | `all/untouched/negotiating/has_phone/scheduled/due` |
| `status` | 営業状態0〜4 |
| `prefecture` | 都道府県コード1〜47 |
| `has_phone` | `true/false` |
| `sort` | `default/next_action/last_activity` |

応答は `{rows, next, count}`。続きが無ければ `next` はnullです。通常一覧のcountは先頭ページだけに付きます。
条件を変えるときはafterを捨てて先頭から取得します。未知条件、重複条件、不正な値は400です。
公的データ由来の企業は `source_id/source_as_of/attribution/modification_notice` で供給元・基準日・出典と加工の表示を返します。

## 企業詳細

`GET /api/v1/companies/{正整数ID}`

応答は `{company, contacts, activities, contacts_revealed}`。消去済みの企業は404です。
担当者連絡先は既定で非表示です。必要な場合だけ `include_contacts=1` を明示します。
個人事業の名称・電話・URLも既定で非表示で、必要な場合だけ `include_private=1` を明示します。
この応答には利用者が登録した活動メモが含まれます。共有先のアクセス権を確認して利用してください。

## 活動の追記

`POST /api/v1/companies/{正整数ID}/activities`

必須 `note` は空白だけを除く1〜10,000文字です。任意項目は次のとおりです。

| 項目 | 契約 |
|---|---|
| `kind` | 架電1、訪問2、メール記録3、メモ4 |
| `status` | 営業状態0〜5。除外からの解除はこの入口では不可 |
| `next_action_at` | 省略=保持、null=解除、整数UNIX秒=設定 |
| `contact_id` | 同じ企業の未消去の担当者ID、またはnull |
| `at` | 整数UNIX秒。省略時は現在時刻 |

ローカルでは `Content-Type: application/json`、`Origin: http://127.0.0.1:4317`、`X-Meibo-Local: 1` が必要です。
hostedでも同一Originと権限検査を維持します。JSONは1MiBまでです。
成功は200と `{ok:true}`。活動は追記されるため、**同じ要求を再送すると別の記録になります**。
応答を受け取れなかった場合、自動再送せず企業詳細の履歴で保存結果を確認してください。

## エラー

`{error: "説明"}` を返します。400=入力、403=アクセス境界/権限、404=対象なし、405=未対応操作、409=業務上の競合、413=容量、500=処理失敗です。
保存エラーの内部パスやSQLは返しません。v1の追加は既存の画面用APIを変更しません。
