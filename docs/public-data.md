# 国税庁カタログから取り込む

企業データをソフトへ同梱せず、利用者が取得・署名検証した国税庁カタログを読み取り専用で使います。出典は[国税庁法人番号公表サイト](https://www.houjin-bangou.nta.go.jp/)、利用条件は[同サイトの利用規約](https://www.houjin-bangou.nta.go.jp/riyokiyaku/)です。画面・CSV・API・MCPに出典と加工した旨を表示します。

## 対応するカタログ

`companies`の法人番号・legal_name・prefecture_code・source_batch_idと、`source_batches`の出典・完了状態・署名確認・基準日を使います。許可するsource_typeは`nta_monthly_all`と`nta_daily`です。未確認・NULL・未完了の出典を拒否します。署名そのものの検証はカタログ作成側の責任で、この取込器は確認済みメタデータを検査します。

`daily_changes`がある場合は、月次companiesから差分対象を除き、未削除の差分を重ねます。日次の追加・更新・削除と`daily_parts`の出典記録を検査します。companiesだけの件数を最新の実効件数とは扱いません。

しょくばらぼ由来の業種・電話・HPや、供給元の住所全文は取り込みません。電話・Webサイト・業種・担当は、利用者が別途適法に入力したものを保持します。

## 取込と再実行

```sh
meibo init real.db --origin real
meibo sync-nta real.db catalog.sqlite3
meibo serve real.db --allow-real
```

`--limit 1000`で取込上限を指定できます。法人番号が同じ企業は名称・都道府県・出典・基準日だけを更新し、活動・営業状態・担当・次回対応・商談・独自項目を変更しません。消去済み番号はhash記録により再作成を防ぎます。古い基準日の入力はスキップします。

500件までのバッチを原子的に確定します。途中失敗では、それ以前のバッチは残ります。エラーのrowと確定済みinserted/updated/skippedを確認して再実行してください。業務データの複製は[復旧手順](hosting.md)に従います。

## 招待制サービスの組織へ取り込む

```sh
meibo hosted SERVICE_ROOT sync-nta ORGANIZATION catalog.sqlite3
```

保守者のCLIで登録組織を確定し、Web処理/管理と同じ保守lockの下で取り込みます。取込中のWeb要求は503になる場合があります。結果は監査へ記録します。CATALOGをコンテナへ渡す場合は読み取り専用にmountし、公開配信フォルダへ置かないでください。

供給元から消えた企業を既存CRMから自動削除することはありません。初回取込の対象は最新の実効名簿、再取込は営業記録を保つ更新です。
