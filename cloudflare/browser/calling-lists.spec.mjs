import { test, expect } from '@playwright/test';

test('saved calling conditions open in another browser and drive calls across pages', async ({ page, browser, baseURL }, testInfo) => {
  const label = `合成架電リスト ${testInfo.project.name}`;
  const rows = page.locator('#company-rows tr');
  await page.addInitScript(() => {
    localStorage.setItem('meibo-list-views-v1', JSON.stringify({version: 1, views: [{
      name: '以前の個人ビュー', query: '', view: 'untouched', filters: {}, sort: 'default', columns: ['name', 'status'],
    }]}));
  });
  await page.goto('/');
  await page.getByLabel('このブラウザのビュー').selectOption({label: '以前の個人ビュー'});
  await expect(page.locator('#list-view')).toHaveValue('untouched');
  await page.locator('#search').fill('会社');
  await page.locator('#search').press('Enter');
  await page.getByRole('button', {name: '絞り込みを開く'}).click();
  await page.locator('#filter-prefecture').selectOption('5');
  await page.locator('#filter-phone').selectOption('true');
  await page.getByRole('button', {name: '適用', exact: true}).click();
  const query = 'q=会社&view=untouched&prefecture=5&has_phone=true';
  const response = await page.request.get(`${baseURL}/api/companies?${query}`);
  expect(response.ok()).toBeTruthy();
  const matched = await response.json();
  await expect(rows).toHaveCount(matched.count);
  await expect(page.locator('#list-count')).toHaveText(`${matched.count} 件`);
  await expect(page.locator('#search')).toHaveValue('会社');
  await page.getByRole('button', {name: '共有架電リスト', exact: true}).click();
  await page.getByLabel('リスト名', {exact: true}).fill(label);
  await page.getByRole('button', {name: '現在の条件で新規保存'}).click();
  await expect(page.locator('#calling-lists-dialog')).not.toBeVisible();
  const colleague = await browser.newContext({ viewport: page.viewportSize() });
  try {
    const other = await colleague.newPage();
    await other.goto(baseURL);
    await expect(other.locator('#saved-list-view option')).toHaveCount(1);
    await other.getByRole('button', {name: '共有架電リスト', exact: true}).click();
    await other.getByLabel('保存済みの架電リスト').selectOption({label});
    await expect(other.locator('#calling-lists-dialog')).toContainText('秋田県');
    await expect(other.locator('#calling-lists-dialog')).toContainText('電話番号あり');
    await other.screenshot({path: testInfo.outputPath('shared-calling-list.png')});
    expect(await other.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await other.getByRole('button', {name: '選んだリストを開く'}).click();
    await expect(other.locator('#list-view')).toHaveValue('untouched');
    await expect(other.locator('#filter-prefecture')).toHaveValue('5');
    await expect(other.locator('#search')).toHaveValue('会社');
    await expect(other.locator('#list-count')).toHaveText(`${matched.count} 件`);
    await page.getByRole('button', {name: '条件を解除', exact:true}).click();
    await page.getByRole('button', {name: '共有架電リスト', exact: true}).click();
    await page.getByRole('button', {name: '現在の条件で更新'}).click();
    await expect(page.locator('#calling-lists-dialog')).not.toBeVisible();
    await other.getByRole('button', {name: '共有架電リスト', exact: true}).click();
    await expect(other.locator('#calling-lists-dialog')).toContainText('ほかの担当者が更新しました');
    await expect(other.getByRole('button', {name: '現在の条件で更新'})).toBeDisabled();
    await other.getByRole('button', {name: '選んだリストを開く'}).click();
    await expect(other.locator('#filter-prefecture')).toHaveValue('');
  } finally { await colleague.close(); }

  const full = await (await page.request.get(`${baseURL}/api/companies?q=会社&view=untouched&limit=200`)).json();
  expect(full.count).toBeGreaterThan(25);
  await page.locator('#next-page').click();
  await expect(rows.first()).toHaveAttribute('data-company-id', String(full.rows[25].id));
  await page.getByRole('button', {name: '架電モード', exact: true}).click();
  await expect(page.locator('#record-workspace h1')).toHaveText(full.rows[0].name);
  const second = full.rows[1], note = `合成・別担当による記録 ${testInfo.project.name}`;
  const activity = id => `${baseURL}/api/companies/${id}/activities`;
  const mutation = { 'Origin': baseURL, 'X-Meibo-Local': '1' };
  await page.route(`**/api/call-candidates/${second.id}?**`, route => route.fulfill({status:503, contentType:'application/json', body:JSON.stringify({error:'合成デモの一時エラー'})}), {times:1});
  const ownNote = `合成・架電完了 ${testInfo.project.name}`;
  try {
    await page.getByLabel('活動メモ', {exact:true}).fill(ownNote);
    await page.locator('#record-workspace').getByLabel('営業状態', {exact:true}).selectOption('1');
    await page.getByRole('button', {name: '保存して次へ', exact:true}).click();
    await expect(page.getByRole('button', {name:'次の取引先を再読み込み', exact:true})).toBeVisible();
    await expect(page.getByRole('button', {name:'再試行', exact:true})).toHaveCount(0);
    await expect(page.locator('#activity-form')).toHaveCount(0);
    const changed = await page.request.post(activity(second.id), {headers: mutation, data:{note, status:1}});
    expect(changed.ok()).toBeTruthy();
    await page.getByRole('button', {name:'次の取引先を再読み込み', exact:true}).click();
    await expect(page.locator('#record-workspace h1')).toHaveText(full.rows[2].name);
    await page.screenshot({path:testInfo.outputPath('call-mode.png')});
    const stored = await (await page.request.get(`${baseURL}/api/companies/${full.rows[0].id}`)).json();
    expect(stored.activities.filter(a => a.note === ownNote)).toHaveLength(1);
    await page.getByRole('button', {name: '記録せずスキップ'}).click();
    await expect(page.locator('#record-workspace h1')).toHaveText(full.rows[3].name);
    await page.getByRole('button', {name: '架電モードを終了'}).click();
    await expect(page.locator('#accounts-workspace')).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({path: testInfo.outputPath('calling-list.png'), fullPage:true});
  } finally {
    for (const [id, text] of [[full.rows[0].id, ownNote], [second.id, note]]) {
      const detail = await (await page.request.get(`${baseURL}/api/companies/${id}`)).json();
      const recorded = detail.activities.find(a => a.note === text);
      if (recorded) expect((await page.request.post(`${activity(id)}/${recorded.id}/cancel`, {headers: mutation, data:{}})).ok()).toBeTruthy();
    }
  }
  await page.getByRole('button', {name: '共有架電リスト', exact: true}).click();
  page.once('dialog', confirm => confirm.accept());
  await page.getByRole('button', {name: 'リストを削除', exact:true}).click();
  await expect(page.locator('#calling-lists-dialog')).not.toBeVisible();
});
