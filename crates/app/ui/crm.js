"use strict";
(() => {
  const stages = ["見込み", "評価", "提案", "交渉", "受注", "失注"];
  const labels = { "crm-tasks": "ToDo", "crm-events": "予定", "crm-deals": "商談" };
  const crm = { name: "crm-tasks", items: [], next: null, cursors: [null], page: 0, version: 0, company: null, view: matchMedia("(max-width:700px)").matches ? "list" : "board", completion: "false", saving: false };
  const box = el("section", "crm-workspace workspace-card"); box.id = "crm-workspace"; box.hidden = true;
  document.querySelector(".workspace-footer").before(box);
  for (const [name, label] of Object.entries(labels)) {
    const control = button(label, "", () => navigate(name)); control.dataset.workspace = name;
    document.querySelector(".workspace-navigation").insertBefore(control, $("record-tab"));
    const menuControl = button(label, "", () => navigate(name)); menuControl.dataset.workspace = name; $("app-menu").append(menuControl);
  }
  const dialog = el("dialog", "company-dialog crm-dialog"); dialog.id = "crm-dialog"; document.body.append(dialog);
  dialog.addEventListener("cancel", (event) => { if (crm.saving) event.preventDefault(); });
  const isDeal = () => crm.name === "crm-deals";
  const kind = () => crm.name === "crm-events" ? "event" : "task";
  const endpoint = () => isDeal() ? "/api/opportunities" : "/api/work-items";
  const money = (v) => new Intl.NumberFormat("ja-JP", { style: "currency", currency: "JPY", maximumFractionDigits: 0 }).format(v);
  const post = async (url, body) => (await request(url, { method: "POST", headers: { "Content-Type": "application/json", "X-Meibo-Local": "1" }, body: JSON.stringify(body) })).json();
  function reset() { crm.cursors = [null]; crm.page = 0; crm.next = null; }
  function open(name, company = null) { if (busy() || crm.saving) return; crm.name = name; crm.company = company; reset(); showWorkspace(name); }
  function onWorkspace(name) { box.hidden = !labels[name]; if (!labels[name]) ++crm.version; else { crm.name = name; load(); } if (name === "home") loadHomeCrm(); }
  let homeGeneration = 0;
  const homeCrm = el("section", "workspace-card crm-home-card"); document.querySelector(".home-grid").append(homeCrm);
  async function loadHomeCrm() {
    const generation = ++homeGeneration; homeCrm.replaceChildren(el("h2", "card-heading", "ToDo・予定と商談"));
    try {
      const [work, deals] = await Promise.all([request("/api/work-items?completed=false&limit=5").then(r => r.json()), request("/api/opportunities?limit=1").then(r => r.json())]);
      if (generation !== homeGeneration) return;
      homeCrm.append(el("p", "crm-home-counts", `未完了ToDo・予定 ${work.total} 件 / 商談 ${deals.total} 件`));
      work.items.forEach(item => { const row = el("div", "home-list-item"); row.append(button(item.subject, "home-item-name", () => open(item.kind === "task" ? "crm-tasks" : "crm-events", {id:item.company_id,name:item.company_name})), el("span", "home-item-meta", formatDate(item.due_at ?? item.start_at))); homeCrm.append(row); });
      if (!work.items.length) homeCrm.append(el("p", "empty-message", "未完了のToDo・予定はありません。"));
      homeCrm.append(button("商談を開く", "button button-secondary crm-home-link", () => open("crm-deals")));
    } catch (e) { if (generation === homeGeneration) homeCrm.append(el("p", "field-error", e.message), button("再試行", "text-button", loadHomeCrm)); }
  }
  function heading() {
    box.replaceChildren();
    const header = el("header", "object-header"), text = el("div");
    text.append(el("p", "eyebrow", "営業ワークスペース"), el("h1", "", labels[crm.name]));
    if (crm.company) text.append(el("p", "subtle", companyName(crm.company)));
    const actions = el("div", "object-actions"); actions.append(button("新規" + labels[crm.name], "button button-primary", () => editor()));
    if (crm.company) actions.append(button("すべての取引先", "button button-secondary", () => open(crm.name)));
    if (isDeal()) actions.append(button(crm.view === "board" ? "一覧で表示" : "ボードで表示", "button button-secondary", () => { crm.view = crm.view === "board" ? "list" : "board"; render(); }));
    if (!isDeal() && kind() === "task") {
      const filter = el("select"); filter.setAttribute("aria-label", "ToDoの完了状態");
      for (const [v, t] of [["false", "未完了"], ["true", "完了済み"], ["", "すべて"]]) { const op = el("option", "", t); op.value = v; filter.append(op); }
      filter.value = crm.completion; filter.addEventListener("change", () => { crm.completion = filter.value; reset(); load(); }); actions.append(filter);
    }
    header.append(text, actions); box.append(header);
  }
  async function load() {
    const version = ++crm.version; heading(); box.append(el("p", "empty-message", "読み込んでいます…"));
    const params = new URLSearchParams({ limit: "50" });
    if (!isDeal()) { params.set("kind", kind()); if (kind() === "task" && crm.completion) params.set("completed", crm.completion); }
    if (crm.company) params.set("company_id", crm.company.id);
    if (crm.cursors[crm.page]) params.set("after", crm.cursors[crm.page]);
    try {
      const data = await (await request(`${endpoint()}?${params}`)).json(); if (version !== crm.version) return;
      crm.items = data.items; crm.next = data.next; crm.total = data.total; render();
    } catch (error) { if (version !== crm.version) return; heading(); box.append(el("p", "inline-error", error.message), button("再試行", "button button-secondary", load)); }
  }
  function render() {
    heading();
    const stats = el("div", "crm-summary"); stats.append(el("strong", "", `${numberFormat.format(crm.total)} 件`));
    if (isDeal()) {
      const sum = crm.items.filter((v) => v.stage !== 5).reduce((a, b) => a + BigInt(b.amount_jpy), 0n);
      stats.append(el("span", "", `表示中の合計（失注を除く） ${money(sum)}`));
    } else stats.append(el("span", "", kind() === "event" ? "開始日時の早い順" : "期限の早い順 · 次回対応とは別に管理"));
    box.append(stats);
    if (!crm.items.length) box.append(el("p", "crm-empty", `該当する${labels[crm.name]}はありません。「新規${labels[crm.name]}」から追加できます。`));
    else if (isDeal() && crm.view === "board") renderBoard(); else renderTable();
    const footer = el("footer", "pagination"); footer.append(el("p", "", `${crm.page + 1} ページ · ${crm.items.length} 件を表示`));
    const controls = el("div"), previous = button("前へ", "page-button", () => { --crm.page; load(); }), next = button("次へ", "page-button", () => { crm.cursors[++crm.page] = crm.next; load(); });
    previous.disabled = crm.page === 0; next.disabled = !crm.next; controls.append(previous, next); footer.append(controls); box.append(footer);
  }
  function account(item) { return button(item.company_name || "名称は非表示", "text-button", () => openCompany(item.company_id)); }
  function renderBoard() {
    box.append(el("p", "crm-board-help", "左右にスクロールして段階を表示できます。段階はカードの選択欄から変更します。"));
    const board = el("div", "crm-board"); board.setAttribute("aria-label", "商談を段階別に表示"); board.tabIndex = 0;
    stages.forEach((label, stage) => {
      const lane = el("section", "crm-lane"), items = crm.items.filter((v) => v.stage === stage);
      lane.append(el("h2", "", `${label} · ${items.length}`), el("p", "subtle", money(items.reduce((sum, v) => sum + BigInt(v.amount_jpy), 0n))));
      for (const item of items) {
        const card = el("article", "crm-deal-card"); card.append(button(item.name, "crm-record-name", () => editor(item)), account(item), el("strong", "crm-money", money(item.amount_jpy)), el("p", "subtle", `完了予定 ${item.close_date}`), stageControl(item)); lane.append(card);
      }
      if (!items.length) lane.append(el("p", "empty-message", "商談なし")); board.append(lane);
    }); box.append(board);
  }
  function stageControl(item) {
    const select = el("select", "crm-stage-select"); select.setAttribute("aria-label", `${item.name}の商談段階`);
    stages.forEach((label, index) => { const option = el("option", "", label); option.value = index; select.append(option); }); select.value = item.stage;
    select.addEventListener("change", async () => {
      const value = Number(select.value); select.disabled = true;
      try { await post(`/api/opportunities/${item.id}/stage`, { stage: value }); if (labels[state.workspace]) await load(); notify("商談段階を更新しました。"); }
      catch (error) { select.value = item.stage; notify(error.message, true); } finally { select.disabled = false; }
    }); return select;
  }
  function renderTable() {
    box.append(el("p", "crm-table-help", "表を左右にスクロールすると、期限や操作欄を確認できます。"));
    const container = el("div", "table-container"), table = el("table", "company-table crm-table"), head = el("thead"), tr = el("tr");
    const columns = isDeal() ? ["商談名", "取引先", "金額", "完了予定", "段階"] : ["件名", "取引先", kind() === "event" ? "開始〜終了" : "期限", "操作"];
    columns.forEach((c) => { const th = el("th", "", c); th.scope = "col"; tr.append(th); }); head.append(tr); table.append(head);
    const body = el("tbody"); crm.items.forEach((item) => {
      const row = el("tr"), title = el("td"), company = el("td"); title.append(button(item.name || item.subject, "company-name", () => editor(item))); company.append(account(item)); row.append(title, company);
      if (isDeal()) { row.append(el("td", "", money(item.amount_jpy)), el("td", "", item.close_date)); const stage = el("td"); stage.append(stageControl(item)); row.append(stage); }
      else {
        const date = el("td", kind() === "task" && !item.completed && item.due_at !== null && item.due_at < Date.now() / 1000 ? "crm-overdue" : "", kind() === "event" ? `${formatDate(item.start_at)} 〜 ${formatDate(item.end_at)}` : formatDate(item.due_at));
        row.append(date); const action = el("td");
        if (kind() === "task") action.append(button(item.completed ? "再開" : "完了にする", "button button-secondary", async (event) => {
          const control = event.currentTarget; control.disabled = true;
          try { await post(`/api/work-items/${item.id}/completion`, { completed: !item.completed }); if (labels[state.workspace]) await load(); notify(item.completed ? "ToDoを再開しました。" : "ToDoを完了しました。"); }
          catch (error) { notify(error.message, true); control.disabled = false; }
        })); else action.append(button("編集", "text-button", () => editor(item))); row.append(action);
      } body.append(row);
    }); table.append(body); container.append(table); box.append(container);
  }
  function inputField(form, label, name, type, value, required = true) {
    const field = el("div", "form-field"), id = `crm-${name}`, labelEl = el("label", "", label), input = el("input"); labelEl.htmlFor = id; input.id = id; input.name = name; input.type = type; input.required = required; input.value = value ?? "";
    field.append(labelEl, input); form.append(field); return input;
  }
  const localTime = (seconds) => { if (seconds === null || seconds === undefined) return ""; const date = new Date(seconds * 1000); return new Date(date - date.getTimezoneOffset() * 60000).toISOString().slice(0, 16); };
  function editor(item = null, context = null) {
    if (crm.saving || busy()) return;
    const mode = context?.name ?? crm.name, deal = mode === "crm-deals", itemKind = mode === "crm-events" ? "event" : "task", endpointAtOpen = deal ? "/api/opportunities" : "/api/work-items"; dialog.replaceChildren();
    const form = el("form"), header = el("header", "dialog-header"), title = el("h2", "", `${item ? "編集" : "新規"}${labels[mode]}`); title.id = "crm-dialog-title"; dialog.setAttribute("aria-labelledby", title.id);
    header.append(title, button("×", "icon-button", () => { if (!crm.saving) dialog.close(); })); form.append(header);
    const fields = el("fieldset", "dialog-fields"), error = el("p", "field-error dialog-error"); error.id = "crm-form-error"; error.setAttribute("role", "alert"); error.hidden = true;
    let chosen = item ? { id: item.company_id, name: item.company_name } : context?.company ?? crm.company;
    const chooser = el("div", "form-field full-width"), selected = el("p", "", chosen ? companyName(chosen) : "取引先を検索して選択してください");
    const searchLabel = el("label", "", "取引先"), search = el("input"); search.id = "crm-company-search"; search.type = "search"; search.placeholder = "取引先名を検索"; search.maxLength = 200; searchLabel.htmlFor = search.id;
    const results = el("div", "crm-company-results"); let searchVersion = 0;
    const find = async () => {
      const version = ++searchVersion;
      try { const data = await (await request(`/api/companies?limit=20&q=${encodeURIComponent(search.value.trim())}`)).json(); if (version !== searchVersion || !dialog.open) return; results.replaceChildren(); data.rows.forEach((c) => results.append(button(companyName(c), "crm-company-option", () => { chosen = c; selected.textContent = companyName(c); results.replaceChildren(); }))); if (!data.rows.length) results.append(el("p", "", "一致する取引先がありません。")); }
      catch (e) { if (version === searchVersion) results.textContent = e.message; }
    };
    chooser.append(searchLabel, selected);
    if (!item) { chooser.append(search, button("取引先を検索", "button button-secondary", find), results); }
    fields.append(chooser);
    const subject = inputField(fields, deal ? "商談名" : "件名", "subject", "text", item?.name || item?.subject); subject.maxLength = 200;
    let amount, close, stage, due, start, end;
    if (deal) {
      amount = inputField(fields, "金額（円・JPY）", "amount", "number", item?.amount_jpy ?? 0); amount.min = "0"; amount.max = "9007199254740991"; amount.step = "1";
      close = inputField(fields, "完了予定日", "close-date", "date", item?.close_date);
      const field = el("div", "form-field"), label = el("label", "", "商談段階"); stage = el("select"); stage.id = "crm-stage"; label.htmlFor = stage.id; stages.forEach((text, i) => { const op = el("option", "", text); op.value = i; stage.append(op); }); stage.value = item?.stage ?? 0; field.append(label, stage); fields.append(field);
    } else if (itemKind === "task") due = inputField(fields, "期限", "due", "datetime-local", localTime(item?.due_at));
    else { start = inputField(fields, "開始", "start", "datetime-local", localTime(item?.start_at)); end = inputField(fields, "終了", "end", "datetime-local", localTime(item?.end_at)); }
    const footer = el("footer", "dialog-footer"), cancel = button("キャンセル", "button button-secondary", () => dialog.close()), save = el("button", "button button-primary", "保存"); save.type = "submit"; footer.append(cancel, save); form.append(fields, error, footer); dialog.append(form);
    form.addEventListener("submit", async (event) => {
      event.preventDefault(); if (crm.saving || !form.reportValidity()) return;
      if (!chosen) { error.textContent = "取引先を検索して選択してください。"; error.hidden = false; return; }
      const body = { company_id: chosen.id, contact_id: item?.contact_id ?? null };
      try {
      if (deal) Object.assign(body, { name: subject.value.trim(), amount_jpy: Number(amount.value), stage: Number(stage.value), close_date: close.value });
      else Object.assign(body, { kind: itemKind, subject: subject.value.trim(), due_at: due ? parseLocalDate(due.value) : null, start_at: start ? parseLocalDate(start.value) : null, end_at: end ? parseLocalDate(end.value) : null, completed: item?.completed ?? false });
      if (!deal && itemKind === "event" && body.end_at <= body.start_at) throw new Error("終了日時は開始日時より後にしてください。");
      } catch (e) { error.textContent = e.message; error.hidden = false; return; }
      crm.saving = true; fields.disabled = true; save.disabled = true; cancel.disabled = true; error.hidden = true;
      try { await post(endpointAtOpen + (item ? `/${item.id}` : ""), body); dialog.close(); notify("保存しました。"); crm.saving = false; if (labels[state.workspace]) await load(); else if (state.workspace === "record" && state.selected === chosen.id) openCompany(chosen.id, false); }
      catch (e) { error.textContent = e.message; error.hidden = false; }
      finally { crm.saving = false; fields.disabled = false; save.disabled = false; cancel.disabled = false; }
    }); dialog.showModal(); if (!chosen) search.focus(); else subject.focus();
  }
  async function related(company, target) {
    const section = el("section", "related-card crm-related"); section.append(el("h2", "card-heading", "ToDo・予定・商談")); target.append(section);
    for (const [name, label] of Object.entries(labels)) {
      const row = el("div", "crm-related-row"); row.append(button(label + "を開く", "text-button", () => open(name, company)), button("新規" + label, "button button-secondary", () => editor(null, {name, company}))); section.append(row);
    }
    try {
      const [work, deals] = await Promise.all([request(`/api/work-items?company_id=${company.id}&limit=5`).then((r) => r.json()), request(`/api/opportunities?company_id=${company.id}&limit=5`).then((r) => r.json())]);
      if (!section.isConnected) return;
      section.append(el("p", "subtle", `ToDo・予定 ${work.total} 件 / 商談 ${deals.total} 件`));
      [...work.items, ...deals.items].forEach((item) => section.append(el("p", "related-record", item.name || item.subject)));
    } catch (error) { if (section.isConnected) section.append(el("p", "field-error", error.message)); }
  }
  window.crmWorkspace = { open, onWorkspace, related, recognizes: (name) => Boolean(labels[name]), busy: () => crm.saving };
})();
