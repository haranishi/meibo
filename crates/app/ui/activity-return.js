"use strict";
(() => {
  const query=new URLSearchParams(location.search);
  const state=query.get("state"), code=query.get("code"), denied=query.has("error");
  history.replaceState(null,"",location.pathname);
  const result=document.getElementById("result");
  if (!/^[0-9a-f]{64}$/.test(state || "") || (!denied && !code)) {
    result.textContent="営業画面で接続をやり直してください。"; return;
  }
  const message={type:"meibo-activity-return",state,code:denied?null:code};
  if ("BroadcastChannel" in window) {
    const channel=new BroadcastChannel("meibo-activity-"+state); channel.postMessage(message); channel.close();
  }
  window.opener?.postMessage(message,location.origin);
  result.textContent="営業画面に戻って接続状態を確認してください。";
  window.close();
})();
