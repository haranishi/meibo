# Requirements: 利用量と費用の目安

## Introduction
運用者が一人で、公開版のどの機能がどれだけ使っているかと、費用の目安を確かめられるようにする。

## Boundary Context
- In: 遠隔保存の公開版の利用量の集計、管理者だけの画面とAPI、推定額と無料枠の使用率。
- Out: 手元版とローカルSQLiteのhosted版、請求額そのものの取得、提供者APIの呼び出し、利用者ごとの課金。

## Requirements

### Requirement 1: 集計
1.1 Where 遠隔保存の場合, the meibo shall 要求を機能の群（ログイン、画面と部品、一覧と検索、取引先の詳細、記録と編集、予定と商談、取り込みと書き出し、受付、管理、拒否、その他）に分け、日（日本時間）と群ごとに、要求数、処理時間、保存先との往復数、読み取り行数、書き込み行数を数える。
1.2 The meibo shall 利用者名・取引先の内容・検索語・URLの問い合わせ文字列・IPアドレス・session を集計に含めない。
1.3 When 集計を保存する, the meibo shall 排他つきの経路で、60秒に1回以下にまとめて書く。排他を取らない読み取りでは書かない。
1.4 If 集計の保存が通信以外の理由で失敗する, the meibo shall 利用者の操作の結果を変えず、その分を捨てる。通信の失敗は既存の結果不明の扱いとする。
1.5 The meibo shall 保存前の集計が停止で失われ得ることを画面に示す。
1.6 The meibo shall 集計の保存そのものの往復と行数を、利用量に含めない。

### Requirement 2: 保存の用意
2.1 When 新しい control を初期化する, the meibo shall 利用量の表を作る。
2.2 Where 既存の control に表がない場合, the meibo shall 画面に未設定と表示し、他の操作を変えない。表は管理CLIの明示した命令で、排他の中で作る。
2.3 The meibo shall 利用量の表を保存（backup）の対象に含めず、復旧先には空の表を作る。表があっても保存と復旧は成功する。
2.4 The meibo shall control の準備確認の条件を変えない。

### Requirement 3: 画面とAPI
3.1 While 利用者が管理者（admin）の場合, the meibo shall 今月と直近31日の群ごとの集計、日ごとの推移、推定額、無料枠の使用率を返す。保存前の集計も含める。
3.2 If 管理者でない利用者が求める, the meibo shall 拒否して監査する。
3.3 The meibo shall 既存の API と同じ Host・context・CSRF の検査を使い、データの応答は保存禁止にする。
3.4 The meibo shall 推定額を「処理時間から計算した目安で、請求額ではない」と示し、通信量・起動の時間・同じ請求先の他の利用を含まないことを示す。
3.5 The meibo shall 単価と無料枠を、確認日とともに1か所で定義する。
3.6 Where 起動時に月の上限額（円）が設定されている場合, the meibo shall 今月の推定額が上限に占める割合を示す。
3.7 The meibo shall 画面を外部のサービス・スクリプト・書体なしで表示し、スマホの幅でも指で操作できるようにする。
3.8 The meibo shall 管理者にだけ、業務画面から利用量の画面へ移る入口を出す。

### Requirement 4: 検証
4.1 When 変更を検証する, the meibo shall 手元の試験用DBで、群の分類・保存・権限・表の有無・保存と復旧を確かめる。
4.2 When 画面を検証する, the meibo shall 手元のサーバーと合成データで、幅390pxと1280pxの表示を撮って確かめる。
