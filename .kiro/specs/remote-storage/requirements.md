# Requirements: 公開版の永続保存

## Introduction
公開版は外部DBを正本とし、サーバーの停止・再作成後も認証と営業情報を引き継ぐ。手元で使うOSS版の保存形式・公開API・通信しない動作は維持する。

## Boundary Context
- In: 保存接続、既存の業務処理、招待制認証、組織分離、管理の排他、保存と復旧、合成データでの検証。
- Out: Cloudflareへの配備・料金変更、外部ログイン、signup、実企業データの移動。
- Adjacent: cloudflare-hostingが認証情報の受渡しとHTTPS入口を担当する。既存team-accessの権限・失効・HTTP境界は引き継ぐ。

## Requirements

### Requirement 1: 手元版と公開版を区別する
1.1 Where 公開版の保存機能を選ばない場合, the meibo shall 既存の保存形式・公開API・通信しない動作を維持する。
1.2 When 公開版を起動する, the meibo shall 設定した外部DBと保存形式の識別・版・データ種別を確認する。
1.3 If 接続設定・認証・保存形式・組織対応が不明または不正である, the meibo shall 起動または対象操作を拒否し、空の手元DBへ切り替えない。
1.4 When 公開版サーバーを停止・再作成する, the meibo shall 同じ外部DBの利用者・所属・営業情報を引き継ぐ。

### Requirement 2: 業務処理と失敗時の確定を保つ
2.1 Where 公開版を使う場合, the meibo shall 一覧・件数・社名検索・担当者・活動・取消・リスト・独自項目・ToDo・予定・商談・CSVの既存動作を維持する。
2.2 When ひとまとまりの更新が成功する, the meibo shall 活動・現在状態・件数・検索索引を同じ確定に含める。
2.3 If 更新途中で制約違反が起きる, the meibo shall 全体を取り消して更新前の状態を保つ。
2.4 If 通信断等で書込みの確定結果を確認できない, the meibo shall 自動再送せず、結果不明を通知して当該接続を再利用しない。
2.5 While 管理者による確認が必要な結果不明の操作が残る, the meibo shall 次の保護対象操作を拒否する。
2.6 When 外部DBへ要求する, the meibo shall 待機・応答サイズ・読み取りの作業領域に上限を設け、認証情報・接続URL・SQL本文を利用者向けエラーやログへ出さない。

### Requirement 3: 認証・組織分離と管理の排他を保つ
3.1 Where 公開版を使う場合, the meibo shall 既存のパスワード保護、session期限・失効、3権限、監査、Host・Origin・CSRF・context検査を維持する。
3.2 When 認証した利用者の業務DBを選ぶ, the meibo shall サーバーが登録した所属とDB識別の対応だけを使い、要求から接続先を選ばせない。
3.3 When Web処理・管理CLI・保存・復旧を実行する, the meibo shall サービス全体で共通の排他を取り、別プロセスの操作も直列化する。
3.4 If 保護対象操作が停止・通信断・結果不明のまま終わる, the meibo shall 排他を自動解除せず、管理者が以前の処理の終了を確認するまで取得を拒否する。
3.5 When 管理者が排他を復旧する, the meibo shall 所有情報を照合し、以前の処理を終了した明示指定を要求して復旧結果を監査する。

### Requirement 4: 復旧できる保存を作る
4.1 When 管理者が保存する, the meibo shall 共通の排他中に認証DBと登録された全組織DBの同じ時点の論理内容を保存する。
4.2 When 保存が完了する, the meibo shall 件数・形式・整合性・hashを検証した目録を作り、sessionと実行中の排他と接続秘密を含めない。
4.3 When 管理者が復旧する, the meibo shall 明示された未使用の新しいDB一式へ復旧し、既存DBを上書きしない。
4.4 When 復旧が成功する, the meibo shall 検索索引・件数・活動からの状態・組織分離を照合し、sessionを失効させ、新しいサービス識別を発行する。
4.5 If 保存物の改変・不正な形式・既存の復旧先・途中の失敗を検出する, the meibo shall 成功を返さず、元DBを保持し、不完全な復旧先を利用開始しない。

### Requirement 5: 公開版の保守範囲を明示して検証する
5.1 When 公開版で個人情報を消去する, the meibo shall 既存の論理消去・索引更新・再作成防止を維持する。
5.2 If 手元ファイル専用の保守を公開版へ要求する, the meibo shall 対応外と明示し、外部提供者の保存領域や過去の保存物を消去したと報告しない。
5.3 When 公開版の検証を行う, the meibo shall 実際の互換DBへ接続した合成の2組織・3権限・保存復旧・再起動・通信障害の証拠を残す。
5.4 When 公開版の変更を統合する, the meibo shall 手元版の既存試験・最低対応版・品質検査と公開版の追加試験を成功させ、未実施の本番確認と区別する。
