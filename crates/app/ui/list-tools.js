"use strict";

(() => {
  const storageKey = "meibo-list-views-v1", maximumFileSize = 32 * 1024 * 1024;
  const columns = [
    ["row", "行番号"], ["name", "取引先名"], ["region", "所在地"], ["phone", "電話番号"],
    ["status", "営業状態"], ["owner", "担当"], ["next", "次回対応"], ["number", "法人番号"]
  ];
  const sorts = [["default", "既定の順序"], ["next_action", "次回対応が早い順"], ["last_activity", "最終活動が新しい順"]];
  const presetViews = ["all", "untouched", "negotiating", "has_phone", "scheduled"];
  let selectedColumns = columns.map(([key]) => key), views = [], importRunning = false;
  let savedSelect, sortSelect, nameInput, viewsMessage, columnInputs;

  function validView(value) {
    if (!value || typeof value !== "object" || Array.isArray(value)) return false;
    if (typeof value.name !== "string" || !value.name.trim() || value.name.length > 64 || /[\u0000-\u001f\u007f]/.test(value.name)) return false;
    if (!presetViews.includes(value.view) || typeof value.query !== "string" || value.query.length > 200
        || /[\u0000-\u001f\u007f]/.test(value.query)) return false;
    if (!sorts.some(([key]) => key === value.sort)) return false;
    if (!value.filters || typeof value.filters !== "object" || Array.isArray(value.filters)) return false;
    for (const [key, filter] of Object.entries(value.filters)) {
      if (key === "status" && typeof filter === "string" && /^[0-4]$/.test(filter)) continue;
      if (key === "prefecture" && typeof filter === "string" && /^(?:[1-9]|[1-3][0-9]|4[0-7])$/.test(filter)) continue;
      if (key === "has_phone" && ["true", "false"].includes(filter)) continue;
      return false;
    }
    if (value.query && (value.view !== "all" || Object.keys(value.filters).length || value.sort !== "default")) return false;
    return Array.isArray(value.columns) && value.columns.length >= 1 && value.columns.length <= 8
      && value.columns.includes("name") && new Set(value.columns).size === value.columns.length
      && value.columns.every((key) => columns.some(([known]) => key === known));
  }

  function configuration(value) {
    return { name: value.name.trim(), query: value.query, view: value.view,
      filters: Object.fromEntries(Object.entries(value.filters)), sort: value.sort, columns: [...value.columns] };
  }
  function readViews() {
    try {
      const raw = localStorage.getItem(storageKey);
      if (raw === null) return;
      if (raw.length > 65536) throw new Error("invalid storage");
      const data = JSON.parse(raw);
      if (!data || data.version !== 1 || !Array.isArray(data.views) || data.views.length > 20
          || !data.views.every(validView) || new Set(data.views.map((view) => view.name.trim())).size !== data.views.length) {
        throw new Error("invalid storage");
      }
      views = data.views.map(configuration);
    } catch (_) {
      views = [];
      try { localStorage.removeItem(storageKey); } catch (_) {}
    }
  }
  function writeViews(next) {
    try { localStorage.setItem(storageKey, JSON.stringify({ version: 1, views: next })); views = next; return true; }
    catch (_) { notify("ビューを保存できませんでした。ブラウザの保存設定と空き容量を確認してください。", true); return false; }
  }
  function refreshSavedViews(selected = "") {
    savedSelect.replaceChildren(new Option("保存したビューを選択", ""));
    views.forEach((view) => savedSelect.add(new Option(view.name, view.name)));
    savedSelect.value = selected;
  }
  function applyColumns() {
    document.querySelectorAll("#table-container .company-table").forEach((table) => {
      table.classList.add("list-columns-configured");
      columns.forEach(([key]) => table.classList.toggle(`list-hide-${key}`, !selectedColumns.includes(key)));
    });
    $("table-container")?.classList.add("list-columns-container");
    columnInputs?.forEach((input) => { input.checked = selectedColumns.includes(input.value); });
  }
  function sync() {
    if (sortSelect) {
      sortSelect.value = sorts.some(([key]) => key === state.sort) ? state.sort : "default";
      sortSelect.disabled = Boolean(state.query);
      sortSelect.title = state.query ? "社名検索では既定の検索順序を使います" : "一覧の順序";
    }
    applyColumns();
  }
  function restoreView() {
    const view = views.find((item) => item.name === savedSelect.value);
    if (!view || busy()) return;
    state.query = view.query; state.view = view.view; state.filters = { ...view.filters }; state.sort = view.sort;
    selectedColumns = [...view.columns]; state.listContext = "accounts";
    state.selected = null; state.detail = null; ++state.detailVersion; state.detailController?.abort();
    resetPages(); syncControls(); sync(); showWorkspace("accounts"); loadList();
    viewsMessage.textContent = `「${view.name}」を復元しました。このブラウザだけの設定です。`;
  }
  function field(label, control) {
    const box = el("div", "form-field"), text = el("label", "", label);
    text.htmlFor = control.id; box.append(text, control); return box;
  }
  function select(id, options) {
    const control = el("select"); control.id = id;
    options.forEach(([value, label]) => control.add(new Option(label, value))); return control;
  }
  function dialog(title, id) {
    const box = el("dialog", "company-dialog list-tool-dialog"); box.id = id;
    const form = el("form"), header = el("header", "dialog-header"), heading = el("h2", "", title);
    heading.id = `${id}-title`; box.setAttribute("aria-labelledby", heading.id);
    const close = button("×", "icon-button", () => box.close()); close.setAttribute("aria-label", `${title}を閉じる`);
    header.append(heading, close); form.append(header); box.append(form); document.body.append(box);
    box.addEventListener("keydown", (event) => event.stopPropagation());
    return { box, form, close };
  }
  function mountSaveDialog() {
    const item = dialog("このブラウザにビューを保存", "save-view-dialog");
    const fields = el("fieldset", "dialog-fields"); nameInput = el("input"); nameInput.id = "saved-view-name";
    nameInput.required = true; nameInput.maxLength = 64; nameInput.autocomplete = "off";
    fields.append(field("ビュー名", nameInput));
    const help = el("p", "dialog-description", "条件・順序・表示列を最大20件保存します。取引先のデータや活動メモは保存しません。チームには共有されません。");
    const footer = el("footer", "dialog-footer"), save = el("button", "button button-primary", "保存"); save.type = "submit";
    footer.append(button("キャンセル", "button button-secondary", () => item.box.close()), save);
    item.form.append(help, fields, footer);
    item.form.addEventListener("submit", (event) => {
      event.preventDefault(); const name = nameInput.value.trim();
      const view = { name, query: state.query, view: state.view, filters: { ...state.filters },
        sort: state.query ? "default" : state.sort || "default", columns: [...selectedColumns] };
      if (!validView(view)) { nameInput.setCustomValidity("ビュー名と一覧の条件を確認してください。"); nameInput.reportValidity(); return; }
      const exists = views.some((entry) => entry.name === name);
      if (!exists && views.length >= 20) { nameInput.setCustomValidity("保存できるビューは20件までです。先に不要なビューを削除してください。"); nameInput.reportValidity(); return; }
      const next = views.filter((entry) => entry.name !== name); next.push(configuration(view));
      if (!writeViews(next)) return;
      refreshSavedViews(name); item.box.close(); viewsMessage.textContent = `「${name}」を${exists ? "更新" : "保存"}しました。このブラウザだけの設定です。`;
    });
    nameInput.addEventListener("input", () => nameInput.setCustomValidity(""));
    return () => { if (busy()) return; nameInput.value = savedSelect.value; nameInput.setCustomValidity(""); item.box.showModal(); nameInput.focus(); };
  }
  function mountImportDialog() {
    const item = dialog("CSVを取り込む", "csv-import-dialog");
    const description = el("p", "dialog-description", "合成データだけを取り込めます。最大32MiB。500行または8MiBごとに確定し、途中で失敗しても確定済みの取引先は残ります。");
    description.id = "csv-import-description"; item.box.setAttribute("aria-describedby", description.id);
    const fields = el("fieldset", "dialog-fields"), file = el("input"); file.id = "csv-import-file";
    file.type = "file"; file.accept = ".csv,text/csv"; file.required = true;
    const encoding = select("csv-import-encoding", [["utf8", "UTF-8"], ["shift-jis", "Shift_JIS（Windows-31J）"]]);
    const duplicate = select("csv-import-duplicate", [["error", "同じ法人番号があれば中止"], ["skip", "同じ法人番号はスキップ"]]);
    const fileField = field("CSVファイル", file); fileField.classList.add("full-width");
    fields.append(fileField, field("文字コード", encoding), field("重複時の処理", duplicate));
    const guide = el("details", "csv-import-guide"), summary = el("summary", "", "CSVのヘッダーと入力例");
    const header = "origin,corporate_number,name,prefecture,municipality,industry,phone,website,owner,status,is_sole_proprietor,source_id,source_as_of";
    guide.append(summary, el("p", "", "必須列は origin・name・prefecture。origin は synthetic、prefecture は1〜47です。列名は下記の固定項目だけを使い、不要な列は省略できます。"),
      el("pre", "", header), el("pre", "", "origin,name,prefecture\nsynthetic,合成あおぞら商事,13"));
    const template = button("UTF-8のテンプレートを保存", "button button-secondary", () => {
      const url = URL.createObjectURL(new Blob(["\ufeff", header, "\r\n"], { type: "text/csv;charset=utf-8" }));
      const link = el("a"); link.href = url; link.download = "meibo-synthetic-template.csv"; document.body.append(link); link.click(); link.remove();
      setTimeout(() => URL.revokeObjectURL(url), 10000);
    });
    guide.append(template);
    const result = el("div", "csv-import-result"); result.id = "csv-import-result"; result.setAttribute("role", "status"); result.hidden = true;
    const footer = el("footer", "dialog-footer"), submit = el("button", "button button-primary", "取り込む"); submit.type = "submit";
    const cancel = button("閉じる", "button button-secondary", () => item.box.close()); footer.append(cancel, submit);
    item.form.append(description, fields, guide, result, footer);
    item.box.addEventListener("cancel", (event) => { if (importRunning) event.preventDefault(); });
    item.close.addEventListener("click", (event) => { if (importRunning) event.preventDefault(); });
    item.form.addEventListener("submit", async (event) => {
      event.preventDefault(); if (importRunning || busy()) return;
      const selected = file.files[0]; if (!selected) return;
      result.hidden = false; result.classList.remove("is-error");
      if (selected.size === 0 || selected.size > maximumFileSize) {
        result.textContent = "空でない32MiB以下のCSVファイルを選んでください。"; result.classList.add("is-error"); return;
      }
      importRunning = true; state.saving = true; fields.disabled = true; submit.disabled = true; cancel.disabled = true; item.close.disabled = true;
      submit.textContent = "取り込み中…"; result.textContent = "CSVを取り込んでいます。この画面を閉じずにお待ちください。";
      try {
        const params = new URLSearchParams({ encoding: encoding.value, duplicate: duplicate.value });
        const response = await fetch(`/api/import?${params}`, { method: "POST", cache: "no-store",
          headers: { "Content-Type": "text/csv", "X-Meibo-Local": "1" }, body: selected });
        const data = await response.json();
        const inserted = Number.isSafeInteger(data.inserted) && data.inserted >= 0 ? data.inserted : 0;
        const skipped = Number.isSafeInteger(data.skipped) && data.skipped >= 0 ? data.skipped : 0;
        const counts = `確定済み ${numberFormat.format(inserted)}件、重複スキップ ${numberFormat.format(skipped)}件。`;
        if (!response.ok || data.ok !== true) {
          result.classList.add("is-error");
          const row = Number.isSafeInteger(data.row) && data.row > 0 ? `CSVの${data.row}レコード目で停止しました（ヘッダーを1と数えます）。` : "取り込めませんでした。";
          result.textContent = `${row}${counts}${inserted ? "確定済みの取引先は保存されています。" : "新しい取引先は保存されていません。"} ${typeof data.error === "string" ? data.error : "CSVの形式を確認してください。"}`;
        } else { result.textContent = `CSV取込が完了しました。${counts}`; notify(`CSVから${numberFormat.format(inserted)}件を登録しました。`); }
        if (inserted > 0) { resetPages(); await Promise.all([loadList(), loadSummary(), loadHomeDue()]); }
      } catch (_) {
        result.classList.add("is-error");
        result.textContent = "通信結果を確認できませんでした。途中まで保存された可能性があります。再送の前に一覧を更新し、確定済みの取引先を確認してください。選択したファイルと設定は保持しています。";
      } finally {
        importRunning = false; state.saving = false; fields.disabled = false; submit.disabled = false; cancel.disabled = false; item.close.disabled = false; submit.textContent = "取り込む";
      }
    });
    return () => { if (busy()) return; item.box.showModal(); };
  }

  function mount() {
    const container = $("accounts-workspace"), filters = $("filters-panel"); if (!container || !filters) return;
    readViews(); if (!sorts.some(([key]) => key === state.sort)) state.sort = "default";
    const bar = el("section", "list-tools-bar"); bar.setAttribute("aria-label", "一覧の表示設定");
    savedSelect = select("saved-list-view", [["", "保存したビューを選択"]]); savedSelect.addEventListener("change", restoreView);
    sortSelect = select("list-sort", sorts);
    sortSelect.addEventListener("change", () => { if (busy()) { sync(); return; } state.sort = sortSelect.value; resetPages(); sync(); loadList(); });
    viewsMessage = el("p", "list-tools-note", "保存ビューはこのブラウザだけで使えます。共有設定ではありません。"); viewsMessage.setAttribute("role", "status");
    const openSave = mountSaveDialog();
    const savedGroup = el("div", "list-tools-saved"); savedGroup.append(field("このブラウザのビュー", savedSelect),
      button("条件を保存", "button button-secondary", openSave), button("削除", "button button-secondary", () => {
        const name = savedSelect.value; if (!name) { viewsMessage.textContent = "削除する保存ビューを選んでください。"; return; }
        if (writeViews(views.filter((view) => view.name !== name))) { refreshSavedViews(); viewsMessage.textContent = `「${name}」を削除しました。現在の一覧条件は維持しています。`; }
      }));
    const columnControl = el("details", "list-column-control"), caption = el("summary", "", "表示する列");
    const choices = el("fieldset", "list-column-choices"); choices.append(el("legend", "sr-only", "表示する列を選択"));
    columnInputs = columns.map(([key, label]) => {
      const line = el("label"), input = el("input"); input.type = "checkbox"; input.value = key; input.checked = true; input.disabled = key === "name";
      input.addEventListener("change", () => { selectedColumns = columnInputs.filter((control) => control.checked).map((control) => control.value); applyColumns(); });
      line.append(input, document.createTextNode(label + (key === "name" ? "（常に表示）" : ""))); choices.append(line); return input;
    });
    columnControl.append(caption, choices);
    bar.append(savedGroup, field("順序", sortSelect), columnControl, viewsMessage); filters.before(bar);
    const importButton = button("CSVを取り込む", "button button-secondary", mountImportDialog()); importButton.id = "import-csv";
    container.querySelector(".object-actions")?.append(importButton);
    refreshSavedViews(); sync();
  }
  window.MeiboListTools = { sync, applyColumns };
  mount();
})();
