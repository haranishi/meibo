"use strict";

window.MeiboLeads = (() => {
  const secondary = "button button-secondary";
  const primary = "button button-primary";
  const read = async url => (await request(url)).json();
  const post = async (url, data) => (await request(url, {method:"POST", headers:{"Content-Type":"application/json","X-Meibo-Local":"1"}, body:JSON.stringify(data)})).json();

  function field(title, control) {
    const label = el("label", "form-field");
    label.append(el("span", "", title), control);
    return label;
  }
  function option(select, text, value) {
    const item = el("option", "", text); item.value = String(value); select.append(item);
  }
  function input(type, required = false) {
    const control = el("input"); control.type = type; control.required = required; return control;
  }

  function mount(container) {
    if (window.MeiboContext?.role === "viewer" || !container || container.querySelector(".lead-inbox-open")) return;
    container.append(button("問い合わせ受付", secondary + " lead-inbox-open", open));
  }

  function open() {
    if (busy() || document.getElementById("lead-inbox-dialog")) return;
    const dialog = el("dialog", "company-dialog");
    dialog.id = "lead-inbox-dialog"; dialog.setAttribute("aria-labelledby", "lead-heading");
    const header = el("header", "dialog-header");
    const heading = el("h2", "", "問い合わせ受付"); heading.id = "lead-heading";
    const close = button("閉じる", secondary, () => dialog.close());
    header.append(heading,close);
    const panel = el("section", "dialog-fields");
    const message = el("p", "dialog-description"); message.setAttribute("role", "status");
    dialog.append(header,message,panel); document.body.append(dialog);
    let generation = 0;
    let pending = false;
    let members = [];
    let hosted = false;
    let statusFilter = "open";
    const live = id => id === generation && dialog.open;
    function clear() { generation++; panel.replaceChildren(); message.textContent = ""; return generation; }
    function report(e) { if (dialog.open) message.textContent = e.message; }
    async function mutate(action, done) {
      if (pending) return;
      pending = true; close.disabled = true; panel.inert = true;
      try { const result = await action(); if (dialog.open) await done(result); }
      catch (e) { report(e); }
      finally { pending = false; close.disabled = false; panel.inert = false; }
    }
    dialog.addEventListener("cancel", event => { if (pending) event.preventDefault(); });
    dialog.addEventListener("close", () => { generation++; dialog.remove(); });
    dialog.showModal();

    function listing(after = null, history = []) {
      const current = clear();
      const select = el("select");
      [["未処理","open"],["変換済み","converted"],["破棄済み","discarded"]].forEach(([t,v]) => option(select,t,v));
      select.value = statusFilter;
      select.addEventListener("change", () => { statusFilter=select.value; listing(); });
      panel.append(field("表示する受付", select));
      if (!window.MeiboContext || window.MeiboContext.role === "admin") {
        panel.append(button("公開フォームの設定", secondary, settings));
      }
      const rows = el("div", "form-field full-width");
      panel.append(rows); message.textContent = "受付を読み込んでいます…";
      read("/api/leads?state=" + statusFilter + (after ? "&after=" + after : "")).then(data => {
        if (!live(current)) return;
        message.textContent = data.items.length ? "" : "該当する問い合わせはありません。";
        for (const lead of data.items) {
          const row = el("div","crm-related-row");
          const owner = members.find(m => m.id === lead.owner_id)?.name || (lead.owner_id ? "担当 " + lead.owner_id : "未割当");
          row.append(button("受付 " + lead.id, "text-button", () => detail(lead.id)),
            el("span","",formatDate(lead.created_at) + "　" + owner));
          rows.append(row);
        }
        if (history.length) panel.append(button("前のページ", secondary, () => listing(history.at(-1), history.slice(0,-1))));
        if (data.next) panel.append(button("次のページ", secondary, () => listing(data.next,[...history, after])));
      }).catch(e => { if (live(current)) report(e); });
    }

    async function settings() {
      const current = clear();
      panel.append(button("受付一覧に戻る", secondary, () => listing()));
      try {
        const {public_key:key} = await read("/api/lead-form");
        if (!live(current)) return;
        if (key) {
          const url = new URL("/forms/" + (window.MeiboContext?.tenant || "local") + "/" + key, location.origin);
          const endpoint = input("url"); endpoint.value = url.href; endpoint.readOnly = true;
          const link = el("a","text-button","フォームを確認"); link.href = url.href; link.target = "_blank"; link.rel = "noopener noreferrer";
          panel.append(field("受付URL", endpoint), link);
          if (!window.MeiboContext?.hosted) message.textContent = "このURLは、この端末でのみ開けます。";
        } else { message.textContent = "現在、フォームの受付は停止中です。"; }
        panel.append(button(key ? "受付を停止" : "受付を開始", key ? secondary : primary, () =>
          mutate(() => post("/api/lead-form", {enabled:!key}), settings)));
      } catch (e) { if (live(current)) report(e); }
    }

    async function detail(id, reveal = false) {
      const current = clear();
      panel.append(button("受付一覧に戻る", secondary, () => listing()));
      try {
        const lead = await read("/api/leads/" + id + (reveal ? "?reveal=1" : ""));
        if (!live(current)) return;
        panel.append(el("h3", "card-heading", "受付 " + lead.id));
        if (lead.state !== 0) {
          message.textContent = lead.state === 1 ? "取引先に引き継ぎました。" : "受付内容を破棄しました。";
          if (lead.company_id) panel.append(button("取引先を開く", secondary, () => {dialog.close(); openCompany(lead.company_id);}));
          return;
        }
        if (!reveal) {
          panel.append(button("問い合わせ内容を表示", secondary, () => detail(id,true)));
        } else {
          const personal = el("dl","form-field full-width");
          for (const [k,v] of [["会社・団体名",lead.company_name],["お名前",lead.contact_name],["返信先",lead.email],["お問い合わせ内容",lead.message || "記入なし"]]) {
            personal.append(el("dt","",k),el("dd","",v));
          }
          panel.append(personal,button("問い合わせ内容を隠す", secondary, () => detail(id,false)));
        }
        const owner = hosted ? el("select") : input("number");
        if (hosted) {
          option(owner,"未割当","");
          members.forEach(m => option(owner,m.name,m.id));
          if (lead.owner_id && !members.some(m => m.id === lead.owner_id)) {
            option(owner,"担当 " + lead.owner_id + "（現在は選択不可）",lead.owner_id);
            owner.lastChild.disabled = true;
          }
        } else {
          owner.min = "1"; owner.step = "1"; owner.max = String(Number.MAX_SAFE_INTEGER);
          owner.placeholder = "未割当";
          const choices = el("datalist"); choices.id = "lead-owner-choices";
          members.forEach(m => option(choices,m.name,m.id));
          owner.setAttribute("list",choices.id); panel.append(choices);
        }
        owner.value = lead.owner_id || "";
        panel.append(field(hosted ? "営業担当" : "営業担当番号",owner),button("担当を保存",secondary,() => {
          if (owner.reportValidity()) mutate(() => post("/api/leads/" + id + "/assign",{revision:lead.revision,owner_id:owner.value ? Number(owner.value) : null}),() => detail(id,reveal));
        }));
        if (reveal) panel.append(button("取引先への変換",primary,() => conversion(lead)));
        const discard = button("受付を破棄",secondary,() => {
          discard.disabled=true;
          panel.append(el("p","full-width","氏名・メール・問い合わせ内容を消去します。"),
            button("消去して破棄",secondary,()=>mutate(()=>post("/api/leads/"+id+"/discard",{revision:lead.revision}),()=>detail(id))),
            button("取り消す",secondary,()=>detail(id,reveal)));
        });
        panel.append(discard);
      } catch (e) { if (live(current)) report(e); }
    }

    function conversion(lead) {
      const current = clear();
      panel.append(button("受付に戻る",secondary,()=>detail(lead.id,true)));
      const form = el("form","form-field full-width");
      const mode = el("select");
      option(mode,"既存の取引先","existing"); option(mode,"新しい取引先","new");
      const search = input("search"); search.value = lead.company_name; search.maxLength = 200;
      const choices = el("select");
      const existing = el("fieldset", "dialog-fields");
      let searchVersion = 0;
      async function lookup() {
        const version = ++searchVersion;
        choices.replaceChildren(); option(choices,"取引先を選択","");
        const term=search.value.trim();
        if (!term) return;
        try {
          const data=await read("/api/companies?q="+encodeURIComponent(term)+"&limit=20");
          if (!live(current) || version !== searchVersion) return;
          for (const company of data.rows) option(choices,companyName(company)+" ("+(prefectures[company.prefecture] || "未設定")+"・ID "+company.id+")",company.id);
          if (!data.rows.length) message.textContent="候補がありません。会社名を変えるか、新しい取引先を選んでください。";
        } catch (e) { if (live(current) && version === searchVersion) report(e); }
      }
      search.addEventListener("input",()=>{searchVersion++;choices.replaceChildren();option(choices,"検索してください","");});
      existing.append(field("検索する会社名",search),button("取引先を検索",secondary,lookup),field("登録先の取引先",choices));
      const fresh=el("fieldset","dialog-fields");
      const prefecture=el("select"); option(prefecture,"選択してください","");
      prefectures.forEach((value,i)=>{if(i) option(prefecture,value,i);});
      const kind=el("select"); option(kind,"法人・団体","organization");option(kind,"個人事業","sole");
      fresh.append(el("p","full-width",lead.company_name),field("都道府県",prefecture),field("取引先の種類",kind));
      function modeChanged() {
        const isNew=mode.value==="new"; existing.hidden=isNew; existing.disabled=isNew; choices.required=!isNew;
        fresh.hidden=!isNew;fresh.disabled=!isNew;prefecture.required=isNew;message.textContent="";
      }
      mode.addEventListener("change",modeChanged);
      const deal = input("checkbox");
      const dealFields=el("fieldset","dialog-fields");
      const name=input("text",true); name.maxLength=200;
      const amount=input("number",true); amount.min="0"; amount.step="1"; amount.max=String(Number.MAX_SAFE_INTEGER); amount.value="0";
      const date=input("date",true);
      dealFields.append(field("商談名",name),field("金額（円）",amount),field("完了予定日",date));
      function dealChanged(){dealFields.disabled=!deal.checked;dealFields.hidden=!deal.checked;}
      deal.addEventListener("change",dealChanged);
      const dealChoice = field("商談も作成",deal);
      dealChoice.classList.add("lead-checkbox"); dealChoice.prepend(deal);
      form.append(field("変換先",mode),existing,fresh,dealChoice,dealFields);
      const submit=el("button",primary,"取引先・担当者に変換");submit.type="submit";form.append(submit);panel.append(form);
      form.addEventListener("submit", event=>{
        event.preventDefault();
        if (!form.reportValidity()) return;
        const data={revision:lead.revision,owner_id:lead.owner_id,company_id:mode.value==="existing"?Number(choices.value):null,
          prefecture:mode.value==="new"?Number(prefecture.value):null,sole_proprietor:mode.value==="new" && kind.value==="sole",
          opportunity:deal.checked?{name:name.value.trim(),amount_jpy:Number(amount.value),close_date:date.value}:null};
        mutate(()=>post("/api/leads/"+lead.id+"/convert",data),()=>detail(lead.id));
      });
      modeChanged();dealChanged();lookup();
    }
    read("/api/sales-members").then(data=>{if(dialog.open){members=data.members;hosted=data.hosted;listing();}}).catch(report);
  }
  return { mount };
})();
