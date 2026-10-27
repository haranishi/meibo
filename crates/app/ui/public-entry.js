"use strict";

(() => {
  const section = document.getElementById("public-entry-share");
  if (!section) return;
  const link = document.querySelector('link[rel="canonical"]');
  if (!link) return;
  let canonical;
  try {
    const url = new URL(link.href);
    if (url.protocol !== "https:" || url.origin !== location.origin || url.pathname !== "/login" || url.search || url.hash || url.username || url.password) return;
    canonical = url.href;
  } catch (_) { return; }

  const copy = document.getElementById("public-share-copy");
  const native = document.getElementById("public-share-native");
  const status = document.getElementById("public-share-status");
  const fallback = document.getElementById("public-share-fallback");
  const field = document.getElementById("public-share-url");
  function selectLink() {
    field.value = canonical;
    fallback.hidden = false;
    field.focus();
    field.select();
  }
  copy.disabled = false;
  copy.addEventListener("click", async () => {
    copy.disabled = true;
    status.textContent = "";
    try {
      if (typeof navigator.clipboard?.writeText === "function") {
        await navigator.clipboard.writeText(canonical);
      } else {
        selectLink();
        if (!document.execCommand("copy")) throw new Error("manual copy required");
      }
      fallback.hidden = true;
      status.textContent = "ログインURLをコピーしました。";
    } catch (_) {
      selectLink();
      status.textContent = "コピーできませんでした。表示されたURLを選択してコピーしてください。";
    } finally {
      copy.disabled = false;
      if (fallback.hidden) copy.focus();
    }
  });

  const payload = { title: "meibo · 営業ワークスペース", text: "企業情報と営業活動を、チームのワークスペースで。", url: canonical };
  let supported = typeof navigator.share === "function";
  try { if (supported && typeof navigator.canShare === "function") supported = navigator.canShare(payload); }
  catch (_) { supported = false; }
  if (!supported) return;
  native.hidden = false;
  native.addEventListener("click", async () => {
    native.disabled = true;
    status.textContent = "";
    try {
      // Keep the browser call in the original click, before any asynchronous work.
      await navigator.share(payload);
      status.textContent = "共有操作を終了しました。";
    } catch (failure) {
      status.textContent = failure?.name === "AbortError"
        ? "共有をキャンセルしました。"
        : "共有できませんでした。リンクをコピーして共有してください。";
    } finally {
      native.disabled = false;
    }
  });
})();
