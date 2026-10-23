"use strict";

const $ = (id) => document.getElementById(id);
const statuses = ["未着手", "対応中", "商談", "受注", "見送り", "除外"];
const prefectures = ["", "北海道", "青森県", "岩手県", "宮城県", "秋田県", "山形県", "福島県", "茨城県", "栃木県", "群馬県", "埼玉県", "千葉県", "東京都", "神奈川県", "新潟県", "富山県", "石川県", "福井県", "山梨県", "長野県", "岐阜県", "静岡県", "愛知県", "三重県", "滋賀県", "京都府", "大阪府", "兵庫県", "奈良県", "和歌山県", "鳥取県", "島根県", "岡山県", "広島県", "山口県", "徳島県", "香川県", "愛媛県", "高知県", "福岡県", "佐賀県", "長崎県", "熊本県", "大分県", "宮崎県", "鹿児島県", "沖縄県"];
const activityKinds = { 1: "架電", 2: "訪問", 3: "メール", 4: "メモ", 5: "状態変更", 9: "取り消し" };
const numberFormat = new Intl.NumberFormat("ja-JP");
const dateFormat = new Intl.DateTimeFormat("ja-JP", { year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false });
const shortDateFormat = new Intl.DateTimeFormat("ja-JP", { month: "2-digit", day: "2-digit" });
const state = {
  workspace: "accounts", returnWorkspace: "accounts", listContext: "accounts",
  rows: [], next: null, count: null, query: "", view: "all", filters: {}, cursors: [null], page: 0,
  selected: null, detail: null, listVersion: 0, detailVersion: 0, summaryVersion: 0, homeVersion: 0,
  listController: null, detailController: null, loading: false, saving: false, creating: false,
  drafts: new Map(), noticeTimer: null, summary: null, recent: readRecent(), recordTab: "details"
};

function el(tag, className, value) {
  const element = document.createElement(tag);
  if (className) element.className = className;
  if (value !== undefined && value !== null) element.textContent = String(value);
  return element;
}
function icon(name) {
  const element = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  element.setAttribute("class", "icon"); element.setAttribute("aria-hidden", "true");
  const use = document.createElementNS("http://www.w3.org/2000/svg", "use");
  use.setAttribute("href", `#icon-${name}`); element.append(use); return element;
}
function button(label, className, action) {
  const result = el("button", className, label); result.type = "button";
  result.addEventListener("click", action); return result;
}
function companyName(company) { return company.name === null || company.name === undefined ? "個人情報は非表示" : company.name; }
function statusBadge(value) { return el("span", `status-badge status-${value}`, statuses[value] || "不明"); }
function formatDate(value, short = false) {
  if (value === null || value === undefined || value === 0) return "未設定";
  const date = new Date(value * 1000);
  return Number.isFinite(date.getTime()) ? (short ? shortDateFormat : dateFormat).format(date) : "日時不明";
}
function notify(message, isError = false) {
  clearTimeout(state.noticeTimer); $("notice").textContent = message;
  $("notice").classList.toggle("is-error", isError); $("notice").hidden = false;
  state.noticeTimer = setTimeout(() => { $("notice").hidden = true; }, isError ? 10000 : 6000);
}
async function request(url, options = {}) {
  let response;
  try { response = await fetch(url, { cache: "no-store", ...options }); }
  catch (error) {
    if (error.name === "AbortError") throw error;
    throw new Error("サーバーに接続できません。ローカルサーバーの起動を確認してください。");
  }
  if (!response.ok) {
    let message = "読み込みに失敗しました。もう一度お試しください。";
    try { const result = await response.json(); if (typeof result.error === "string") message = result.error; } catch (_) {}
    const error = new Error(message); error.status = response.status; throw error;
  }
  return response;
}
function busy() {
  if (state.saving || state.creating) { notify("保存しています。完了してから操作してください。"); return true; }
  return false;
}
function closeAppMenu() {
  $("app-menu").hidden = true; $("app-launcher").setAttribute("aria-expanded", "false");
}
function showWorkspace(name) {
  state.workspace = name;
  $("home-workspace").hidden = name !== "home";
  $("accounts-workspace").hidden = name !== "accounts" && name !== "tasks";
  $("record-workspace").hidden = name !== "record";
  const active = name === "record" ? state.returnWorkspace : name;
  document.querySelectorAll("[data-workspace]").forEach((item) => {
    const selected = item.dataset.workspace === active;
    item.classList.toggle("is-active", selected);
    if (selected) item.setAttribute("aria-current", "page"); else item.removeAttribute("aria-current");
  });
  const recordName = state.detail?.company?.id === state.selected ? companyName(state.detail.company) : "取引先を読み込み中";
  document.title = `${name === "home" ? "営業ホーム" : name === "record" ? recordName : name === "tasks" ? "次回対応" : "取引先"} · meibo`;
  $("record-tab").hidden = state.selected === null;
  if (state.selected !== null) { $("record-tab-name").textContent = recordName; $("record-tab-name").setAttribute("aria-current", name === "record" ? "page" : "false"); }
  closeAppMenu();
}
function resetPages() { state.cursors = [null]; state.page = 0; state.next = null; state.count = null; }
function syncControls() {
  $("search").value = state.query; $("global-search").value = state.query;
  $("clear-search").hidden = !state.query; $("list-view").value = state.view;
  $("filter-status").value = state.filters.status || "";
  $("filter-prefecture").value = state.filters.prefecture || "";
  $("filter-phone").value = state.filters.has_phone || "";
  $("toggle-filters").classList.toggle("has-filters", Object.keys(state.filters).length > 0);
  if ($("list-heading")) $("list-heading").textContent = state.listContext === "tasks" ? "次回対応" : "取引先";
  if ($("object-title")) $("object-title").textContent = state.listContext === "tasks" ? "次回対応" : "取引先";
  $("accounts-title").textContent = state.listContext === "tasks" ? "次回対応" : "取引先";
  $("view-title").textContent = state.query ? `社名検索: ${state.query}` : $("list-view").selectedOptions[0].textContent + (Object.keys(state.filters).length ? " · 絞り込みあり" : "");
}
function navigate(name) {
  if (busy()) return;
  ++state.detailVersion; state.detailController?.abort();
  if (name === "home") {
    showWorkspace("home"); renderRecent(); loadSummary(); loadHomeDue(); return;
  }
  if (state.listContext !== name) {
    state.listContext = name; state.view = name === "tasks" ? "scheduled" : "all";
    state.query = ""; state.filters = {}; resetPages(); syncControls();
  }
  showWorkspace(name); loadList();
}

