"use strict";

window.MeiboFollowups = (() => {
  let generation = 0;
  let current = "";
  const tokyo = new Intl.DateTimeFormat("ja-JP", {timeZone:"Asia/Tokyo", year:"numeric", month:"numeric", day:"numeric", hour:"2-digit", minute:"2-digit", hourCycle:"h23"});

  function refreshAfterSave() { current = ""; void refresh(); }
  async function refresh() {
    const panel = document.getElementById("followup-items");
    if (!panel || document.hidden) return;
    const version = ++generation;
    try {
      const data = await (await request("/api/followups")).json();
      if (version !== generation) return;
      const signature = JSON.stringify(data);
      if (signature === current) return;
      current = signature;
      const badge = document.getElementById("followup-count");
      badge.hidden = !data.total;
      badge.textContent = data.total ? String(data.total) : "";
      badge.setAttribute("aria-label", `本日までの再架電 ${data.total}件`);
      const summary = document.getElementById("followup-summary");
      summary.textContent = data.total > data.items.length ? `${data.total}件のうち、期限の早い${data.items.length}件` : `${data.total}件`;
      panel.replaceChildren();
      if (!data.total) panel.append(el("p", "empty-message", "今日までに再架電する取引先はありません。"));
      for (const item of data.items) {
        const row = el("div", "home-list-item");
        const detail = el("div");
        detail.append(button(item.company_name || "個人情報は非表示", "home-item-name", () => { if (!busy()) openCompany(item.company_id); }));
        detail.append(el("p", "home-item-meta", tokyo.format(item.due_at * 1000) + "（日本時間）"));
        row.append(detail);
        if (window.MeiboContext?.role !== "viewer") {
          const done = button("完了", "button button-secondary", async () => {
            done.disabled = true;
            try {
              await request(`/api/work-items/${item.work_id}/completion`, {method:"POST",headers:{"Content-Type":"application/json","X-Meibo-Local":"1"},body:JSON.stringify({completed:true})});
              notify("再架電を完了にしました。"); refreshAfterSave();
            } catch (error) { notify(error.message,true); done.disabled=false; }
          });
          done.setAttribute("aria-label", (item.company_name || "取引先") + "の再架電を完了"); row.append(done);
        }
        panel.append(row);
      }
    } catch (error) {
      if (version !== generation) return;
      current = "";
      const summary=document.getElementById("followup-summary"); summary.textContent = "";
      const badge=document.getElementById("followup-count"); badge.hidden = true;
      panel.replaceChildren(el("p","empty-message",error.message),button("再読み込み","button button-secondary",refresh));
    }
  }
  function start() {
    if (window.MeiboContext?.hosted) document.getElementById("followup-title").textContent = "自分の再架電";
    void refresh();
    window.setInterval(refresh, 60_000);
    document.addEventListener("visibilitychange", () => { if (!document.hidden) void refresh(); });
  }
  return {start, refresh, refreshAfterSave};
})();
