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
  const isDemo = Boolean(document.querySelector(".demo-banner"));
  function leave(reset = false) {
    if (leaving) return;
    leaving = true; hideBusiness(); cleanStorage(true);
    document.querySelectorAll("input, textarea").forEach((input) => { input.value = ""; });
    window.location.replace(reset === true ? "/login?reset=1" : "/login");
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

  const demoEnter = document.getElementById("demo-enter");
  if (demoEnter) {
    const notice = document.getElementById("demo-reset-notice");
    const error = document.getElementById("login-error");
    if (notice && new URLSearchParams(window.location.search).get("reset") === "1") {
      notice.textContent = "デモが初期化されたか、しばらく操作がなかったため、入り直しが必要です。もう一度『デモを開く』を押してください。";
      notice.hidden = false;
    }
    const busyMessage = "混み合っています。少し待ってからもう一度お試しください。";
    const genericMessage = "入室できませんでした。時間を置いてもう一度お試しください。";
    const maintenanceMessage = "メンテナンス中です。しばらくしてからお試しください。";
    const startingText = "起動しています。数秒かかることがあります…";
    const slowHintDelayMs = 1500;
    const defaultRetrySeconds = 5;
    // 起動待ちの案内は数秒のもの。長い Retry-After（メンテナンスなど）をそのまま待たない。
    const maxRetrySeconds = 10;
    const maxRetries = 4;
    const idleText = demoEnter.textContent || "デモを開く";
    const retryAfterMs = (response) => {
      const raw = response.headers.get("Retry-After");
      if (raw !== null) {
        const trimmed = raw.trim();
        if (/^\d+$/.test(trimmed)) {
          const seconds = Number(trimmed);
          if (Number.isSafeInteger(seconds) && seconds <= maxRetrySeconds) return seconds * 1000;
        }
      }
      return defaultRetrySeconds * 1000;
    };
    const wait = (ms) => new Promise((resolve) => window.setTimeout(resolve, ms));
    let entering = false;
    demoEnter.addEventListener("click", async () => {
      if (entering) return;
      entering = true;
      if (error) { error.hidden = true; error.textContent = ""; }
      demoEnter.disabled = true;
      demoEnter.textContent = "開いています…";
      let redirected = false;
      let slowTimer = window.setTimeout(() => {
        slowTimer = null;
        if (entering && !redirected) demoEnter.textContent = startingText;
      }, slowHintDelayMs);
      const clearSlowTimer = () => {
        if (slowTimer !== null) {
          window.clearTimeout(slowTimer);
          slowTimer = null;
        }
      };
      try {
        let response = null;
        for (let attempt = 0; attempt <= maxRetries; attempt += 1) {
          response = await nativeFetch("/demo/enter", {
            method: "POST",
            cache: "no-store",
            credentials: "same-origin",
            headers: {
              "Accept": "application/json",
              "Content-Type": "application/json",
              "X-Meibo-Local": "1",
              "X-Requested-With": "XMLHttpRequest",
            },
            body: "{}",
          });
          if (response.status === 503) {
            let code = "";
            try { code = String((await response.clone().json())?.code || ""); } catch (_) { code = ""; }
            // 停止の切替（メンテナンス）は、待っても直らないので、すぐ案内する。
            if (code === "maintenance") throw new Error(maintenanceMessage);
            if (attempt < maxRetries) {
              clearSlowTimer();
              demoEnter.textContent = startingText;
              await wait(retryAfterMs(response));
              continue;
            }
          }
          break;
        }
        clearSlowTimer();
        if (!response.ok) throw new Error(response.status === 429 ? busyMessage : genericMessage);
        await response.json();
        cleanStorage(true);
        signalChange();
        redirected = true;
        window.location.replace("/");
      } catch (failure) {
        clearSlowTimer();
        if (error) {
          const known = failure?.message === busyMessage || failure?.message === maintenanceMessage;
          error.textContent = known ? failure.message : genericMessage;
          error.hidden = false;
        }
      } finally {
        clearSlowTimer();
        if (!redirected) {
          entering = false;
          demoEnter.disabled = false;
          demoEnter.textContent = idleText;
          if (error && !error.hidden) demoEnter.focus();
        }
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
      if ((await session()).storage_scope !== scope) { leave(isDemo); return false; }
      return true;
    } catch (_) { leave(isDemo); return false; }
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
      if (business && response.status === 401) leave(isDemo);
      if (business && response.status === 409) {
        try { if ((await response.clone().json()).code === "context_changed") leave(isDemo); } catch (_) {}
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
      const badge = isDemo ? target.querySelector(".demo-badge") : null;
      const box = document.createElement("div"); box.className = "hosted-session";
      const label = document.createElement("span");
      label.textContent = `${identity.tenant} · ${{ viewer: "閲覧", editor: "編集", admin: "管理者" }[identity.role]}`;
      label.title = `${identity.username} · ${identity.tenant}`;
      const logout = document.createElement("button"); logout.type = "button"; logout.textContent = "ログアウト";
      logout.addEventListener("click", async () => {
        logout.disabled = true;
        try {
          const response = await window.fetch("/auth/logout", { method: "POST", headers: { "Content-Type": "application/json", "X-Meibo-Local": "1" }, body: "{}" });
          if (response.ok) { signalChange(); leave(false); }
          else { logout.disabled = false; logout.textContent = "もう一度ログアウト"; }
        } catch (_) { logout.disabled = false; logout.textContent = "もう一度ログアウト"; }
      });
      box.append(label, logout);
      // The server adds the usage entry for remote admins only.
      const usage = target.querySelector("a.usage-entry[href='/usage']");
      target.replaceChildren(...[badge, usage, box].filter(Boolean));
      const menuLink = document.querySelector("#app-menu > a.usage-menu-link");
      if (menuLink) menuLink.parentElement.append(menuLink);
    }
    ready = true;
    await revalidate();
  }).catch(() => leave(isDemo));
})();
