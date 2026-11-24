# 権利と規約の確認記録

更新日: 2026-10-26。調べていない点は「未確認」と書き、問題なしとは書かない。この表は法律の判断ではなく、専門家の確認は受けていない。ソース公開とサービス配備を分けて確認する。

| # | 項目 | 確認した内容 | 状態 | 次の一手 |
|--:|---|---|---|---|
| 1 | ライセンス | コードは MIT OR Apache-2.0、文書は CC BY 4.0、貢献は署名つきコミット（DCO）。 | 決定済み | 公開前に LICENSE と REUSE の整合を確認 |
| 2 | 製品名 | 「meibo」は仮称。crates.ioのmeibo/core/app/importは取得時点で未使用、GitHub同名repoは複数。J-PlatPatは公式メンテナンス画面となり第9/42類の商標照合は未完了。 | 仮称・未確認事項あり | 正式ブランド/パッケージ公開前に商標を再確認。未使用は予約や商標許可を意味しない |
| 3 | 他社の商標 | 競合の製品名を、製品名・説明・文書に使わない。 | 方針 | 公開前に文書を目視で確認 |
| 4 | 競合のコードと画面 | 公開された公式資料から一般的な機能・情報構造・操作配置を確認し、自前のHTML/CSS/JSで実装する。他社のコード・ロゴ・画像・文章は取り込まない。GPL/AGPL系CRMのコードは読まず、写さない。 | 更新済み（2026-10-23） | PRで実装と参照元を確認 |
| 5 | 個人情報 | ローカルに加えて招待制hostedを追加。組織別DB、認証/3権限、既定マスク、管理者の消去、再取込防止hashを実装。複製へ消去は及ばない。 | 機能と合成試験を確認 | 実際の提供者は利用目的・保存期間・窓口を定める。専門家確認は未実施 |
| 6 | 営業活動の法規 | メールの一斉送信は特定電子メール法、消費者向けの電話勧誘は特定商取引法（再勧誘の禁止ほか）が関係しうる。このソフトはメール・SMSの送信機能を持たず、除外（連絡停止）の状態を持つ。適合は利用者の責任。 | 未確認 | 専門家の確認 |
| 7 | 国税庁 法人番号 | 現行規約はPDL1.0。データ非同梱、読取専用カタログ、国税庁の許可項目だけを選択。画面/CSV/API/MCPに出典と加工表示を追加。 | 公式規約と実装を確認 | 他出典の業種/電話/HPを自動転記しない |
| 8 | 厚生労働省 職場情報総合サイト（しょくばらぼ） | コンテンツ利用ルール（更新 2025-02-27）は政府標準利用規約第2.0版に準拠し、CC BY 4.0 と互換。「ダウンロードして使用するコンテンツ等については商用利用も可能」で、条件は出典の記載、加工した場合はその旨の記載、国が作成したかのような公表の禁止。ただし「利用は、求職者等の就職活動及び企業等の採用活動に資する取組や研究その他正当な目的の範囲内」とあり、営業一般の利用が含まれるかの読みは確定していない。サイト利用規約（更新 2026-03-04）に、営業目的や一括取得を禁じる条項は見当たらない。 | 使わない方針（2026-10-23 保守者が決定） | このソフトはこの出典の取込器もデータも提供しない。読みを確定させる作業もしない |
| 9 | 合成データ | 法人番号は、国税庁「法人番号の構成」で未使用とされる帯（基礎番号の先頭が6・8・9）から作る。将来の非割当は保証されないため、保存先の種別の記録（要件8.3）でも取り違えを防ぐ。社名は乱数による架空名で、実在の社名と偶然一致しうる。 | 確認済み | — |
| 10 | 依存部品 | Cargo.lockで固定。新しい認証/hash依存も同じ許可ライセンス方針で検査する。 | 公開前の再検査対象 | licenses/bans/sources、オンラインadvisories、REUSEの実行結果をタスクへ記録 |
| 11 | 公開の範囲 | 2026-10-26に保守者が残機能の実装とソース一般公開を指示。全履歴/追跡ファイルの秘密検査、LICENSE/REUSE、README/SECURITYを公開用に更新。 | ソース公開を許可済み | 最終検証SHAをPRへ記録し、公開後に匿名到達とCIを確認。外部SaaS配備は別工程 |
| 12 | 公開デモ（匿名の訪問者・共有の合成データ） | 運営者が自分のドメインで Cloudflare Workers/Containers に配備する使い捨てのデモ。npm の依存はすべて開発・試験用で配布物に入るものはなく、Rust の依存は増えていない。Cloudflare の契約（許容される利用・開発者向けの追加条項）と GitHub Actions の規約に抵触する機能は見当たらない。外部送信規律は、CSP が外部送信を許さず Cookie がセッション用だけで、運営者自身のサービスの提供にあたるため、調べた範囲では義務は生じない見込み（全員がデータを共有する形が「媒介」に当たるかは判断が分かれうる）。個人情報は「入力しない」と表示し、運営者は保存・参照・解析せず、初期化で消す設計。投稿内容の責任は、小規模な個人運営に届出や窓口の義務はないが、連絡先の表示と停止・初期化の手順を用意。 | 調べた範囲では止める問題なし（専門家の確認は未実施） | 公開する配備では連絡先（`MEIBO_DEMO_CONTACT`）を必須にする。公開ドメインのゾーンで、外部へ送る計測スクリプトを差し込む機能を使わない。通報を受けたら停止か再起動で初期化する（`deploy/demo/cloudflare/OPERATIONS.md`）。確かめていないことは下の一覧 |

