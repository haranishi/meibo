"use strict";

(() => {
  const nativeFetch = window.fetch.bind(window);
  const legacyKeys = ["meibo-recent-companies", "meibo-list-views-v1"];
  let scope = null, leaving = false, suspendedTitle = null, ready = false, validation = 0;

  function cleanStorage(allHosted = false) {
    for (const name of ["localStorage", "sessionStorage"]) {
      try {
        const storage = window[name];
        const keys = [];
        for (let i = 0; i < storage.length; i++) keys.push(storage.key(i));
        keys.filter((key) => legacyKeys.includes(key) || (allHosted && key?.startsWith("meibo-hosted:"))).forEach((key) => storage.removeItem(key));
      } catch (_) {}
    }
  }
  function signalChange() {
    try { localStorage.setItem("meibo-auth-change", String(Date.now())); } catch (_) {}
  }
  function hideBusiness() {
    if (suspendedTitle === null) suspendedTitle = document.title;
    document.title = "meibo";
    document.documentElement.setAttribute("data-meibo-auth-pending", "");
    document.body.hidden = true;
    document.body.inert = true;
    document.querySelectorAll("main").forEach((main) => { main.hidden = true; main.setAttribute("data-meibo-gated", ""); main.inert = true; });
  }
  function showBusiness() {
    if (leaving) return;
    document.documentElement.removeAttribute("data-meibo-auth-pending");
    document.body.hidden = false;
    document.body.inert = false;
    document.querySelectorAll("main[data-meibo-gated]").forEach((main) => { main.hidden = false; main.inert = false; main.removeAttribute("data-meibo-gated"); });
    if (suspendedTitle !== null && document.title === "meibo") document.title = suspendedTitle;
    suspendedTitle = null;
  }
  function leave() {
    if (leaving) return;
    leaving = true; hideBusiness(); cleanStorage(true);
    document.querySelectorAll("input, textarea").forEach((input) => { input.value = ""; });
    window.location.replace("/login");
  }
  cleanStorage();

  const form = document.getElementById("login-form");
  if (form) {
    const access = form.hasAttribute("data-cloudflare-access");
    let saving = false;
    form.addEventListener("submit", async (event) => {
      event.preventDefault(); if (saving || !form.reportValidity()) return;
      saving = true;
      const fields = document.getElementById("login-fields"), error = document.getElementById("login-error");
      const control = document.getElementById("login-submit"), password = document.getElementById("login-password");
      const idleText = access ? "Cloudflareでログイン" : "ログイン";
      error.hidden = true; fields.disabled = true; control.textContent = "ログインしています…";
      try {
        const body = { tenant: document.getElementById("login-tenant").value.trim() };
        if (!access) { body.username = document.getElementById("login-username").value.trim(); body.password = password.value; }
        const response = await nativeFetch("/auth/login", { method: "POST", cache: "no-store", credentials: "same-origin",
          headers: { "Content-Type": "application/json", "X-Meibo-Local": "1", "X-Requested-With": "XMLHttpRequest" },
          body: JSON.stringify(body) });
        if (access && response.status === 401) { window.location.replace("/login"); return; }
        const data = await response.json();
        if (!response.ok) throw new Error(typeof data.error === "string" ? data.error : "ログインできませんでした。");
        if (password) password.value = "";
        cleanStorage(true); signalChange(); window.location.replace("/");
      } catch (failure) {
        if (password) password.value = "";
        error.textContent = failure.message || "接続できませんでした。もう一度お試しください。"; error.hidden = false;
      } finally {
        saving = false; fields.disabled = false; control.textContent = idleText;
        if (!error.hidden) (password || document.getElementById("login-tenant")).focus();
      }
    });
    return;
  }

  hideBusiness();
  async function session() {
    const response = await nativeFetch("/auth/session", { cache: "no-store", credentials: "same-origin", headers: { "X-Requested-With": "XMLHttpRequest" } });
    if (!response.ok) throw new Error("authentication required");
    const identity = await response.json();
    if (!/^[a-f0-9]{64}$/.test(identity.storage_scope) || typeof identity.tenant !== "string" || !["viewer", "editor", "admin"].includes(identity.role)) throw new Error("invalid context");
    return identity;
  }
  async function verifyContext() {
    if (leaving || !scope) return false;
    try {
      if ((await session()).storage_scope !== scope) { leave(); return false; }
      return true;
    } catch (_) { leave(); return false; }
  }
  async function revalidate() {
    const current = ++validation;
    hideBusiness();
    if (!ready || leaving) return;
    if (await verifyContext() && current === validation && !document.hidden) showBusiness();
  }
  window.addEventListener("pagehide", () => { validation++; hideBusiness(); });
  window.addEventListener("pageshow", async (event) => {
    if (!event.persisted) return;
    await revalidate();
  });
  window.addEventListener("focus", revalidate);
  document.addEventListener("visibilitychange", () => { if (document.hidden) { validation++; hideBusiness(); } else revalidate(); });
  window.addEventListener("storage", (event) => { if (event.key === "meibo-auth-change" || event.key === null) revalidate(); });

  session().then(async (identity) => {
    scope = identity.storage_scope;
    window.MeiboContext = Object.freeze({ hosted: true, role: identity.role, tenant: identity.tenant,
      storageKey: (base) => `meibo-hosted:${scope}:${base}` });
    window.fetch = async (input, options = {}) => {
      const url = new URL(typeof input === "string" ? input : input.url, window.location.href);
      const business = url.origin === location.origin && (url.pathname.startsWith("/api/") || ["/auth/logout", "/auth/audit"].includes(url.pathname));
      const headers = new Headers(options.headers || (input instanceof Request ? input.headers : undefined));
      if (business) { headers.set("X-Meibo-Context", scope); headers.set("X-Requested-With", "XMLHttpRequest"); }
      const response = await nativeFetch(input, { ...options, headers });
      if (business && response.status === 401) leave();
      if (business && response.status === 409) {
        try { if ((await response.clone().json()).code === "context_changed") leave(); } catch (_) {}
      }
      return response;
    };
    for (const inertScript of document.querySelectorAll("script[data-meibo-src]")) {
      const source = inertScript.dataset.meiboSrc;
      if (!source?.startsWith("/") || source.startsWith("//")) throw new Error("invalid script");
      await new Promise((resolve, reject) => {
        const script = document.createElement("script"); script.src = source;
        script.onload = resolve; script.onerror = reject; document.body.append(script);
      });
    }
    window.MeiboAppStart?.();
    const target = document.querySelector(".global-utilities");
    if (target) {
      const box = document.createElement("div"); box.className = "hosted-session";
      const label = document.createElement("span");
      label.textContent = `${identity.tenant} · ${{ viewer: "閲覧", editor: "編集", admin: "管理者" }[identity.role]}`;
      label.title = `${identity.username} · ${identity.tenant}`;
      const logout = document.createElement("button"); logout.type = "button"; logout.textContent = "ログアウト";
      logout.addEventListener("click", async () => {
        logout.disabled = true;
        try {
          const response = await window.fetch("/auth/logout", { method: "POST", headers: { "Content-Type": "application/json", "X-Meibo-Local": "1" }, body: "{}" });
          if (response.ok) { signalChange(); leave(); }
          else { logout.disabled = false; logout.textContent = "もう一度ログアウト"; }
        } catch (_) { logout.disabled = false; logout.textContent = "もう一度ログアウト"; }
      });
      box.append(label, logout); target.replaceChildren(box);
    }
    ready = true;
    await revalidate();
  }).catch(leave);
})();
