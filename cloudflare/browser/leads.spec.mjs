import { expect, test } from '@playwright/test';

test('a visitor submits a consented inquiry and a salesperson converts it once', async ({page, browser, baseURL}, testInfo) => {
  const sample = await (await page.request.get('/api/companies?limit=100')).json();
  const company = sample.rows.findLast(row => row.name && row.status !== 5);
  const contact = '合成受付担当 ' + testInfo.project.name;
  const email = testInfo.project.name + '@lead.example.invalid';
  const headers = {Origin:baseURL,'X-Meibo-Local':'1'};
  expect((await page.request.post('/api/lead-form',{headers,data:{enabled:false}})).ok()).toBeTruthy();
  await page.goto('/');
  await page.locator('#app-launcher').click();
  await page.locator('#app-menu [data-workspace="home"]').click();
  await page.getByRole('button', {name:'問い合わせ受付',exact:true}).click();
  const dialog=page.getByRole('dialog');
  await dialog.getByRole('button',{name:'公開フォームの設定'}).click();
  await dialog.getByRole('button',{name:'受付を開始'}).click();
  const url=await dialog.getByLabel('受付URL').inputValue();
  const visitor=await browser.newContext({baseURL,viewport:page.viewportSize()});
  try {
    const form=await visitor.newPage();
    await form.goto(url);
    await form.getByLabel('会社・団体名').fill(company.name);
    await form.getByLabel('お名前').fill(contact);
    await form.getByLabel('返信先メールアドレス').fill(email);
    await form.getByRole('button',{name:'問い合わせを送信'}).click();
    await expect(form.getByRole('checkbox')).not.toBeChecked();
    await expect(form.getByRole('button',{name:'問い合わせを送信'})).toBeVisible();
    await form.getByRole('checkbox').check();
    await form.screenshot({path:testInfo.outputPath('web-inquiry-form.png'),fullPage:true});
    const received=form.waitForResponse(res=>res.url()===url && res.request().method()==='POST');
    await form.getByRole('button',{name:'問い合わせを送信'}).click();
    const acceptance=await received;
    expect(acceptance.status()).toBe(202);
    expect(await acceptance.json()).toEqual({accepted:true});
    await expect(form.getByRole('status')).toHaveText('お問い合わせを受け付けました。');
    await dialog.getByRole('button',{name:'受付一覧に戻る'}).click();
    await expect(dialog).not.toContainText(email);
    await dialog.getByRole('button',{name:/^受付 [0-9]+$/}).first().click();
    await dialog.getByRole('button',{name:'問い合わせ内容を表示'}).click();
    await expect(dialog).toContainText(contact);
    const owner = 30000 + page.viewportSize().width;
    await dialog.getByLabel('営業担当番号').fill(String(owner));
    const assignment = page.waitForResponse(res=>res.url().endsWith('/assign'));
    await dialog.getByRole('button',{name:'担当を保存'}).click();
    expect((await (await assignment).json()).owner_id).toBe(owner);
    await expect(dialog.getByLabel('営業担当番号')).toHaveValue(String(owner));
    await dialog.screenshot({path:testInfo.outputPath('lead-inbox-detail.png')});
    await dialog.getByRole('button',{name:'取引先への変換'}).click();
    const choices=dialog.getByLabel('登録先の取引先');
    await expect(choices.locator('option[value="'+company.id+'"]')).toHaveCount(1);
    await choices.selectOption(String(company.id));
    await dialog.getByLabel('商談も作成').check();
    await dialog.getByLabel('商談名').fill('受付からの合成商談');
    await dialog.getByLabel('金額（円）').fill('100');
    await dialog.getByLabel('完了予定日').fill('2180-01-01');
    const conversion=page.waitForResponse(res=>res.url().endsWith('/convert') && res.request().method()==='POST');
    await dialog.getByRole('button',{name:'取引先・担当者に変換'}).click();
    const converted=await conversion;
    expect(converted.status()).toBe(200);
    const receipt=await converted.json();
    expect(receipt.company_id).toBe(company.id);
    expect(receipt.owner_id).toBe(owner);
    expect(receipt.opportunity_id).toBeGreaterThan(0);
    expect(receipt.email).toBeUndefined();
    await expect(dialog.getByText('取引先に引き継ぎました。')).toBeVisible();
    await dialog.getByRole('button',{name:'受付一覧に戻る'}).click();
    await expect(dialog.getByText('該当する問い合わせはありません。')).toBeVisible();
    await dialog.getByRole('button',{name:'公開フォームの設定'}).click();
    await dialog.getByRole('button',{name:'受付を停止'}).click();
    await expect(dialog.getByText('現在、フォームの受付は停止中です。')).toBeVisible();
    expect((await form.goto(url)).status()).toBe(404);
  } finally {
    await page.request.post('/api/lead-form',{headers,data:{enabled:false}});
    await visitor.close();
  }
});