function readRecent() {
  try {
    const values = JSON.parse(sessionStorage.getItem("meibo-recent-companies") || "[]");
    return Array.isArray(values) ? values.filter((v) => Number.isSafeInteger(v.id) && v.id > 0 && typeof v.name === "string").slice(0, 6) : [];
  } catch (_) { return []; }
}
function rememberCompany(company) {
  state.recent = [{ id: company.id, name: companyName(company).slice(0, 256) }, ...state.recent.filter((v) => v.id !== company.id)].slice(0, 6);
  try { sessionStorage.setItem("meibo-recent-companies", JSON.stringify(state.recent)); } catch (_) {}
  renderRecent();
}
function companyLink(company, metadata) {
  const item = el("div", "home-list-item"); item.dataset.companyId = String(company.id);
  const name = button(companyName(company), "home-item-name", () => openCompany(company.id));
  name.setAttribute("aria-label", `${companyName(company)}の詳細を表示`);
  item.append(name); if (metadata) item.append(el("span", "home-item-meta", metadata)); return item;
}
function renderRecent() {
  const box = $("recent-records"); box.replaceChildren();
  if (!state.recent.length) box.append(el("p", "empty-state", "まだ開いた取引先はありません。取引先一覧から企業を選びましょう。"));
  else state.recent.forEach((company) => box.append(companyLink(company, `取引先 · ID ${company.id}`)));
}
function openFilteredList(view, filters = {}) {
  if (busy()) return;
  state.query = ""; state.view = view; state.filters = filters; state.listContext = "accounts";
  state.selected = null; state.detail = null; ++state.detailVersion; state.detailController?.abort();
  resetPages(); syncControls(); showWorkspace("accounts"); loadList();
}
function renderStatusChart(counts) {
  const box = $("status-chart"); box.replaceChildren();
  const maximum = Math.max(1, ...counts);
  statuses.forEach((label, index) => {
    const row = el("div", "status-chart-row");
    const title = index === 5 ? el("span", "status-chart-label", label) : button(label, "status-chart-label", () => openFilteredList("all", { status: String(index) }));
    const track = el("span", "status-chart-track"), bar = el("span", `status-chart-bar status-chart-${index}`);
    bar.style.width = `${Math.round((counts[index] || 0) / maximum * 100)}%`; track.append(bar);
    row.append(title, track, el("span", "status-chart-value", `${numberFormat.format(counts[index] || 0)} 社`)); box.append(row);
  });
}
async function loadSummary() {
  const version = ++state.summaryVersion; $("stats").setAttribute("aria-busy", "true");
  try {
    const data = await (await request("/api/summary")).json(); if (version !== state.summaryVersion) return;
    state.summary = data;
    $("total-count").textContent = numberFormat.format(data.total); $("active-count").textContent = numberFormat.format(data.active);
    $("phone-count").textContent = numberFormat.format(data.with_phone); $("due-count").textContent = numberFormat.format(data.due || 0);
    if ($("scheduled-count")) $("scheduled-count").textContent = numberFormat.format(data.scheduled || 0);
    renderStatusChart(Array.isArray(data.status_counts) ? data.status_counts : [0, 0, 0, 0, 0, 0]); $("summary-error").hidden = true;
  } catch (error) {
    if (version !== state.summaryVersion) return;
    $("summary-error").replaceChildren(el("span", "", `集計を読み込めませんでした。${error.message}`), button("再試行", "", loadSummary));
    $("summary-error").hidden = false;
  } finally { if (version === state.summaryVersion) $("stats").setAttribute("aria-busy", "false"); }
}
async function loadHomeDue() {
  const version = ++state.homeVersion, box = $("home-due-list"); box.replaceChildren(el("p", "empty-state", "対応予定を読み込んでいます…"));
  try {
    const data = await (await request("/api/companies?view=due&limit=5")).json(); if (version !== state.homeVersion) return;
    box.replaceChildren();
    if (!data.rows.length) box.append(el("p", "empty-state", "現在、期限を迎えた対応予定はありません。"));
    else data.rows.forEach((company) => box.append(companyLink(company, `${formatDate(company.next_action_at)} · ${statuses[company.status]}`)));
  } catch (error) {
    if (version !== state.homeVersion) return;
    box.replaceChildren(el("p", "empty-state", error.message), button("もう一度読み込む", "button button-secondary", loadHomeDue));
  }
}
function showListState(title, description, retry = false) {
  const box = $("list-state"); box.replaceChildren(); if (title) box.append(el("h3", "", title));
  box.append(el("p", "", description)); if (retry) box.append(button("もう一度読み込む", "button button-secondary", loadList)); box.hidden = false;
}
function renderRows() {
  const fragment = document.createDocumentFragment();
  for (const [index, company] of state.rows.entries()) {
    const row = el("tr", company.id === state.selected ? "is-selected" : ""); row.dataset.companyId = String(company.id);
    const order = el("td", "mobile-hide row-number", state.page * 25 + index + 1);
    const identity = el("td"); const name = button(companyName(company), "company-name", () => openCompany(company.id));
    name.setAttribute("aria-label", `${companyName(company)}の詳細を表示`); identity.append(name);
    const region = el("td", "region-column", prefectures[company.prefecture] || "未設定");
    const phone = el("td", "phone-column", company.phone || "—");
    const status = el("td"); status.append(statusBadge(company.status));
    const owner = el("td", "owner-column", company.owner === null || company.owner === undefined ? "未割当" : `担当 ${company.owner}`);
    const next = el("td", "next-column", company.next_action_at ? formatDate(company.next_action_at) : "—");
    const number = el("td", "number-column", company.corporate_number || "—");
    row.append(order, identity, region, phone, status, owner, next, number);
    row.addEventListener("click", (event) => { if (!event.target.closest("button")) openCompany(company.id); }); fragment.append(row);
  }
  $("company-rows").replaceChildren(fragment);
}
function renderPagination() {
  $("previous-page").disabled = state.loading || state.page === 0; $("next-page").disabled = state.loading || !state.next;
  $("page-number").textContent = String(state.page + 1);
  if (state.loading) { $("page-description").textContent = "読み込み中"; return; }
  const start = state.page * 25 + 1, end = state.page * 25 + state.rows.length;
  const range = state.rows.length ? `${numberFormat.format(start)}–${numberFormat.format(end)} 件を表示` : "0 件を表示";
  $("page-description").textContent = state.count === null ? range : `${numberFormat.format(state.count)} 件中 ${range}`;
  $("list-count").textContent = state.count === null ? "—" : `${numberFormat.format(state.count)} 件`;
}
async function loadList() {
  const version = ++state.listVersion; state.listController?.abort(); state.listController = new AbortController(); state.loading = true;
  $("table-container").setAttribute("aria-busy", "true"); $("company-rows").replaceChildren();
  $("list-state").replaceChildren(el("span", "loading-spinner"), el("p", "", "取引先を読み込んでいます…")); $("list-state").hidden = false; renderPagination();
  const params = new URLSearchParams({ limit: "25" });
  if (state.query) params.set("q", state.query); else {
    params.set("view", state.view); Object.entries(state.filters).forEach(([key, value]) => params.set(key, value));
  }
  if (state.cursors[state.page]) params.set("after", state.cursors[state.page]);
  try {
    const data = await (await request(`/api/companies?${params}`, { signal: state.listController.signal })).json(); if (version !== state.listVersion) return;
    state.rows = data.rows; state.next = data.next; if (data.count !== null && data.count !== undefined) state.count = data.count;
    renderRows(); if (state.rows.length) $("list-state").hidden = true;
    else showListState(state.query ? "該当する取引先がありません" : "該当する取引先がありません", "検索語や表示条件を変更してください。");
  } catch (error) {
    if (error.name === "AbortError" || version !== state.listVersion) return;
    state.rows = []; state.next = null; showListState("取引先を読み込めませんでした", error.message, true);
  } finally { if (version === state.listVersion) { state.loading = false; $("table-container").setAttribute("aria-busy", "false"); renderPagination(); } }
}
function searchCompanies(value) {
  if (busy()) return;
  const cleared = state.view !== "all" || Object.keys(state.filters).length;
  state.query = value.trim(); state.view = "all"; state.filters = {}; state.listContext = "accounts";
  state.selected = null; state.detail = null; ++state.detailVersion; state.detailController?.abort();
  resetPages(); syncControls(); showWorkspace("accounts"); loadList();
  if (cleared) notify("すべての取引先を検索するため、表示条件を解除しました。");
}
function applyListView() {
  if (busy()) { syncControls(); return; }
  const hadSearch = Boolean(state.query), hadFilters = Object.keys(state.filters).length > 0; state.view = $("list-view").value; state.query = ""; state.filters = {};
  if (state.listContext === "tasks" && state.view !== "scheduled") { state.listContext = "accounts"; showWorkspace("accounts"); }
  resetPages(); syncControls(); loadList(); if (hadSearch || hadFilters) notify("リストビューを切り替えるため、社名検索と個別の絞り込みを解除しました。");
}
function applyFilters(clear = false) {
  if (busy()) return;
  const hadSearch = Boolean(state.query); state.query = ""; state.view = state.listContext === "tasks" ? "scheduled" : "all";
  state.filters = clear ? {} : Object.fromEntries([["status", $("filter-status").value], ["prefecture", $("filter-prefecture").value], ["has_phone", $("filter-phone").value]].filter(([, value]) => value !== ""));
  resetPages(); syncControls(); loadList(); if (hadSearch) notify("絞り込みを適用するため、社名検索を解除しました。");
}

