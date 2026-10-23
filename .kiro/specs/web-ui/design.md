# 手元のデモを操作する画面とHTTPサーバー

最新の画面構成は本人の追加指示を反映した [workspace-redesign.md](workspace-redesign.md)。初回の一覧/サイド詳細から、共通ナビ・全幅一覧・独立したレコード画面へ改める。以下のAPI契約と通信境界は、その追加契約と併せて引き継ぐ。

## ユーザーストーリーとUX5階層
営業担当が、コマンドを覚えず企業を探して架電結果を保存できるようにする。受入条件はローカルURLから検索・詳細・保存が完結すること。
戦略: ローカルでまず触れる。要件: 一覧・検索・詳細・架電メモ・CSV書出。構造: 一覧と詳細の2領域、別ページへの遷移を減らす。骨格: 上に検索と件数、左に企業表、右に選択企業と記録フォーム。表層: 明るい背景・濃い文字・tealの操作色、システム書体、静かな罫線。スマホは詳細を一覧の手前に展開し、戻る操作を用意する。

## 実装境界
appに `server::serve(&Path,u16)` を追加し、tiny_http 0.12.0（SSL機能なし）でloopbackだけを受ける。core/importに通信を足さない。静的な `crates/app/ui/{index.html,style.css,app.js}` をinclude_strで同梱し、素のJSでfetchする。core保存形式を変えず、Engineを1スレッドで使う。

## API契約
- GET `/health`: `{ok:true}`。
- GET `/api/summary`: `{total,active,with_phone,origin:"synthetic"}`。totalは除外込み、active/with_phoneは除外抜き。
- GET `/api/companies?q=&after=&limit=50`: `{rows:[Company],next:string|null,count:number|null}`。通常一覧と社名検索で同じ形式。初回に件数、続きの検索ではcountはnull可。Companyはid/name/corporate_number/prefecture/phone/website/status/owner/last_activity_at/next_action_at/is_sole_proprietor。都道府県名はUIで変換する。
- GET `/api/companies/<id>`: `{company:Company,activities:[{id,kind,at,note,status_after}],contacts:[...]}`。個人情報の追加Fieldsは渡さない。消去済みは404。
- POST `/api/companies/<id>/activities`: JSON `{note:string,status?:0..5,next_action_at?:number|null}`。kindはCall、atはサーバーの現在時刻。next_action_at未指定はKeep、nullはClear。成功は `{ok:true}`。
- GET `/api/export`: 企業CSVを既定の安全なオプションで添付ダウンロード。出力は最大32MiBまで、超えたら明確に失敗。
- static `/`, `/style.css`, `/app.js`。そのほかは404。エラーJSONは `{error:string}` で入力値やDBの内部エラーを出さない。

## HTTPの境界
listenは127.0.0.1固定。Hostは127.0.0.1:portまたはlocalhost:portだけ。POSTはOriginとHostが一致し、`X-Meibo-Local: 1` とapplication/jsonを必須にする。最大body1MiB。CSPはselfのみ、nosniff、frame拒否、no-store。実データDB起動は拒否。UIはtextContentで値を表示する。

## 判断と検証
userの明示したローカル起動のため、従来の通信シンボル禁止テストの対象をcore/import/xtaskへ限定する。外部通信しない要件7.4はcoreに維持し、appの受信だけを別仕様で許可する。MCP・公開・ユーザー認証・本番運用は今回の対象外。call-modeのキーボード専用フローは後続とし、今回の記録フォームに依存させない。
ローカルキャッシュのtiny_httpとPlaywrightを再利用する。HTTPのorigin/Host拒否・不正入力・保存と再読込を試験し、実ブラウザで主要操作と3幅を確認する。新しいネットワーク依存をcoreには入れない。