test('an edited retry can be sent separately after a lost acknowledgement', async ({page,baseURL},testInfo)=>{
  const headers = {Origin:baseURL,'X-Meibo-Local':'1'};
  const admin = page.request;
  const active = await admin.post('/api/lead-form',{headers,data:{enabled:true}});
  const {public_key:key} = await active.json();
  const path = '/forms/local/'+key;
  const tenantUrl = baseURL+path;
  const company = 'A'.repeat(180);
  const text = 'https://example.invalid/'+'x'.repeat(500)+'\n別の行';
  const original = 'first-'+testInfo.project.name+'@lead.example.invalid';
  const changed = 'updated-'+testInfo.project.name+'@lead.example.invalid';
  const own = [];
  try {
    await page.goto(path);
    await page.getByLabel('会社・団体名').fill(company);
    await page.getByLabel('お名前').fill('受付確認用');
    await page.getByLabel('返信先メールアドレス').fill(original);
    await page.getByLabel('お問い合わせ内容').fill(text);
    await page.getByRole('checkbox').check();
    await page.route(tenantUrl,async route=>{
      const accepted=await route.fetch();
      expect(accepted.status()).toBe(202);
      await route.abort('failed');
    },{times:1});
    await page.getByRole('button',{name:'問い合わせを送信',exact:true}).click();
    await expect(page.getByRole('status')).toContainText('送信結果を確認できませんでした');
    await page.getByLabel('返信先メールアドレス').fill(changed);
    await page.getByRole('button',{name:'前回と同じ内容を再送'}).click();
    await expect(page.getByRole('status')).toContainText('前回から入力内容が変わりました');
    await page.getByRole('button',{name:'別の問い合わせとして送信'}).click();
    await expect(page.getByRole('status')).toHaveText('お問い合わせを受け付けました。');
    const inbox = await (await admin.get('/api/leads')).json();
    for (const row of inbox.items) {
      const detail=await (await admin.get('/api/leads/'+row.id+'?reveal=1')).json();
      if ([original,changed].includes(detail.email)) own.push(detail);
    }
    expect(own.map(r=>r.email).sort()).toEqual([original,changed].sort());
    await page.goto('/');
    await page.locator('#app-launcher').click();
    await page.locator('#app-menu [data-workspace="home"]').click();
    await page.getByRole('button',{name:'問い合わせ受付',exact:true}).click();
    const dialog=page.getByRole('dialog');
    await dialog.getByRole('button',{name:'受付 '+own[0].id,exact:true}).click();
    await dialog.getByRole('button',{name:'問い合わせ内容を表示'}).click();
    await expect(dialog).toContainText(text);
    expect(await dialog.evaluate(d=>d.scrollWidth<=d.clientWidth)).toBeTruthy();
    await dialog.getByRole('button',{name:'取引先への変換'}).click();
    await dialog.getByLabel('変換先').selectOption('new');
    expect(await dialog.evaluate(d=>d.scrollWidth<=d.clientWidth)).toBeTruthy();
  } finally {
    for (const row of own) await admin.post('/api/leads/'+row.id+'/discard',{headers,data:{revision:row.revision}});
    await admin.post('/api/lead-form',{headers,data:{enabled:false}});
  }
});