検証の生ログを保存する `.agent-harness/` はローカル限定で、リポジトリに含めません。共有する結果は仕様のタスク、測定文書、CIに残します。

### 公開デモ（12）で確かめていないこと
- 画面（`demo.css`・`login.css`）の見た目が、他社の画面に似ていないかの目視。
- npm の各 `package.json` の個別の照合（ロックファイルの値だけを見た）。Debian イメージ内の個々のパッケージのライセンス。
- Cloudflare の DPA の本文と、許容される利用のリンク先の詳細。公開ドメインのゾーン設定（Web Analytics・Zaraz など）で外部スクリプトが差し込まれないかの実機での確認。
- 総務省の外部送信規律の FAQ・ガイドラインの本文、情報流通プラットフォーム対処法の条文と省令の原文。個人情報保護法と電気通信事業法の該当性についての専門家の確認。
- 障害者差別解消法など、アクセシビリティに関する法令の適用。

## 一次資料（確認日 2026-10-22。ただし7は2026-10-21）
- 国税庁 法人番号公表サイト 利用規約: https://www.houjin-bangou.nta.go.jp/riyokiyaku/
- 公共データ利用規約（第1.0版）: https://www.digital.go.jp/resources/open_data/public_data_license_v1.0
- 国税庁「法人番号の構成」: https://www.houjin-bangou.nta.go.jp/documents/houjinbangounokousei.pdf
- 職場情報総合サイト コンテンツ利用ルール: https://shokuba.mhlw.go.jp/080/020/20180302211639.html
- 職場情報総合サイト 利用規約: https://shokuba.mhlw.go.jp/080/010/20180302211203.html

## 一次資料（公開デモ・確認日 2026-11-24）
- Cloudflare Self-Serve Subscription Agreement: https://www.cloudflare.com/terms/
- Cloudflare Service-Specific Terms（Developer Platform）: https://www.cloudflare.com/service-specific-terms-developer-platform/
- Cloudflare Containers の外への通信: https://developers.cloudflare.com/containers/platform-details/outbound-traffic/
- GitHub Terms for Additional Products and Features（Actions）: https://docs.github.com/en/site-policy/github-terms/github-terms-for-additional-products-and-features
- 総務省 外部送信規律: https://www.soumu.go.jp/main_sosiki/joho_tsusin/d_syohi/gaibusoushin_kiritsu.html
- 総務省 情報流通プラットフォーム対処法の資料: https://www.soumu.go.jp/main_content/000996607.pdf
- 個人情報保護委員会 ガイドライン（通則編）: https://www.ppc.go.jp/personalinfo/legal/guidelines_tsusoku/
- 個人情報保護委員会 Q&A 7-53（クラウド）: https://www.ppc.go.jp/all_faq_index/faq1-q7-53/