function getDraft(company) {
  if (!state.drafts.has(company.id)) state.drafts.set(company.id, { note: "", status: String(company.status), nextMode: "keep", nextDate: "" });
  return state.drafts.get(company.id);
}

function addFact(facts, label, value) {
  facts.append(el("dt", "", label), el("dd", "", value));
}

function section(title, iconName) {
  const element = el("section", "detail-section");
  const heading = el("div", "section-heading");
  heading.append(icon(iconName), el("h3", "", title));
  element.append(heading);
  return element;
}

function fieldLabel(label, id) {
  const result = el("label", "", label); result.htmlFor = id; return result;
}

function renderActivityForm(company) {
  const draft = getDraft(company);
  const container = section("架電を記録する", "phone");
  const form = el("form", "activity-form"); form.id = "activity-form";
  const fields = el("fieldset"); fields.disabled = state.saving;
  const note = el("textarea"); note.id = "activity-note"; note.name = "note"; note.required = true; note.maxLength = 10000;
  note.placeholder = "話した内容や、次回確認したいことをメモ…"; note.value = draft.note; note.rows = 3;
  note.addEventListener("input", () => { draft.note = note.value; });
  fields.append(fieldLabel("架電メモ", note.id), note);
  const row = el("div", "form-row");
  const statusField = el("div");
  const status = el("select"); status.id = "activity-status"; status.name = "status";
  statuses.forEach((label, value) => { const option = el("option", "", label); option.value = String(value); status.append(option); });
  status.value = draft.status;
  statusField.append(fieldLabel("営業状態", status.id), status);
  const nextField = el("div");
  const next = el("select"); next.id = "activity-next-mode"; next.name = "next_mode";
  [["keep", "現状のまま"], ["set", "日時を指定"], ["clear", "設定を解除"]].forEach(([value, label]) => { const option = el("option", "", label); option.value = value; next.append(option); });
  next.value = draft.nextMode;
  nextField.append(fieldLabel("次回対応", next.id), next);
  row.append(statusField, nextField); fields.append(row);
  const dateField = el("div", "date-field"); dateField.hidden = draft.nextMode !== "set";
  const nextDate = el("input"); nextDate.id = "activity-next-date"; nextDate.name = "next_date"; nextDate.type = "datetime-local"; nextDate.value = draft.nextDate; nextDate.required = draft.nextMode === "set";
  nextDate.addEventListener("input", () => { draft.nextDate = nextDate.value; });
  dateField.append(fieldLabel("次回対応の日時", nextDate.id), nextDate); fields.append(dateField);
  const help = el("p", "form-help", "「除外」にすると企業リストの表示対象から外れます。"); help.hidden = draft.status !== "5";
  status.addEventListener("change", () => { draft.status = status.value; help.hidden = status.value !== "5"; });
  next.addEventListener("change", () => { draft.nextMode = next.value; dateField.hidden = next.value !== "set"; nextDate.required = next.value === "set"; if (next.value === "set") nextDate.focus(); });
  fields.append(help);
  const error = el("div", "field-error"); error.id = "activity-error"; error.hidden = true; error.setAttribute("role", "alert");
  const save = el("button", "button button-primary save-button"); save.type = "submit"; save.append(icon("check"), el("span", "", state.saving ? "保存しています…" : "架電の記録を保存"));
  fields.append(error, save); form.append(fields);
  form.addEventListener("submit", (event) => { event.preventDefault(); saveActivity(company.id, form, draft); });
  container.append(form); return container;
}


