"use strict";

(() => {
  let writes = 0;
  const mounts = new WeakMap();
  const node = (tag, className = "", text = "") => {
    const element = document.createElement(tag);
    element.className = className;
    element.textContent = text;
    return element;
  };
  const action = (text, className = "button button-secondary") => {
    const control = node("button", className, text);
    control.type = "button";
    return control;
  };
  const labelled = (text, control) => {
    const label = node("label", "custom-field-label", text);
    label.htmlFor = control.id;
    const box = node("div", "custom-field-control");
    box.append(label, control);
    return box;
  };
  const kindLabels = { text: "テキスト", number: "数値", boolean: "真偽" };

  function render({ container, companyId, request, notice }) {
    const token = {};
    mounts.set(container, token);
    container.querySelector(":scope > .custom-fields-card")?.remove();
    const section = node("section", "custom-fields-card");
    section.setAttribute("aria-label", "独自項目");
    section.append(node("h2", "", "独自項目"), node("p", "custom-fields-help", "自社で使う補足情報を保存できます。標準の企業詳細やCSVには含まれません。"));
    const message = node("p", "custom-fields-message");
    message.setAttribute("role", "status");
    message.setAttribute("aria-live", "polite");
    const rows = node("div", "custom-fields-rows");
    const retry = action("項目を再読込");
    retry.hidden = true;
    section.append(message, retry, rows);
    container.append(section);
    let pending = false;
    const drafts = new Map();
    const alive = () => mounts.get(container) === token && section.isConnected;
    const feedback = (text, error = false) => {
      if (!alive()) return;
      message.textContent = text;
      message.classList.toggle("has-error", error);
      notice?.(text, error);
    };
    const updateBusy = () => {
      section.querySelectorAll("input,textarea,select,button").forEach((control) => { control.disabled = pending; });
      section.setAttribute("aria-busy", String(pending));
    };
    const post = async (url, body) => {
      const response = await request(url, { method: "POST", headers: { "Content-Type": "application/json", "X-Meibo-Local": "1" }, body: JSON.stringify(body) });
      return response.json();
    };

    async function mutate(url, body, success, committed) {
      if (pending) return;
      pending = true; writes += 1; updateBusy();
      try {
        await post(url, body);
        committed();
        const refreshed = await load(false);
        feedback(refreshed ? success : `${success} 表示を更新できなかったため、再読込してください。`, !refreshed);
      } catch (error) {
        feedback(error.message || "保存できませんでした。入力を保持しています。", true);
      } finally {
        pending = false; writes -= 1;
        if (alive()) updateBusy();
      }
    }

    function valueForm(field, saved) {
      const form = node("form", "custom-field-value-form");
      const group = node("fieldset", "custom-field-value-group");
      group.dataset.fieldId = String(field.id);
      group.append(node("legend", "", field.name));
      const metadata = `${kindLabels[field.kind]}・${field.indexed ? "一致検索の索引あり" : "一致検索の索引なし"}`;
      group.append(node("p", "custom-fields-help", metadata));
      const control = node(field.kind === "text" ? "textarea" : field.kind === "boolean" ? "select" : "input");
      control.id = `custom-value-${companyId}-${field.id}`;
      if (field.kind === "text") { control.rows = 2; control.maxLength = 8000; }
      else if (field.kind === "number") { control.type = "number"; control.step = "any"; control.required = true; }
      else {
        control.add(new Option("未設定", ""));
        control.add(new Option("はい", "true"));
        control.add(new Option("いいえ", "false"));
        control.required = true;
      }
      control.value = drafts.has(field.id) ? drafts.get(field.id) : saved === undefined ? "" : String(saved);
      control.addEventListener("input", () => { control.setCustomValidity(""); drafts.set(field.id, control.value); });
      group.append(labelled(`${field.name}の値`, control));
      const buttons = node("div", "custom-fields-actions");
      const save = action("保存", "button button-primary"); save.type = "submit";
      const clear = action("値を解除");
      clear.addEventListener("click", () => mutate(`/api/companies/${companyId}/custom-fields`, { field_id: field.id, value: null }, `「${field.name}」の値を解除しました。`, () => drafts.delete(field.id)));
      const remove = action("項目を削除", "button button-secondary custom-field-delete");
      remove.addEventListener("click", () => {
        if (pending || !window.confirm(`「${field.name}」を削除すると、全取引先のこの項目の値も削除されます。削除しますか？`)) return;
        mutate(`/api/custom-fields/${field.id}/delete`, {}, `「${field.name}」を削除しました。`, () => drafts.delete(field.id));
      });
      buttons.append(save, clear, remove); group.append(buttons); form.append(group);
      form.addEventListener("submit", (event) => {
        event.preventDefault();
        let value = control.value;
        if (field.kind === "number") {
          value = control.valueAsNumber;
          if (!Number.isFinite(value)) { control.setCustomValidity("有限の数値を入力してください。"); control.reportValidity(); return; }
        } else if (field.kind === "boolean") value = control.value === "true";
        else if ([...value].length > 4000 || value.includes("\0")) { control.setCustomValidity("4000文字以内で入力してください。"); control.reportValidity(); return; }
        mutate(`/api/companies/${companyId}/custom-fields`, { field_id: field.id, value }, `「${field.name}」を保存しました。`, () => drafts.delete(field.id));
      });
      return form;
    }

    const createForm = node("form", "custom-field-create-form");
    const createGroup = node("fieldset", "custom-field-create-group");
    createGroup.append(node("legend", "", "項目を追加"));
    const name = node("input"); name.id = `custom-field-name-${companyId}`;
    name.required = true; name.maxLength = 200; name.autocomplete = "off";
    name.addEventListener("input", () => name.setCustomValidity(""));
    const kind = node("select"); kind.id = `custom-field-kind-${companyId}`;
    Object.entries(kindLabels).forEach(([value, label]) => kind.add(new Option(label, value)));
    const indexed = node("input"); indexed.type = "checkbox"; indexed.checked = true;
    indexed.id = `custom-field-indexed-${companyId}`;
    const indexedLabel = node("label", "custom-fields-check"); indexedLabel.htmlFor = indexed.id;
    indexedLabel.append(indexed, document.createTextNode("この項目での一致検索を速くする"));
    const create = action("項目を追加", "button button-primary"); create.type = "submit";
    createGroup.append(labelled("項目名", name), labelled("種類", kind), indexedLabel, create);
    createForm.append(createGroup); section.append(createForm);
    createForm.addEventListener("submit", (event) => {
      event.preventDefault();
      const value = name.value.trim();
      if (!value || [...value].length > 100 || /[\u0000-\u001f\u007f]/.test(value)) { name.setCustomValidity("項目名は1〜100文字で入力してください。"); name.reportValidity(); return; }
      mutate("/api/custom-fields", { name: value, kind: kind.value, indexed: indexed.checked }, `「${value}」を追加しました。`, () => { name.value = ""; });
    });

    async function load(showLoading = true) {
      if (showLoading && alive()) message.textContent = "独自項目を読み込んでいます…";
      try {
        const data = await (await request(`/api/companies/${companyId}/custom-fields`)).json();
        if (!alive()) return false;
        const values = new Map(data.values.map((value) => [value.field_id, value.value]));
        rows.replaceChildren(...data.fields.map((field) => valueForm(field, values.get(field.id))));
        if (!data.fields.length) rows.append(node("p", "custom-fields-empty", "独自項目はまだありません。下のフォームから追加できます。"));
        retry.hidden = true;
        if (showLoading) { message.textContent = ""; message.classList.remove("has-error"); }
        updateBusy();
        return true;
      } catch (error) {
        if (alive()) { retry.hidden = false; if (showLoading) feedback(error.message || "項目を読み込めませんでした。再試行してください。", true); }
        return false;
      }
    }
    retry.addEventListener("click", () => { if (!pending) load(); });
    load();
    return section;
  }

  window.MeiboFields = { render, busy: () => writes > 0 };
})();
