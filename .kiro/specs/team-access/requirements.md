# Team access requirements

2026-10-26 本人指示: 未完了機能をそれぞれ実行し、公開も進める。
初回の既定は無料・招待制ベータ。料金・契約・送信を伴う機能は含めない。

## 1. 組織と利用者の管理
1.1 CLIは専用ROOTを初期化し、組織・利用者・所属・viewer/editor/adminを管理できる。
1.2 passwordは標準入力から受け取り、引数・ログ・応答へ返さない。Argon2idのsalt付きhashのみ保存する。
1.3 組織の識別子は限定したASCII形式とし、サーバーがROOT内の別DBパスへ対応させる。
1.4 利用停止・password変更・所属変更は既存sessionを失効させる。既存DBやROOTを上書きしない。

## 2. ログインとsession
2.1 username/password/組織が有効なときだけ新しいsessionを発行する。不正な組合せは同一の応答にする。
2.2 session IDはOS乱数32bytes以上とし、サーバーにはSHA-256 hashのみ保存する。
2.3 本番cookieはHttpOnly・Secure・SameSite=Strict・Path=/とし、Domainは付けない。
2.4 絶対8時間・無操作30分で失効し、ログアウト後の再使用を拒否する。期限はサーバーで検査する。
2.5 ログイン失敗に時間枠付きの回数制限と保存件数上限を設ける。body・passwordの長さも制限する。
2.6 HTTP試験モードは明示したloopbackに限定し、本番とは別名のcookieを使う。

## 3. データ分離と権限
3.1 tenantはsessionの所属から確定する。URL・query・header・bodyから任意DBを選べない。
3.2 viewerは参照のみ、editorはCRM更新も可能、adminは加えて組織の監査を参照できる。
3.3 利用者・所属の管理は保守者CLIに限定する。個人情報消去はadminに限定する。
3.4 未認証・他組織・失効・権限不足の要求は、CRMを開く前に拒否する。
3.5 既存の個人情報マスク・明示取得・除外・入力検証・件数上限を維持する。
3.6 Engineのcacheを小さくし、同時に保持する接続数を制限する。無制限の組織別cacheを作らない。

## 4. HTTPと画面
4.1 公開Hostを設定値と照合し、全書込みに同一Originと専用headerを要求する。重複headerを拒否する。
4.2 forwarded headerから認証・組織・信頼するOriginを決めない。未知のmethod・不正path・過大bodyを拒否する。
4.3 login画面は組織・利用者・passwordを明示し、失敗時もpasswordを保存しない。成功後にCRMへ進む。
4.4 CRMに現在の組織・権限とログアウトを表示する。認証情報はWeb Storageへ保存しない。
4.5 no-store・CSPなど既存のセキュリティheaderを維持し、HTTP health応答に業務内容を含めない。
4.6 hostedでは認証コンテキストの確定前に業務script・業務UIを初期化しない。ページ離脱/復帰の確認中はheader/nav/dialog/通知を含む業務画面全体と企業名のtitleも非表示/中立化する。利用者/組織別のWeb Storageを使い、旧固定キーとlogout/切替時の保存内容を破棄する。
4.7 各業務API要求を初期化時の利用者/組織コンテキストへ結び付ける。同一ブラウザの別tabでsessionが切り替わった場合、旧画面から新組織を参照/更新せず再認証へ戻す。

## 5. 監査と保存・復旧
5.1 管理変更・認証・権限拒否・CRM更新の監査は時刻/actor/組織/操作/結果を記録し、password/session/本文を含めない。
5.2 control DBとCRM DBは永続Volumeへ置き、hostedではFULL durabilityを使う。
5.3 backupは管理操作とWeb処理を共通の保守lockで整列し、オンライン複製と検証済みmanifestを作る。
5.4 restoreは未存在のROOTへ行い、path逸脱・symlink・hash不一致・壊れたDBを拒否する。復旧したsessionは失効させる。
5.5 合成の2組織でbackup→新ROOTへrestore→再ログイン→各組織のデータ照合を実証する。

## 6. 配備と検証
6.1 Docker構成はnon-rootアプリ・永続Volume・HTTPS前段を備え、アプリportをホストへ公開しない。
6.2 本番HTTPSには設定したdomain・DNS・到達可能な80/443・証明書用永続保存が必要であると手順に示す。
6.3 2組織分離、各role、CSRF/Host、失効/再使用/期限、容量制限と復旧を自動試験する。
6.4 実ブラウザでlogin→CRM→更新→logout→戻る/再要求と3画面幅を確認する。同一ブラウザのA組織→logout→B組織・利用者切替・旧tabを試す。復帰時の認証応答を遅延させ、header検索語/レコード名/title/開いたdialog/通知を含む旧情報が表示されず、最近の企業/保存ビュー/下書き/要求も混ざらないことを確認する。
6.5 未配備、未実行のTLS/Volume確認は未確認と記録し、公開済みとは報告しない。
