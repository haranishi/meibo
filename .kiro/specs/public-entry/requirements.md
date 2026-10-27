# Requirements: 公開ログイン入口

本人の公開Webアプリ共通ルールに従い、招待制meiboのログイン入口にmetadataとサービスURL共有を用意する。
認証・業務データ・外部投稿の自動実行は追加しない。

## Requirements
### Requirement 1: 公開metadata
1.1 When 公開HTTPSのログイン画面を返す, the meibo shall 設定済みoriginだけを基にcanonicalとOGP/title/description/url/imageおよびTwitter cardを返す。
1.2 The meibo shall 汎用のmeibo画像を同一originから正しい画像形式とMIMEで返し、metadataへ組織・利用者・顧客・session・要求queryの値を含めない。

### Requirement 2: サービスURLの共有
2.1 The meibo shall X・LINE・リンクコピーを利用者の明示操作で使え、対応端末では標準共有シートを使えるようにする。
2.2 The meibo shall 共有先を固定canonical URLだけにし、非対応の共有シートを隠すか無効化し、コピー失敗・共有失敗・キャンセルを適切に扱う。
2.3 The meibo shall InstagramやYouTubeへの直接投稿を表示せず、共有をアカウント発行や招待送信と誤表示しない。

### Requirement 3: 境界と操作性
3.1 The meibo shall 既存のHost/Origin/CSRF/context・認証・CSP・排他を維持し、通常ローカル画面で外部通信を自動実行しない。
3.2 The meibo shall 390/768/1440pxの幅で共有操作とログインを使え、キーボード操作と結果通知を用意する。
