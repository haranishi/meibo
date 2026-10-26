# ローカル画面の通信境界

2026-10-23、本人がローカルで試すための起動を明示した。`web-ui` のapp層だけにloopback HTTP受信を認め、core/importには通信を入れない。従来の通信シンボル禁止テストはcore/import/xtaskを検査する。

ローカルserveは127.0.0.1固定。Hostはそのポートのlocalhost/127.0.0.1だけ、更新は同一Originと専用ヘッダーを必須にする。既定は合成DB、実DBは2026-10-26追加の明示--allow-realで起動する。外部CDN・フォント・分析送信は使わない。

同日の追加依頼で、team-accessは認証・組織分離・権限・HTTPS前段を備えるhosted入口を別に提供する。api-mcpはこのHTTP境界内の版付きAPIと明示DBを使うstdio入口を追加する。無認証のlocal serveを外部へ公開する許可とは解釈しない。

公開時の確認だけに頼らず、ローカルでHost/Origin/不正なbodyと個人情報の境界をHTTP回帰試験に通す。画面は実ブラウザの主要操作と390/768/1440pxで確認する。
