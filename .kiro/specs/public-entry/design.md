# Design: 公開ログイン入口

## Story and UX
招待された利用者が同僚へログイン入口を案内できるよう、認証情報を含まないサービスURLを共有する。

| 層 | 方針 |
|---|---|
| 戦略 | 招待済み利用者へ公開入口を案内する |
| 要件 | canonical/OGP、X/LINE/コピー/対応端末の共有 |
| 構造 | 既存ログインフォームを主、共有を補助操作とする |
| 骨格 | フォーム・管理者への案内の後に小さな共有領域 |
| 表層 | 既存の色・文字・カード・focus表現を継承する |

## Boundary Commitments
- Owns: hosted公開ログインのrender、汎用静的画像・共有JS/CSS、関連検証。
- Out: 認証/権限/業務画面の変更、外部投稿、signup、Cloudflare配備、DB。
- Dependencies: 既存HostedConfigの検証済みHTTPS originと既存HTTP境界。remote-storage 3.3の共有HTTPを使う。

## Implementation
`crates/app/src/hosted/public_entry.rs`を新設し、hosted.rsのmodule登録とhttp.rsの公開login/static asset応答へ組み込む。ui/login.html・login.cssと新しいpublic-entry.js・汎用PNG/編集可能な画像sourceを使う。login.jsの認証処理は維持する。
公開modeは既存のHTTPS hosted（development=false）。canonicalは`${origin}/login`、画像は固定public asset pathの絶対HTTPS URLとする。canonical・og:type=websiteを含むmetadataはJS実行前のHTMLへ出す。HTML値は安全にencodeし、要求Host/query・identity・DB内容をtemplateへ使わない。画像は第一者制作の汎用meiboカードで、顧客dataを入れない。
X/LINEはcanonicalをURL encodeしたリンクとし、noopener/noreferrerを付ける。自動送信しない。共有シートはnavigator.shareの対応時だけクリックから直接使い、キャンセルは通常終了、その他の失敗とコピー結果はaria-liveで通知する。Promise成功を投稿完了と表示しない。コピーは許可されたbrowser APIと適切なfallbackを使う。
既存CSPを弱めず、同一originのscript/imageを使う。汎用assetは既存のHost/method/排他内で返し、業務DB・cookie・接続先を画像やmetadataへ埋めない。開発用loopback画面には公開URL共有を出さない。通常standalone local画面は変更しない。

## Verification
実HTTP応答のcanonical/OGP/MIME・PNG実decodeと寸法、Cookie/query値でmetadata/画像が変わらないこと、Host拒否、developmentと通常local回帰を確認する。画像/JSは既存Host/method/gate検査内、session認証より前で匿名取得できる。
実browserで3幅、X/LINEのhref、コピー、共有シート対応/非対応・キャンセル/失敗、keyboard、操作前の外部通信なしを確認する。試験中に外部へ投稿しない。API模擬と実端末の共有操作、公開先のSNSプレビューは区別して記録する。
