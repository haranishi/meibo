import { expect, test } from '@playwright/test';

test('sales meeting report switches months and records an actual decision date', async ({ page, baseURL }, testInfo) => {
  const year = { 'chromium-390': 2131, 'chromium-768': 2132, 'chromium-1440': 2133 }[testInfo.project.name];
  const month = `${year}-03`;
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto('/');
  const list = await page.request.get('/api/companies?limit=20');
  expect(list.ok()).toBeTruthy();
  const company = (await list.json()).rows.find((row) => row.status !== 5);
  const dealName = `合成受注 ${year}`;
  const headers = { Origin: baseURL, 'X-Meibo-Local': '1' };
  async function create(name, amount, stage, close, decision) {
    const saved = await page.request.post('/api/opportunities', {
      headers, data: { name, amount_jpy: amount, stage, company_id: company.id, close_date: close, outcome_date: decision },
    });
    expect(saved.status()).toBe(201);
  }
  await create(dealName, 320000, 4, `${year}-04-10`, `${month}-01`);
  await create(`合成失注 ${year}`, 70000, 5, `${year}-02-07`, `${month}-31`);
  await create(`合成見込み ${year}`, 210000, 2, `${month}-28`, null);
  await page.getByRole('button', { name: '商談', exact: true }).filter({visible:true}).first().click();
  const workspace = page.locator('#crm-workspace');
  await workspace.getByRole('button', { name:'月次集計', exact:true }).click();
  await workspace.getByLabel('対象月').fill(month);
  const totals = workspace.locator('.sales-report-totals');
  await expect(totals).toContainText('￥320,000');
  await expect(totals).toContainText('50%');
  const forecast = workspace.locator('.sales-report-table');
  await expect(forecast).toContainText('￥210,000');
  await expect(forecast).toContainText(company.owner == null ? '未割当' : `担当 ${company.owner}`);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBeTruthy();
  const screenshot = testInfo.outputPath('synthetic-sales-report.png');
  await page.screenshot({path:screenshot, fullPage:true});
  await testInfo.attach('synthetic-sales-report', {path:screenshot, contentType:'image/png'});
  await workspace.getByRole('button', {name:'商談一覧に戻る'}).click();
  await workspace.getByRole('button', {name:dealName, exact:true}).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.getByLabel('完了予定日')).toHaveValue(`${year}-04-10`);
  await dialog.getByLabel('決着日', {exact:true}).fill(`${year}-04-01`);
  await dialog.getByRole('button', {name:'保存', exact:true}).click();
  await expect(dialog).not.toBeVisible();
  await workspace.getByRole('button', {name:'月次集計', exact:true}).click();
  await expect(totals).toContainText('0%');
  await workspace.getByLabel('対象月').fill(`${year}-04`);
  await expect(totals).toContainText('100%');
  await workspace.getByLabel('対象月').fill(`${year}-05`);
  await expect(totals).toContainText('—');
  await expect(totals).toContainText('対象月に決着した商談がないため未算出です。');
  await expect(workspace).toContainText('対象月に完了予定の未決着商談はありません');
  expect(errors).toEqual([]);
});
