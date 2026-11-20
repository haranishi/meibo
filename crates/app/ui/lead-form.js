"use strict";
(() => {
  const form = document.getElementById("inquiry");
  const result = document.getElementById("result");
  const submit = form.querySelector('button[type="submit"]');
  const newInquiry = document.getElementById("new-inquiry");
  const keyName = "inquiry-submission:" + location.pathname;
  document.getElementById("recipient").textContent = "受付先　" + location.pathname.split("/")[2];
  let key = null;
  let lastPayload = null;
  let sending = false;

  function offerRetry(message) {
    result.textContent = message;
    submit.textContent = "前回と同じ内容を再送";
    newInquiry.hidden = false;
  }

  function clearAttempt() {
    key = null;
    lastPayload = null;
    try { sessionStorage.removeItem(keyName); } catch (_) {}
    submit.textContent = "問い合わせを送信";
    newInquiry.hidden = true;
  }

  function submissionKey() {
    if (!key) {
      const bytes = crypto.getRandomValues(new Uint8Array(16));
      key = Array.from(bytes, byte => byte.toString(16).padStart(2, "0")).join("");
      try { sessionStorage.setItem(keyName, key); } catch (_) {}
    }
    return key;
  }

  try { key = sessionStorage.getItem(keyName); } catch (_) {}
  if (/^[a-f0-9]{32}$/.test(key || "")) {
    offerRetry("前回の送信結果を確認できていません。同じ内容を再送するか、別の問い合わせとして送信してください。");
  } else { clearAttempt(); }
  submit.disabled = false;

  newInquiry.addEventListener("click", () => {
    if (sending || !form.reportValidity()) return;
    clearAttempt();
    form.requestSubmit(submit);
  });

  form.addEventListener("submit", async event => {
    event.preventDefault();
    if (sending || !form.reportValidity()) return;
    const data = new FormData(form);
    const payload = {
      company_name: data.get("company_name").trim(), contact_name: data.get("contact_name").trim(),
      email: data.get("email").trim(), message: data.get("message").trim(), consent: data.has("consent"),
    };
    const serialized = JSON.stringify(payload);
    if (lastPayload && lastPayload !== serialized) {
      offerRetry("前回から入力内容が変わりました。変更した内容を届けるには「別の問い合わせとして送信」を選んでください。");
      return;
    }
    lastPayload = serialized;
    sending = true;
    form.inert = true;
    result.textContent = "送信しています…";
    try {
      const response = await fetch(location.pathname, {
        method: "POST", credentials: "omit", cache: "no-store",
        headers: { "Content-Type": "application/json", "X-Meibo-Local": "1" },
        body: JSON.stringify({...payload, submission_key:submissionKey()}),
      });
      if (response.status === 409) {
        offerRetry("先に送信した内容を受け付けています。変更した内容は別の問い合わせとして送信してください。");
        return;
      }
      if (response.status >= 500) throw new Error("受付結果を確認できません。");
      if (response.status !== 202) {
        clearAttempt();
        result.textContent = response.status === 404 ? "この受付フォームは停止しています。" :
          response.status === 429 ? "現在は受け付けられません。時間を置いてお試しください。" :
          "送信できませんでした。入力内容を確認して、もう一度送信してください。";
        return;
      }
      const receipt = await response.json();
      if (receipt.accepted !== true) throw new Error("受付結果を確認できません。");
      clearAttempt();
      form.reset(); form.hidden = true;
      result.textContent = "お問い合わせを受け付けました。";
      result.focus();
    } catch (_) {
      offerRetry("送信結果を確認できませんでした。通信状況を確認して、同じ内容を再送してください。");
    } finally {
      sending = false;
      form.inert = false;
    }
  });
})();
