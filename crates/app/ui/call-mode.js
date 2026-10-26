"use strict";
(() => {
  let context, session = null, moving = false;
  const make = (tag, text, className) => { const n = document.createElement(tag); if (text) n.textContent = text; if (className) n.className = className; return n; };
  function control(text, action, primary = false) {
    const n = make("button", text, `button ${primary ? "button-primary" : "button-secondary"}`);
    n.type = "button"; n.disabled = moving || context.busy(); n.addEventListener("click", action); return n;
  }
  function render() {
    document.querySelector(".call-mode-banner")?.remove();
    if (!session) return;
    const box = make("section", "", "call-mode-banner"); box.setAttribute("aria-label", "架電モード");
    const info = make("div"), title = make("strong", `架電モード · ${session.position + 1}件目`);
    const current = context.company();
    const status = make("p", moving ? "次の取引先を読み込んでいます…" : session.error || `${current?.phone ? "電話番号を確認して記録してください。" : "電話番号は未登録です。"} Ctrl / ⌘ + Enter で保存して次へ。`);
    status.setAttribute("role", "status"); info.append(title, status);
    const actions = make("div", "", "call-mode-actions");
    if (session.error) actions.append(control("次の取引先を再読み込み", () => showCurrent()));
    else actions.append(control("保存して次へ", () => document.getElementById("activity-form")?.requestSubmit(), true));
    actions.append(control("記録せずスキップ", skip), control("架電モードを終了", () => stop(true)));
    box.append(info, actions); document.getElementById("detail-content").prepend(box);
  }
  function allowMove() {
    if (moving || context.busy()) { context.notice("処理の完了を待ってください。"); return false; }
    return !context.unsaved() || window.confirm("未保存の入力があります。記録せず移動しますか？");
  }
  function stop(close = false) {
    if (!session) return true;
    if (!allowMove()) return false;
    session = null; document.querySelector(".call-mode-banner")?.remove();
    if (close) context.close(); return true;
  }
  async function nextPage() {
    const current = session;
    while (current && session === current && current.next) {
      const params = new URLSearchParams(current.params); params.set("after", current.next);
      let data;
      try { data = await (await context.request(`/api/companies?${params}`)).json(); }
      catch (error) {
        if (error.status !== 400 || current.restarted) throw error;
        current.restarted = true;
        data = await (await context.request(`/api/companies?${current.params}`)).json();
      }
      current.next = data.next;
      const rows = data.rows.filter(r => r.status !== 5 && !current.seen.has(r.id));
      for (const row of rows) current.seen.add(row.id);
      current.ids.push(...rows.map(r => r.id));
      if (rows.length) return;
    }
  }
  async function showCurrent() {
    const current = session; if (!current || moving) return;
    moving = true; current.error = null; render();
    try {
      while (session === current) {
        if (current.position >= current.ids.length) await nextPage();
        if (current.position >= current.ids.length) {
          session = null; moving = false;
          context.notice("対象の記録が完了しました。取引先一覧へ戻ります。"); context.close(); return;
        }
        await context.open(current.ids[current.position]);
        const company = context.company();
        if (!company || company.id !== current.ids[current.position]) throw new Error("詳細を取得できませんでした。");
        if (company.status === 5) { current.position++; continue; }
        break;
      }
    } catch (error) {
      if (session === current) current.error = `記録済みの内容は保持しています。${error.message}`;
    } finally { moving = false; render(); if (session && !session.error) document.getElementById("activity-note")?.focus(); }
  }
  async function start() {
    if (context.busy()) return;
    if (session) { await showCurrent(); return; }
    const rows = context.rows().filter(r => r.status !== 5);
    if (!rows.length) { context.notice("対象の取引先がありません。検索条件を変更してください。"); return; }
    session = {ids: rows.map(r => r.id), seen: new Set(rows.map(r => r.id)), next: context.next(), params: context.params(), position: 0, restarted: false, error: null};
    await showCurrent();
  }
  async function skip() { if (!session || !allowMove()) return; session.position++; await showCurrent(); }
  async function saved(id) {
    if (!session || session.ids[session.position] !== id) return;
    session.position++; await showCurrent();
  }
  document.addEventListener("keydown", event => {
    if (session && !moving && !context.busy() && (event.ctrlKey || event.metaKey) && event.key === "Enter") {
      if (document.querySelector("dialog[open]")) return;
      event.preventDefault(); document.getElementById("activity-form")?.requestSubmit();
    }
  });
  window.MeiboCallMode = {configure: value => { context = value; }, start, render, saved, stop, active: () => Boolean(session)};
})();