function closeCompany() {
  if (busy()) return;
  const destination = state.workspace === "record" ? state.returnWorkspace : state.workspace;
  const id = state.selected; state.selected = null; state.detail = null; ++state.detailVersion; state.detailController?.abort();
  showWorkspace(destination); renderRows();
  const selector = destination === "home" ? `#recent-records [data-company-id="${id}"] button, #home-due-list [data-company-id="${id}"] button` : `#company-rows [data-company-id="${id}"] button`;
  const fallback = document.querySelector(`.workspace-navigation [data-workspace="${destination}"]`) || $("global-search");
  (document.querySelector(selector) || fallback).focus({ preventScroll: true });
}
function recordTabs(company, activities, contacts) {
  const main = el("section", "record-main"), tabs = el("div", "record-tabs");
  tabs.setAttribute("role", "tablist"); tabs.setAttribute("aria-label", "取引先の情報");
  const details = el("div", "record-details"); details.id = "record-details"; details.setAttribute("role", "tabpanel"); details.setAttribute("aria-labelledby", "record-tab-details");
  const related = el("div", "record-related"); related.id = "record-related"; related.setAttribute("role", "tabpanel"); related.setAttribute("aria-labelledby", "record-tab-related");
  const tabButtons = [];
  function selectTab(name, focus = false) {
    state.recordTab = name;
    tabButtons.forEach((control) => {
      const selected = control.dataset.recordTab === name;
      control.setAttribute("aria-selected", String(selected)); control.tabIndex = selected ? 0 : -1; control.classList.toggle("is-active", selected);
      if (selected && focus) control.focus();
    });
    details.hidden = name !== "details"; related.hidden = name !== "related";
  }
  for (const [name, label] of [["details", "詳細"], ["related", `関連 (${contacts.length + activities.length})`]]) {
    const control = button(label, "record-tab", () => selectTab(name)); control.id = `record-tab-${name}`;
    control.dataset.recordTab = name; control.setAttribute("role", "tab"); control.setAttribute("aria-controls", `record-${name}`);
    control.addEventListener("keydown", (event) => {
      if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
      event.preventDefault(); selectTab(event.key === "Home" ? "details" : event.key === "End" ? "related" : state.recordTab === "details" ? "related" : "details", true);
    });
    tabButtons.push(control); tabs.append(control);
  }
  const facts = el("dl", "detail-grid");
  for (const [label, value] of [
    ["取引先名", companyName(company)], ["営業状態", statuses[company.status] || "不明"],
    ["法人番号", company.corporate_number || "未登録"], ["担当", company.owner === null || company.owner === undefined ? "未割当" : `担当 ${company.owner}`],
    ["都道府県", prefectures[company.prefecture] || "未設定"], ["市区町村コード", company.municipality || "未登録"],
    ["電話番号", company.phone || "未登録"], ["Webサイト", company.website || "未登録"],
    ["最終活動", formatDate(company.last_activity_at)], ["次回対応", formatDate(company.next_action_at)]
  ]) { const field = el("div", "detail-field"); field.append(el("dt", "", label), el("dd", "", value)); facts.append(field); }
  details.append(el("h2", "card-heading", "取引先情報"), facts);
  if (company.is_sole_proprietor) details.append(el("p", "contacts-note", "個人事業の名称・電話番号・Webサイトは、この画面では非表示です。"));
  const contactCard = el("section", "related-card"); contactCard.append(el("h2", "card-heading", `担当者 (${contacts.length})`));
  if (!contacts.length) contactCard.append(el("p", "empty-state", "登録されている担当者はありません。"));
  else {
    const list = el("ul", "related-records");
    for (const contact of contacts) {
      const item = el("li", "related-record"); item.append(el("strong", "", contact.name || "個人情報は非表示"), el("span", "", contact.email || contact.phone || "連絡先は非表示")); list.append(item);
    }
    contactCard.append(list, el("p", "contacts-note", "担当者の個人情報は既定で非表示です。"));
  }
  const activityCard = el("section", "related-card"); activityCard.append(el("h2", "card-heading", `活動 (${activities.length})`));
  if (!activities.length) activityCard.append(el("p", "empty-state", "まだ活動がありません。右のフォームから架電を記録できます。"));
  else {
    const list = el("ul", "related-records");
    [...activities].sort((a, b) => b.at - a.at || b.id - a.id).slice(0, 5).forEach((activity) => {
      const item = el("li", "related-record"); item.append(el("strong", "", activityKinds[activity.kind] || "活動"), el("span", "", formatDate(activity.at)), el("p", "", activity.note ?? "メモなし / 非表示")); list.append(item);
    }); activityCard.append(list);
  }
  related.append(contactCard, activityCard); main.append(tabs, details, related); selectTab(state.recordTab); return main;
}
function renderTimeline(activities) {
  const history = section(`活動履歴 (${activities.length})`, "clock");
  if (!activities.length) { history.append(el("p", "no-activity", "まだ活動の記録がありません。最初の架電を記録しましょう。")); return history; }
  const timeline = el("ol", "timeline");
  for (const activity of [...activities].sort((a, b) => b.at - a.at || b.id - a.id)) {
    const item = el("li", "timeline-item"), meta = el("div", "timeline-meta");
    meta.append(el("span", "timeline-kind", activityKinds[activity.kind] || "活動"), el("time", "", formatDate(activity.at)));
    item.append(meta, el("p", "timeline-note", activity.note ?? "メモなし / 非表示"));
    if (activity.status_after !== null && activity.status_after !== undefined) item.append(el("p", "timeline-status", `営業状態を「${statuses[activity.status_after] || "不明"}」に変更`));
    timeline.append(item);
  }
  history.append(timeline); return history;
}
function renderDetail(data, focusHeading = false) {
  const { company, activities, contacts = [] } = data, container = $("detail-content"); container.replaceChildren();
  const header = el("header", "record-header"), breadcrumb = el("nav", "record-breadcrumb"); breadcrumb.setAttribute("aria-label", "パンくず");
  breadcrumb.append(button(state.returnWorkspace === "home" ? "ホーム" : "取引先一覧", "record-back", closeCompany), el("span", "", " / 取引先"));
  const titleRow = el("div", "record-title-row"), identity = el("div", "record-identity"), avatar = el("span", "object-icon"); avatar.append(icon("building"));
  const title = el("h1", "", companyName(company)); title.id = "selected-company-title"; title.tabIndex = -1;
  const titleText = el("div"); titleText.append(el("p", "object-label", "取引先"), title); identity.append(avatar, titleText);
  const actions = el("div", "record-actions"); actions.append(button("一覧に戻る", "button button-secondary", closeCompany), button("架電を記録", "button button-primary", () => { $("activity-note").focus(); $("activity-form").scrollIntoView({ block: "center", behavior: "smooth" }); }));
  titleRow.append(identity, actions);
  const highlights = el("dl", "record-highlights");
  for (const [label, value] of [["法人番号", company.corporate_number || "未登録"], ["電話番号", company.phone || "未登録"], ["所在地", prefectures[company.prefecture] || "未設定"], ["担当", company.owner === null || company.owner === undefined ? "未割当" : `担当 ${company.owner}`], ["営業状態", statuses[company.status] || "不明"]]) {
    const field = el("div", "highlight-field"); field.append(el("dt", "", label), el("dd", "", value)); highlights.append(field);
  }
  header.append(breadcrumb, titleRow, highlights);
  const path = el("ol", "record-path"); path.setAttribute("aria-label", "営業段階");
  for (const index of [0, 1, 2, 3]) {
    const step = el("li", `path-step ${company.status === index ? "is-current" : company.status < 4 && company.status > index ? "is-complete" : ""}`, statuses[index]);
    if (company.status === index) step.setAttribute("aria-current", "step"); path.append(step);
  }
  if (company.status >= 4) path.append(el("li", "path-outcome", `現在の状態: ${statuses[company.status]}`));
  const body = el("div", "record-body"), activity = el("aside", "record-activity"); activity.setAttribute("aria-label", "活動と架電記録");
  activity.append(renderActivityForm(company), renderTimeline(activities)); body.append(recordTabs(company, activities, contacts), activity);
  container.append(header, path, body); if (focusHeading) title.focus({ preventScroll: true });
}
async function openCompany(id, focusHeading = true) {
  if (busy()) return;
  const previous = state.selected; if (state.workspace !== "record") state.returnWorkspace = state.workspace;
  if (previous !== id) { state.recordTab = "details"; state.detail = null; }
  const version = ++state.detailVersion; state.detailController?.abort(); state.detailController = new AbortController();
  state.selected = id; showWorkspace("record"); $("record-workspace").setAttribute("aria-busy", "true");
  const loading = el("div", "panel-state detail-loading"); loading.setAttribute("role", "status");
  loading.append(el("p", "", "取引先の詳細を読み込んでいます…"), button("一覧へ戻る", "button button-secondary", closeCompany)); $("detail-content").replaceChildren(loading);
  try {
    const data = await (await request(`/api/companies/${encodeURIComponent(id)}`, { signal: state.detailController.signal })).json();
    if (version !== state.detailVersion || state.selected !== id) return;
    state.detail = data; rememberCompany(data.company); renderDetail(data, focusHeading); showWorkspace("record");
    if (focusHeading) window.scrollTo({ top: 0, behavior: "instant" });
  } catch (error) {
    if (error.name === "AbortError" || version !== state.detailVersion) return;
    const failure = el("div", "panel-state detail-error"); failure.setAttribute("role", "alert");
    failure.append(el("h1", "", "取引先を表示できません"), el("p", "", error.message), button("再試行", "button button-secondary", () => openCompany(id)), button("一覧に戻る", "button button-secondary", closeCompany)); $("detail-content").replaceChildren(failure);
  } finally { if (version === state.detailVersion) $("record-workspace").setAttribute("aria-busy", "false"); }
}

