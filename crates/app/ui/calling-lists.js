"use strict";

window.MeiboCallingLists = (() => {
  function describe(conditions) {
    const choice = (id, value) => Array.from($(id).options).find(option => option.value === value)?.textContent;
    return [
      choice("list-view", conditions.view || "all"),
      conditions.q && `社名「${conditions.q}」`,
      conditions.prefecture && prefectures[Number(conditions.prefecture)],
      conditions.status && statuses[Number(conditions.status)],
      conditions.has_phone && (conditions.has_phone === "true" ? "電話番号あり" : "電話番号なし"),
      choice("list-sort", conditions.sort || "default"),
    ].filter(Boolean).join("・");
  }

  function mount({ dialog, field, select }) {
    const item = dialog("共有架電リスト", "calling-lists-dialog");
    const canEdit = window.MeiboContext?.role !== "viewer";
    const scope = window.MeiboContext ? "この組織" : "このデータベース";
    const description = el("p", "dialog-description", `${scope}に保存した条件から、現在の架電先を検索します。`);
    const fields = el("fieldset", "dialog-fields");
    const picker = select("shared-calling-list", []);
    const selectedConditions = el("p", "calling-description");
    const savedField = field("保存済みの架電リスト", picker);
    savedField.classList.add("full-width"); savedField.append(selectedConditions);
    const current = el("p", "calling-description"), name = el("input");
    name.id = "calling-list-name"; name.maxLength = 64; name.autocomplete = "off";
    const editField = field("リスト名", name);
    editField.classList.add("full-width"); editField.append(current);
    const status = el("p", "dialog-description"); status.setAttribute("role", "status");
    const read = button("選んだリストを開く", "button button-primary", () => {
      const list = selected(); if (!list || busy()) return;
      const { view = "all", sort = "default", q = "", ...filters } = list.conditions;
      active = list;
      item.box.close(); openFilteredList(view, filters, q, sort);
      notify(`「${list.name}」の架電先を表示します。`);
    });
    const create = button("現在の条件で新規保存", "button button-secondary", () => write("create"));
    const update = button("現在の条件で更新", "button button-secondary", () => write("update"));
    const remove = button("リストを削除", "button button-secondary", () => write("delete"));
    const writes = el("div", "filter-actions full-width");
    writes.append(create, update, remove);
    const footer = el("footer", "dialog-footer");
    footer.append(button("閉じる", "button button-secondary", () => item.box.close()), read);
    fields.append(savedField);
    if (canEdit) fields.append(editField, writes);
    item.form.append(description, fields, status, footer);

    let lists = [], active = null, pending = false, loaded = false;
    const selected = () => lists.find(list => String(list.id) === picker.value);
    function sync() {
      const list = selected();
      const stale = active && list?.id === active.id && list.revision !== active.revision;
      picker.disabled = !loaded || pending || !lists.length;
      read.disabled = !list || pending;
      create.disabled = !loaded || pending; update.disabled = !list || pending || stale; remove.disabled = update.disabled;
      name.disabled = pending;
      selectedConditions.textContent = list ? describe(list.conditions) : "";
      current.textContent = `保存する条件　${describe(listConditions())}`;
      if (stale) status.textContent = "ほかの担当者が更新しました。リストを開き直すと最新の条件を確認できます。";
    }
    picker.addEventListener("change", () => {
      status.textContent = ""; name.setCustomValidity(""); name.value = selected()?.name || ""; sync();
    });
    name.addEventListener("input", () => name.setCustomValidity(""));
    item.form.addEventListener("submit", event => event.preventDefault());
    async function load() {
      pending = true; loaded = false; lists = [];
      picker.replaceChildren(new Option("リストを選択", "")); name.value = ""; name.setCustomValidity("");
      sync(); status.textContent = "架電リストを読み込んでいます…";
      try {
        lists = (await (await request("/api/calling-lists")).json()).lists;
        lists.forEach(list => picker.add(new Option(list.name, String(list.id))));
        if (active && lists.some(list => list.id === active.id)) picker.value = String(active.id);
        name.value = selected()?.name || "";
        loaded = true;
        status.textContent = lists.length ? "" : "保存済みの架電リストはまだありません。";
      } catch (error) { status.textContent = error.message; }
      finally { pending = false; sync(); }
    }
    async function write(action) {
      if (pending || !loaded || !canEdit) return;
      const list = selected(), editing = action !== "create";
      if (editing && (!list || update.disabled)) return;
      if (action !== "delete" && !name.value.trim()) {
        name.setCustomValidity("リスト名を入力してください。"); name.reportValidity(); return;
      }
      if (action === "delete" && !window.confirm(`「${list.name}」を${scope}の架電リストから削除しますか？`)) return;
      const body = editing ? { revision: list.revision } : {};
      if (action !== "delete") Object.assign(body, { name: name.value.trim(), conditions: listConditions() });
      let url = "/api/calling-lists";
      if (editing) url += `/${list.id}`;
      if (action === "delete") url += "/delete";
      pending = true; sync();
      try {
        const saved = await (await request(url, { method: "POST", headers: { "Content-Type": "application/json", "X-Meibo-Local": "1" }, body: JSON.stringify(body) })).json();
        active = action === "delete" ? null : saved;
        item.box.close();
        notify(action === "delete" ? `「${list.name}」を削除しました。` : `「${saved.name}」を${scope}に保存しました。`);
      } catch (error) {
        if (error.status === 409) loaded = false;
        status.textContent = error.message;
      } finally { pending = false; sync(); if (!loaded) { update.disabled = true; remove.disabled = true; read.disabled = true; } }
    }
    const opener = button("共有架電リスト", "button button-secondary", () => {
      if (busy() || pending) return;
      item.box.showModal(); load();
    });
    opener.id = "open-calling-lists";
    return opener;
  }
  return { mount };
})();
