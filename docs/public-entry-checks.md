# 公開ログイン入口の確認

`cargo test -p meibo-app --test public_entry --locked` は、独立した合成fixtureの実HTTPでmetadata、匿名PNG/JS、query/Cookie不変、Host/method/CSRF/保守中の拒否、development/通常localの回帰を確認する。

既存のPlaywrightを使える環境では、次の実ブラウザ確認を追加する。`MEIBO_BINARY` は検証するビルドを指す。Node、Chromium付きPlaywright、OpenSSLが必要で、製品への依存追加はない。

```sh
cargo build -p meibo-app --locked
MEIBO_BINARY="${CARGO_TARGET_DIR:-target}/debug/meibo" \
PLAYWRIGHT_MODULE=/path/to/playwright/index.mjs \
node crates/app/tests/public-entry-browser.mjs
```

試験は専用のloopback HTTPS前段と合成の招待制サーバーを起動する。自己署名証明書の例外はこのブラウザcontextだけに適用し、システムの証明書やSDKのTLS検証は変更しない。既存のサーバー・実DBには接続しない。

390/768/1440pxの画像と結果を `.agent-harness/runs/public-entry-browser/` へ保存する。`MEIBO_BROWSER_OUTPUT` で保存先を変えられる。実Clipboard API、選択コピーへのfallback、keyboard、390×844pxのtouch模擬、実ログイン、PNGの実decode、操作前の外部通信なしを確認する。

共有シートの成功・取消・失敗・非対応はAPI模擬で確認する。実端末の共有シート、公開URLのSNSプレビュー、SNSへの投稿はこの試験には含まれない。X/LINEのリンクは読み取りだけで、外部requestは試験中に遮断する。

汎用画像の編集元は [public-entry.svg](../crates/app/ui/public-entry.svg)。1200×630px、device scale 1のブラウザで表示し、同名PNGへ書き出す。外部フォント・素材は使わない。

LINEの共有URLは [公式のcustom button形式](https://developers.line.biz/en/docs/line-social-plugins/install-guide/using-line-share-buttons/#using-custom-icons) を使う。外部のwidget scriptは読み込まない。
