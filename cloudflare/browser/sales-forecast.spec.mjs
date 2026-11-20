import { expect, test } from '@playwright/test';

// Requirement 20.5: edit the deal's owner/probability and the manager's monthly target separately.
test('deal ownership, weighted sales and monthly targets work on each viewport', async ({ page, baseURL }, testInfo) => {
  const year = { 'chromium-390': 2101, 'chromium-768': 2102, 'chromium-1440': 2103 }[testInfo.project.name];
  const month = `${year}-09`;
  const name = `確認対象の合成商談 ${year}`;
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto('/');
  const companies = await page.request.get('/api/companies?limit=20');
  const company = (await companies.json()).rows.find((row) => row.status !== 5);
  const headers = { Origin: baseURL, 'X-Meibo-Local': '1' };
  const common = { company_id: company.id, close_date: `${month}-01`, stage: 2 };
  for (const deal of [
    { name, amount_jpy: 101 },
    { name: `合成受注 ${year}`, amount_jpy: 40, owner_id: 72001, stage: 4, outcome_date: `${month}-30` },
    { name: `確度をまだ入力していない合成商談 ${year}`, amount_jpy: 50, owner_id: 72001 },
    { name: `同じ取引先の別担当 ${year}`, amount_jpy: 100, owner_id: 72002, probability: 0 },
  ]) {
    const saved = await page.request.post('/api/opportunities', { headers, data: { ...common, ...deal } });
    expect(saved.status()).toBe(201);
  }
  await page.getByRole('button', { name: '商談', exact: true }).filter({ visible:true }).first().click();
  const workspace = page.locator('#crm-workspace');
  await workspace.getByRole('button', { name, exact:true }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByLabel('商談担当', { exact:true }).selectOption('72001');
  await dialog.getByLabel('受注確度（%）').fill('50');
  await dialog.getByRole('button', { name:'保存', exact:true }).click();
  await expect(dialog).not.toBeVisible();
  await workspace.getByRole('button', { name:'月次集計', exact:true }).click();
  await workspace.getByLabel('対象月').fill(month);
  const table = workspace.getByRole('table', { name:'商談担当別の着地予測' });
  const owner = table.getByRole('row').filter({ has:page.getByRole('rowheader', { name:'担当 72001' }) });
  await expect(owner.getByRole('cell')).toHaveText(['￥40','￥51','￥91','—','—','1 件']);
  const other = table.getByRole('row').filter({ has:page.getByRole('rowheader', { name:'担当 72002' }) });
  await expect(other.getByRole('cell')).toHaveText(['￥0','￥0','￥0','—','—','0 件']);
  await workspace.getByText('月間目標を設定', { exact:true }).click();
  await workspace.getByLabel('目標の担当').selectOption('72001');
  await workspace.getByLabel('月間目標（円）').fill('120');
  await workspace.getByRole('button', { name:'目標を保存' }).click();
  await expect(owner.getByRole('cell')).toHaveText(['￥40','￥51','￥91','￥120','-￥29','1 件']);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBeTruthy();
  if (testInfo.project.name === 'chromium-768') {
    const rows = await workspace.locator('.sales-report-totals > div').evaluateAll((cards) => cards.map((card) => card.getBoundingClientRect().top));
    expect(rows[1]).toBe(rows[0]); expect(rows[3]).toBe(rows[2]); expect(rows[2]).toBeGreaterThan(rows[0]);
  }
  const path = testInfo.outputPath('synthetic-weighted-forecast.png');
  await page.screenshot({ path, fullPage:true });
  await testInfo.attach('monthly-forecast', { path, contentType:'image/png' });
  await workspace.getByText('月間目標を設定', { exact:true }).click();
  await workspace.getByLabel('目標の担当').selectOption('72001');
  await expect(workspace.getByLabel('月間目標（円）')).toHaveValue('120');
  await workspace.getByRole('button', { name:'目標を解除' }).click();
  await expect(owner.getByRole('cell').nth(3)).toHaveText('—');
  expect(errors).toEqual([]);
});
