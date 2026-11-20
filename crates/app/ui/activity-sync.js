"use strict";
window.MeiboActivitySync = (() => {
  const labels={google:"Google",microsoft:"Microsoft"};
  const products={google:"Gmail・Google カレンダー",microsoft:"Outlook メール・予定表"};
  const mounted=new WeakSet();
  function mount(root) {
    if (!root || mounted.has(root) || window.MeiboContext?.role==="viewer") return;
    mounted.add(root);
    const dialog=el("dialog","company-dialog"), inner=el("div","crm-dialog-body");
    const title=el("h2","","メール・予定の連携"); title.id="activity-sync-title";
    dialog.setAttribute("aria-labelledby",title.id);
    const details=el("p","subtle","登録済みの担当者が一人だけ一致したメールは活動履歴に、予定は取引先の予定一覧に保存します。組織の営業担当者も件名と日時を確認できます。");
    const cadence=el("p","subtle","接続後はこの画面を開いている間に同期します。閉じていた間の変更は次回の起動後に取り込みます。メールは接続時点以降、予定は接続の30日前から今後180日分が対象です。");
    const cards=el("div"), feedback=el("p","subtle"); feedback.setAttribute("role","status");
    const close=button("閉じる","button button-secondary",()=>dialog.close());
    inner.append(title,details,cadence,cards,feedback,close); dialog.append(inner); document.body.append(dialog);
    const opener=button("メール・予定の連携","button button-secondary",()=>{dialog.showModal();load(true);});
    root.append(opener);
    let providers=[], real=false, loading=false, pending=null;
    const busy=new Set(), timers=new Map();
    const storageKey=window.MeiboContext?.storageKey("activity-sync-auto") || "meibo-activity-sync-auto";
    const setting=value=>{try { if(value===undefined) return localStorage.getItem(storageKey)==="1";
      if(value) localStorage.setItem(storageKey,"1"); else localStorage.removeItem(storageKey); } catch {return false;} };
    const post=async(id,action,body={})=>(await request("/api/activity-sync/"+id+"/"+action,{
      method:"POST",headers:{"Content-Type":"application/json","X-Meibo-Local":"1"},body:JSON.stringify(body)})).json();
    function schedule(id,delay) {
      clearTimeout(timers.get(id));
      timers.set(id,setTimeout(()=>pull(id),delay));
    }
    function render() {
      cards.replaceChildren();
      for(const {id,status} of providers) {
        const section=el("section","crm-summary");
        section.append(el("h3","",products[id]));
        if (!status.configured) {
          section.append(el("p","subtle",real?"管理者がOAuthアプリと専用コネクターを設定すると接続できます。":"実データ用のワークスペースで接続できます。"));
        } else {
          section.append(el("p","",status.connected ? (status.account || labels[id]+" アカウント")+" · "+(status.reauth?"再認可が必要":"接続済み") : "未接続"));
          if(status.connected) section.append(el("p","subtle","最終同期 "+(status.last_sync?formatDate(status.last_sync):"初回の取得中")));
          if(!status.connected||status.reauth) {
            const connect=button(labels[id]+"に接続","button button-primary",()=>authorize(id));
            connect.disabled=Boolean(pending)||busy.has(id); section.append(connect);
            if(pending?.id===id && pending.cancel) section.append(button("接続手続きを中断","button button-secondary",pending.cancel));
          }
          if(status.connected) {
            if(!status.reauth) {
              const sync=button("今すぐ同期","button button-secondary",()=>pull(id));
              sync.disabled=busy.has(id); section.append(sync);
            }
            const stop=button("接続を解除","button button-secondary",()=>disconnect(id));
            stop.disabled=busy.has(id);section.append(stop);
          }
        }
        cards.append(section);
      }
    }
    async function load(start=false) {
      if(loading) return;
      loading=true;
      try {
        const result=await (await request("/api/activity-sync")).json();
        providers=result.providers; real=result.real; render();
        const active=providers.filter(p=>p.status.connected);
        setting(active.length>0);
        if(start) for(const p of active) if(!p.status.reauth&&!busy.has(p.id)) schedule(p.id,100);
      } catch(e){feedback.textContent=e.message;} finally {loading=false;}
    }
    async function pull(id) {
      if(busy.has(id) || !providers.some(p=>p.id===id&&p.status.connected&&!p.status.reauth)) return;
      busy.add(id);render();
      try {
        const result=await post(id,"pull");
        if(result.more) schedule(id,600);
        else { feedback.textContent=labels[id]+" の同期が完了しました。";schedule(id,60000); }
        if(!result.more) await load();
      } catch(e){ feedback.textContent=e.message; await load();schedule(id,90000); }
      finally {busy.delete(id);render();}
    }
    async function disconnect(id) {
      if(busy.has(id)) return;
      clearTimeout(timers.get(id)); busy.add(id);render();
      try { await post(id,"disconnect");feedback.textContent="接続を解除しました。取り込み済みの営業記録は引き続き参照できます。"; await load(); }
      catch(e){feedback.textContent=e.message;} finally {busy.delete(id);render();}
    }
    async function authorize(id) {
      if(pending || busy.has(id)) return;
      const popup=window.open("about:blank","_blank","popup,width=560,height=720");
      if(!popup){feedback.textContent="ポップアップを許可して、もう一度接続してください。";return;}
      pending={id};render();
      let timeout,channel;
      function clear(){clearTimeout(timeout);channel?.close();window.removeEventListener("message",receive);pending=null;render();}
      async function finish(data) {
        if(!pending?.state || data?.type!=="meibo-activity-return" || data.state!==pending.state) return;
        const state=pending.state; pending.state=null;
        if(!data.code){clear();feedback.textContent="接続をキャンセルしました。";return;}
        try { await post(id,"finish",{state,code:data.code});feedback.textContent=labels[id]+" に接続しました。";await load(true); }
        catch(e){feedback.textContent=e.message;} finally {clear();}
      }
      function receive(event){if(event.origin===location.origin && event.source===popup) finish(event.data);}
      try {
        const a=await post(id,"begin"), dest=new URL(a.url);
        const expected=id==="google"?"https://accounts.google.com":"https://login.microsoftonline.com";
        if (dest.origin!==expected || !/^[0-9a-f]{64}$/.test(a.state)) throw new Error("接続の設定を確認してください。");
        pending.state=a.state; window.addEventListener("message",receive);
        pending.cancel=()=>{popup.close();clear();feedback.textContent="接続手続きを中断しました。";};
        render();
        if("BroadcastChannel" in window){channel=new BroadcastChannel("meibo-activity-"+a.state);channel.onmessage=e=>finish(e.data);}
        timeout=setTimeout(()=>{clear();feedback.textContent="接続手続きの期限が切れました。もう一度接続してください。";},600000);
        popup.location.replace(a.url);
      } catch(e){clear();popup.close();feedback.textContent=e.message;}
    }
    if(setting()) load(true);
  }
  return {mount};
})();
