"use strict";

(() => {
  const $ = (id) => document.getElementById(id);
  const status = $("usage-status");
  const refresh = $("usage-refresh");
  const content = $("usage-content");
  const emptyContent = content.cloneNode(true);
  let data = null;
  let range = "month";
  let generation = 0;
  let allDays = false;

  const integer = new Intl.NumberFormat("ja-JP", { maximumFractionDigits: 0 });
  const decimal = new Intl.NumberFormat("ja-JP", { maximumFractionDigits: 1 });
  const tenth = new Intl.NumberFormat("ja-JP", { minimumFractionDigits: 1, maximumFractionDigits: 1 });
  const percent = new Intl.NumberFormat("ja-JP", { style: "percent", maximumFractionDigits: 1 });
  const number = (value) => (Number.isFinite(value) ? value : 0);
  const yen = (value) => {
    // 0 is "0円", under 1 yen "1円未満", under 10 one decimal, else whole yen.
    const amount = number(value);
    if (amount <= 0) return "0円";
    if (amount < 1) return "1円未満";
    const rounded = Math.round(amount * 10) / 10;
    return rounded < 10 ? `${tenth.format(rounded)}円` : `${integer.format(Math.round(amount))}円`;
  };
  const count = (value) => integer.format(number(value));
  const seconds = (value) => decimal.format(number(value));
  const dayLabel = (day) => {
    const [, month, date] = String(day).split("-");
    return `${Number(month)}月${Number(date)}日`;
  };

  function element(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = text;
    return node;
  }

  function showError(message) {
    status.textContent = message;
    status.classList.add("is-error");
    status.hidden = false;
  }

  async function request(path, headers = {}) {
    return fetch(path, { cache: "no-store", credentials: "same-origin", headers: { "X-Requested-With": "XMLHttpRequest", ...headers } });
  }

  function clearUsage() {
    generation++;
    refresh.disabled = false;
    content.hidden = true;
    $("usage-unconfigured").hidden = true;
    data = null;
    content.replaceChildren(...emptyContent.cloneNode(true).childNodes);
    status.textContent = "";
    status.hidden = true;
  }

  async function load() {
    clearUsage();
    if (document.hidden) return;
    const current = generation;
    const active = () => current === generation && !document.hidden;
    refresh.disabled = true;
    status.classList.remove("is-error");
    status.textContent = "読み込んでいます…";
    status.hidden = false;
    try {
      const session = await request("/auth/session");
      if (!active()) return;
      if (session.status === 401) { window.location.replace("/login"); return; }
      if (!session.ok) throw new Error("ログインの状態を確認できませんでした。");
      const identity = await session.json();
      if (!active()) return;
      if (identity.role !== "admin") { showError("この画面は管理者だけが使えます。"); return; }
      if (!identity.storage_scope) throw new Error("ログインの状態を確認できませんでした。");
      const response = await request("/api/admin/usage", { "X-Meibo-Context": identity.storage_scope });
      if (!active()) return;
      if (response.status === 401) { window.location.replace("/login"); return; }
      const body = await response.json().catch(() => ({}));
      if (!active()) return;
      if (!response.ok) throw new Error(typeof body.error === "string" ? body.error : "利用量を読み込めませんでした。");
      data = body;
      render();
    } catch (failure) {
      if (active()) {
        clearUsage();
        showError(failure.message || "接続できませんでした。もう一度お試しください。");
      }
    } finally {
      if (active()) refresh.disabled = false;
    }
  }

  function render() {
    if (!data.configured) {
      $("usage-unconfigured").hidden = false;
      $("usage-content").hidden = true;
      status.textContent = "";
      status.hidden = true;
      return;
    }
    $("usage-unconfigured").hidden = true;
    status.textContent = `${dayLabel(data.today)}時点の目安です。`;
    renderMonth();
    renderFree();
    renderGroups();
    renderDays();
    const [checkedYear] = String(data.price_checked).split("-");
    $("price-note").textContent = `単価と無料枠は${checkedYear}年${dayLabel(data.price_checked)}に確認した値です（Cloud Run と Turso の無料プラン）。円は1ドル=${integer.format(data.jpy_per_usd)}円で換算しています。`;
    $("unsaved-note").textContent = `集計は約${integer.format(data.flush_seconds)}秒ごとに、書き込みを伴う操作のときにまとめて保存します。保存前の分は、サーバーが止まると失われることがあります。`;
    content.hidden = false;
  }

  function verdict() {
    const estimate = yen(data.totals.jpy_after_free);
    const free = data.free || [];
    const over = free.filter((item) => item.exceeded);
    const near = free.filter((item) => number(item.ratio) >= 0.8);
    const [year, month, date] = String(data.today).split("-").map(Number);
    const daysInMonth = new Date(Date.UTC(year, month, 0)).getUTCDate();
    const pace = date > 0 ? daysInMonth / date : 1;
    const projected = free.filter((item) => number(item.ratio) * pace > 1);
    const names = (items) => items.map((item) => item.label).join("・");
    if (over.length) return { warn: true, text: `無料枠を超えた項目があります（${names(over)}）。今月の請求の見込みは${estimate}です。` };
    if (near.length) return { warn: true, text: `無料枠の80%を超えた項目があります（${names(near)}）。今月の請求の見込みは今のところ${estimate}です。` };
    if (projected.length) return { warn: true, text: `無料枠の内側ですが、このペースだと月末に超える見込みです（${names(projected)}）。今月の請求の見込みは今のところ${estimate}です。` };
    return { warn: false, text: `無料枠の内側です。今月の請求の見込みは${estimate}です。` };
  }

  function renderMonth() {
    const totals = data.totals;
    const [year, month] = String(data.month).split("-");
    $("month-name").textContent = `（${year}年${Number(month)}月）`;
    const result = verdict();
    $("month-verdict").textContent = result.text;
    $("month-verdict").classList.toggle("is-warn", result.warn);
    $("month-after-free").textContent = yen(totals.jpy_after_free);
    $("month-jpy").textContent = yen(totals.jpy);
    $("month-detail").textContent = `要求 ${count(totals.requests)}件・処理 ${seconds(totals.seconds)}秒・保存先（Turso）との通信 ${count(totals.round_trips)}回`;
    const cap = $("cap");
    if (data.cap_jpy && Number.isFinite(totals.cap_ratio)) {
      cap.hidden = false;
      const ratio = totals.cap_ratio;
      const bar = $("cap-bar");
      bar.style.width = `${Math.min(100, ratio * 100)}%`;
      bar.parentElement.classList.toggle("is-warn", ratio >= 0.8);
      $("cap-text").textContent = `月の上限 ${integer.format(data.cap_jpy)}円に対して、引く前の目安は ${percent.format(ratio)}${totals.cap_exceeded ? "（上限を超えています）" : ratio >= 0.8 ? "（上限の80%を超えています）" : ""}`;
    } else {
      cap.hidden = true;
    }
    $("unsaved").textContent = data.unsaved_requests > 0
      ? `まだ保存していない要求が ${count(data.unsaved_requests)}件あります（上の数字に含めています）。`
      : "表示している数字はすべて保存済みです。";
  }

  function renderFree() {
    const list = $("free-list");
    list.replaceChildren();
    for (const item of data.free) {
      const ratio = number(item.ratio);
      const row = element("li");
      const head = element("div", "usage-meter-head");
      head.append(element("span", "", item.label), element("strong", "", percent.format(ratio)));
      const meter = element("div", "usage-meter");
      meter.setAttribute("role", "img");
      meter.setAttribute("aria-label", `${item.label}の使用率 ${percent.format(ratio)}`);
      const bar = element("span");
      bar.style.width = `${Math.min(100, ratio * 100)}%`;
      meter.append(bar);
      row.append(head, meter, element("span", "usage-meter-sub", `${count(item.used)} / ${count(item.limit)}`));
      if (ratio >= 0.8) {
        meter.classList.add("is-warn");
        row.append(element("span", "usage-warn", item.exceeded ? "無料枠を超えています。" : "無料枠の80%を超えています。"));
      }
      list.append(row);
    }
  }

  function renderGroups() {
    content.querySelectorAll(".usage-range button").forEach((button) => button.setAttribute("aria-pressed", String(button.dataset.range === range)));
    const rows = $("group-rows");
    rows.replaceChildren();
    const items = (data.groups[range] || []).filter((item) => number(item.requests) > 0);
    $("group-empty").hidden = items.length > 0;
    items.forEach((item, index) => {
      const row = element("tr");
      const name = element("th", "", item.label);
      name.scope = "row";
      const toggleCell = element("td", "toggle-col");
      const toggle = element("button", "", "詳細");
      toggle.type = "button";
      toggle.setAttribute("aria-expanded", "false");
      const detailId = `group-detail-${index}`;
      toggle.setAttribute("aria-controls", detailId);
      toggle.setAttribute("aria-label", `${item.label}の詳細`);
      toggleCell.append(toggle);
      row.append(name,
        element("td", "num", count(item.requests)),
        element("td", "num wide", seconds(item.seconds)),
        element("td", "num wide", count(item.round_trips)),
        element("td", "num wide", count(item.rows_read)),
        element("td", "num wide", count(item.rows_written)),
        element("td", "num", yen(item.jpy)),
        toggleCell);
      const detail = element("tr", "usage-detail-row");
      detail.id = detailId;
      detail.hidden = true;
      const cell = element("td");
      cell.colSpan = 8;
      const list = element("dl");
      for (const [label, value] of [["処理（秒）", seconds(item.seconds)], ["保存先（Turso）との通信", `${count(item.round_trips)}回`], ["読み取り行", count(item.rows_read)], ["書き込み行", count(item.rows_written)]]) {
        list.append(element("dt", "", label), element("dd", "", value));
      }
      cell.append(list);
      detail.append(cell);
      toggle.addEventListener("click", () => {
        const open = detail.hidden;
        detail.hidden = !open;
        toggle.setAttribute("aria-expanded", String(open));
        toggle.textContent = open ? "閉じる" : "詳細";
      });
      rows.append(row, detail);
    });
  }

  function renderDays() {
    const chart = $("day-chart");
    chart.replaceChildren();
    const days = data.days || [];
    const highest = Math.max(0, ...days.map((day) => number(day.jpy)));
    const peak = days.reduce((best, day) => (number(day.jpy) > number(best?.jpy) ? day : best), null);
    let total = 0;
    days.forEach((day, index) => {
      total += number(day.jpy);
      const item = element("li");
      const height = highest > 0 ? Math.max(1, (number(day.jpy) / highest) * 100) : 1;
      const bar = element("span", day.day === data.today ? "usage-day-bar is-today" : "usage-day-bar");
      bar.style.height = `${height}%`;
      item.append(bar);
      if (day.day === data.today || (peak && day === peak && number(peak.jpy) > 0)) {
        const position = index < 3 ? " at-start" : index > days.length - 4 ? " at-end" : "";
        const label = element("span", `usage-day-value${position}`, yen(day.jpy));
        label.style.bottom = `calc(${height}% + 2px)`;
        item.append(label);
      }
      chart.append(item);
    });
    $("day-first").textContent = days.length ? dayLabel(days[0].day) : "";
    $("day-last").textContent = days.length ? `${dayLabel(days[days.length - 1].day)}（今日）` : "";
    $("day-summary").textContent = peak && number(peak.jpy) > 0
      ? `31日間の合計は${yen(total)}。いちばん多かったのは${dayLabel(peak.day)}の${yen(peak.jpy)}です。`
      : "直近31日の記録はまだありません。";
    renderDayRows();
  }

  function renderDayRows() {
    const table = $("day-rows");
    table.replaceChildren();
    const newest = [...(data.days || [])].reverse();
    for (const day of allDays ? newest : newest.slice(0, 7)) {
      const row = element("tr");
      const label = element("th", "", dayLabel(day.day));
      label.scope = "row";
      row.append(label, element("td", "num", count(day.requests)), element("td", "num wide", seconds(day.seconds)), element("td", "num", yen(day.jpy)));
      table.append(row);
    }
    const more = $("day-all");
    more.setAttribute("aria-expanded", String(allDays));
    more.textContent = allDays ? "直近7日だけ表示" : `すべて表示（${newest.length}日）`;
  }

  content.addEventListener("click", (event) => {
    if (!data?.configured) return;
    const button = event.target.closest("button");
    if (button?.id === "day-all") {
      allDays = !allDays;
      renderDayRows();
    } else if (button?.dataset.range) {
      range = button.dataset.range;
      renderGroups();
    }
  });
  refresh.addEventListener("click", load);
  window.addEventListener("pagehide", clearUsage);
  window.addEventListener("pageshow", (event) => { if (event.persisted) load(); });
  window.addEventListener("focus", load);
  document.addEventListener("visibilitychange", () => { if (document.hidden) clearUsage(); else load(); });
  window.addEventListener("storage", (event) => { if (event.key === "meibo-auth-change" || event.key === null) load(); });
  load();
})();