function parseLocalDate(value) {
  const match = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})$/.exec(value);
  if (!match) throw new Error("次回対応の日時を正しく入力してください。");
  const [year, month, day, hour, minute] = match.slice(1).map(Number);
  const date = new Date(year, month - 1, day, hour, minute);
  const seconds = date.getTime() / 1000;
  if (!Number.isSafeInteger(seconds) || seconds < 0 || date.getFullYear() !== year || date.getMonth() !== month - 1 || date.getDate() !== day || date.getHours() !== hour || date.getMinutes() !== minute) throw new Error("次回対応の日時を正しく入力してください。");
  return seconds;
}

async function saveActivity(id, form, draft) {
  if (state.saving || !form.reportValidity()) return;
  const errorBox = form.querySelector(".field-error"); errorBox.hidden = true;
  let body;
  try {
    const status = Number(draft.status);
    if (!Number.isInteger(status) || status < 0 || status > 5) throw new Error("営業状態を選び直してください。");
    if (!draft.note.trim()) throw new Error("架電メモを入力してください。");
    body = { note: draft.note.trim(), status };
    if (draft.nextMode === "set") body.next_action_at = parseLocalDate(draft.nextDate);
    else if (draft.nextMode === "clear") body.next_action_at = null;
    else if (draft.nextMode !== "keep") throw new Error("次回対応の設定を選び直してください。");
  } catch (error) { errorBox.textContent = error.message; errorBox.hidden = false; return; }
  state.saving = true;
  form.querySelector("fieldset").disabled = true;
  const saveLabel = form.querySelector(".save-button span"); saveLabel.textContent = "保存しています…";
  form.setAttribute("aria-busy", "true");
  notify("架電の記録を保存しています…");
  try {
    await request(`/api/companies/${encodeURIComponent(id)}/activities`, { method: "POST", headers: { "Content-Type": "application/json", "X-Meibo-Local": "1" }, body: JSON.stringify(body) });
    state.drafts.delete(id);
    state.saving = false;
    notify("架電の記録を保存しました。");
    await Promise.all([loadSummary(), loadHomeDue(), loadList(), state.selected === id ? openCompany(id, false) : Promise.resolve()]);
  } catch (error) {
    errorBox.textContent = `保存できませんでした。${error.message} 入力内容を残しています。もう一度保存できます。`;
    errorBox.hidden = false; notify("架電の記録を保存できませんでした。入力内容をご確認ください。", true);
  } finally {
    state.saving = false;
    form.querySelector("fieldset").disabled = false;
    saveLabel.textContent = "架電の記録を保存";
    form.setAttribute("aria-busy", "false");
  }
}


