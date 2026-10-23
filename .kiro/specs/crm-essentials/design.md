# 実装設計

## 保存とAPI

既存Engineの取引先更新・活動・担当者追加/消去を再利用し、担当者更新を追加する。更新時は既存レコードを読み、画面で編集する許可項目だけを置換する。状態変更は活動の既存経路を使う。担当者明示取得は専用のGET queryで要求し、既定の詳細取得を変えない。

ToDo/予定/商談は独立IDとcompany_idを持つSQLite追加テーブルとし、既存v1に後方互換の移行を用意する。外部キーと企業の消去状態を毎回検査する。会社消去時の関連個人情報の保持を避ける。件名はtrim後1〜200文字、時刻は0〜253402300799のUNIX秒、商談の完了予定は実在するYYYY-MM-DD、金額は0〜9007199254740991の整数円。段階は見込み/評価/提案/交渉/受注/失注の0〜5。APIは既存同一Origin境界内に置く。

合成DBのcreate/openで同一トランザクション内に追加テーブル/索引/triggerを作成し、独立したmetaの`crm_schema_version=1`で拡張版を管理する。初回拡張はDDL前に既存SpaceProbeで容量を確認する。基盤の`schema_version`/`user_version`は1を維持し、旧アプリが読む既存列を変更しない。未来のCRM版はDDL前に拒否。実データDBへ追加しない。企業または関連担当者の消去triggerで関連件名/商談名を空にし、商談金額を0、関連contactをNULL、erased_atを設定する。company/contactの索引を用意し、消去前見積に該当CRM行数・旧テキスト量と索引/WALの余裕を加算する。論理消去された関連レコードは返さず、更新も許可しない。backupは追加表も含むSQLiteオンライン複製を継承する。

GET/POST `/api/work-items`、POST `/api/work-items/{id}`、POST `/api/work-items/{id}/completion`。taskはdue_at必須、eventはstart_at/end_at必須（終了>開始）で完了不可。GET/POST `/api/opportunities`、POST `/api/opportunities/{id}`、POST `/api/opportunities/{id}/stage`。更新でcompany_idと仕事kindは変更不可。一覧はcompany_id、仕事kind/completed、limit=1〜200とafter=正整数IDを受け、日時/完了予定日+ID順に`{items,total,next}`を返す。afterは同じ条件のレコードに限定する。

CSVは同一Origin専用経路で最大32MiB、既存importerを再利用する。通常JSONの1MiB制限は維持する。全件のブラウザ文字列化や無制限読込を避ける。途中失敗時に既確定バッチが残る契約を表示する。

## 画面

既存のナビと取引先詳細に編集と関連レコード操作を追加する。ToDo/予定/商談は専用ワークスペースと、取引先から対象を保持した作成フォームを用意する。商談ボードの段階変更はselect/buttonで操作できる。新規フォームを閉じる・保存失敗・通信遅延でも意図しない画面遷移や下書き喪失を起こさない。

保存ビューはlocalStorageの版付き設定として条件/順序/列だけ保存し、連絡先や活動メモを保存しない。不正な設定は既定へ戻す。大規模企業データを画面側へ一括取得して並べ替えない。

## 検証と反映

coreの取引先/担当者整合性、新domainの入力・再開・段階変更・再open・消去を検証する。HTTPのOrigin/Host/サイズ/未存在/他社を確認。ブラウザは取引先編集→担当者→活動→ToDo完了→予定→商談段階→CSV→保存ビュー→reloadの流れ。旧デモDBをonline backupし、合成stagingで先に動作確認、最後に4317へ反映する。

初回PRは既存土台とCI、その後変更単位ごとにPRを閉じる。収集の実行はCRMが終わってから始める。原企業DBは独立SSD上の正本を復旧し、NTA日次差分を別仕様で扱う。