function populatePrefectures() {
  for (const id of ["filter-prefecture", "company-prefecture"]) {
    const select = $(id);
    prefectures.slice(1).forEach((name, index) => { const option = el("option", "", name); option.value = String(index + 1); select.append(option); });
  }
}
function openCompanyDialog() {
  if (busy()) return;
  $("company-form-error").hidden = true;
  $("company-dialog").showModal(); $("company-name").focus();
}
function closeCompanyDialog() {
  if (state.creating) { notify("取引先を保存しています。完了までお待ちください。"); return; }
  $("company-dialog").close(); $("new-company").focus();
}
async function createCompany(event) {
  event.preventDefault();
  const form = $("company-form"), errorBox = $("company-form-error");
  if (state.creating || state.saving || !form.reportValidity()) return;
  const name = $("company-name").value.trim(), prefecture = Number($("company-prefecture").value);
  if (!name || !Number.isInteger(prefecture) || prefecture < 1 || prefecture > 47) {
    errorBox.textContent = "取引先名と都道府県を入力してください。"; errorBox.hidden = false; return;
  }
  const body = { name, prefecture };
  for (const [key, id] of [["corporate_number", "company-number"], ["phone", "company-phone"], ["website", "company-website"]]) {
    const value = $(id).value.trim(); if (value) body[key] = value;
  }
  state.creating = true; errorBox.hidden = true; $("company-fields").disabled = true;
  $("company-submit").disabled = true; $("company-submit").textContent = "保存しています…";
  $("company-cancel").disabled = true; $("company-dialog-close").disabled = true; form.setAttribute("aria-busy", "true");
  try {
    const data = await (await request("/api/companies", { method: "POST", headers: { "Content-Type": "application/json", "X-Meibo-Local": "1" }, body: JSON.stringify(body) })).json();
    if (!Number.isSafeInteger(data.id) || data.id <= 0) throw new Error("保存結果を確認できません。更新してからご確認ください。");
    $("company-dialog").close(); form.reset();
    notify("取引先を登録しました。");
    await Promise.all([loadSummary(), loadHomeDue(), loadList()]);
    state.creating = false; await openCompany(data.id);
  } catch (error) {
    errorBox.textContent = `登録できませんでした。${error.message} 入力内容を残しています。`; errorBox.hidden = false;
  } finally {
    state.creating = false; $("company-fields").disabled = false; $("company-submit").disabled = false;
    $("company-submit").textContent = "保存"; $("company-cancel").disabled = false; $("company-dialog-close").disabled = false;
    form.setAttribute("aria-busy", "false");
  }
}

populatePrefectures(); syncControls(); showWorkspace("accounts"); renderRecent();
document.querySelectorAll("[data-workspace]").forEach((control) => control.addEventListener("click", () => navigate(control.dataset.workspace)));
$("record-tab-name").addEventListener("click", () => { if (!busy() && state.selected !== null) { const id = state.selected; showWorkspace("record"); openCompany(id); } });
$("close-record-tab").addEventListener("click", closeCompany);
$("app-launcher").addEventListener("click", () => {
  const expanded = $("app-menu").hidden; $("app-menu").hidden = !expanded; $("app-launcher").setAttribute("aria-expanded", String(expanded));
  if (expanded) $("app-menu").querySelector("button").focus();
});
document.addEventListener("click", (event) => { if (!event.target.closest(".app-switcher")) closeAppMenu(); });
$("global-search-form").addEventListener("submit", (event) => { event.preventDefault(); searchCompanies($("global-search").value); });
$("search-form").addEventListener("submit", (event) => { event.preventDefault(); searchCompanies($("search").value); });
$("search").addEventListener("input", () => { $("clear-search").hidden = !$("search").value; });
$("search").addEventListener("search", () => { if (!$("search").value && state.query) searchCompanies(""); });
$("clear-search").addEventListener("click", () => { searchCompanies(""); $("search").focus(); });
$("list-view").addEventListener("change", applyListView);
$("toggle-filters").addEventListener("click", () => {
  const opening = $("filters-panel").hidden; $("filters-panel").hidden = !opening;
  $("toggle-filters").setAttribute("aria-expanded", String(opening));
  if (opening) $("filter-status").focus();
});
$("apply-filters").addEventListener("click", () => applyFilters()); $("clear-filters").addEventListener("click", () => applyFilters(true));
$("previous-page").addEventListener("click", () => { if (!state.loading && state.page > 0) { state.page -= 1; loadList(); } });
$("next-page").addEventListener("click", () => { if (!state.loading && state.next) { state.cursors[state.page + 1] = state.next; state.page += 1; loadList(); } });
$("refresh").addEventListener("click", async () => {
  if (busy()) return; $("refresh").disabled = true;
  try { await Promise.all([loadSummary(), loadHomeDue(), loadList()]); }
  finally { $("refresh").disabled = false; }
});
$("export").addEventListener("click", async () => {
  const control = $("export"), label = control.querySelector("span"); control.disabled = true; label.textContent = "書き出し中…";
  try {
    const response = await request("/api/export"), blob = await response.blob(), url = URL.createObjectURL(blob);
    const link = el("a"); link.href = url; link.download = "meibo-companies.csv"; document.body.append(link); link.click(); link.remove();
    setTimeout(() => URL.revokeObjectURL(url), 10000);
    notify("営業対象の全取引先をCSVに書き出しました。表示条件・社名検索はCSVには適用されません。個人事業と除外した企業は含まれません。");
  } catch (error) { notify(`CSVを書き出せませんでした。${error.message}`, true); }
  finally { control.disabled = false; label.textContent = "CSVを書き出す"; }
});
$("new-company").addEventListener("click", openCompanyDialog); $("company-form").addEventListener("submit", createCompany);
$("company-cancel").addEventListener("click", closeCompanyDialog); $("company-dialog-close").addEventListener("click", closeCompanyDialog);
$("company-dialog").addEventListener("cancel", (event) => { if (state.creating) event.preventDefault(); });
$("company-dialog").addEventListener("close", () => { if (state.workspace !== "record") $("new-company").focus(); });
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape") {
    if ($("company-dialog").open) return;
    if (!$("app-menu").hidden) { closeAppMenu(); $("app-launcher").focus(); }
    else if (state.workspace === "record") closeCompany();
  }
  if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
    event.preventDefault(); if (busy() || $("company-dialog").open) return; $("global-search").focus();
  }
});
Promise.all([loadSummary(), loadHomeDue(), loadList()]);
