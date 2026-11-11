import { test, expect } from '@playwright/test';

test('synthetic company search, activity persistence and CSV export', async ({ page, context, baseURL }, testInfo) => {
  const errors = [];
  const external = [];
  page.on('pageerror', () => errors.push('pageerror'));
  page.on('console', (message) => { if (message.type() === 'error') errors.push('console.error'); });
  await context.route('**/*', (route) => {
    if (new URL(route.request().url()).origin !== baseURL) {
      external.push('external request');
      return route.abort();
    }
    return route.continue();
  });
  const fits = async () => {
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  };
  await page.goto('/');
  await expect(page.locator('.demo-badge')).toContainText('合成デモ');
  const summary = await (await context.request.get(`${baseURL}/api/summary`)).json();
  expect(summary.origin).toBe('synthetic');
  expect(summary.total).toBe(100);
  const rows = page.locator('#company-rows tr');
  await expect(rows.first()).toBeVisible();
  const company = rows.first().getByRole('button').first();
  const name = (await company.innerText()).trim();
  await fits();

  await page.locator('#search').fill('E2E_NO_SUCH_SYNTHETIC_COMPANY_20261111');
  await page.locator('#search').press('Enter');
  await expect(rows).toHaveCount(0);
  await expect(page.locator('#table-container')).toHaveAttribute('aria-busy', 'false');
  await expect(page.locator('#list-state')).toBeVisible();
  await expect(page.locator('#list-state h3')).toHaveText('該当する取引先がありません');
  await page.getByRole('button', { name: '検索をクリア', exact: true }).click();
  await expect(rows.first()).toBeVisible();
  await page.locator('#search').fill(name);
  await page.locator('#search').press('Enter');
  await expect(rows.first()).toContainText(name);
  await rows.first().getByRole('button').first().focus();
  await page.keyboard.press('Enter');
  await expect(page.locator('#record-workspace')).toBeVisible();
  await expect(page.locator('#record-workspace h1')).toHaveText(name);
  await fits();

  const note = `合成デモのブラウザ試験 ${testInfo.project.name}`;
  await page.getByLabel('活動メモ', { exact: true }).fill(note);
  await page.locator('#record-workspace').getByLabel('営業状態', { exact: true }).selectOption('1');
  await page.getByRole('button', { name: '架電の記録を保存', exact: true }).click();
  await expect(page.locator('#record-workspace')).toContainText(note);
  await page.reload();
  await expect(rows.first()).toBeVisible();
  await page.locator('#search').fill(name);
  await page.locator('#search').press('Enter');
  await expect(rows.first()).toContainText(name);
  await rows.first().getByRole('button').first().click();
  await expect(page.locator('#record-workspace')).toContainText(note);
  await expect(page.locator('#record-workspace').getByLabel('営業状態', { exact: true })).toHaveValue('1');
  await fits();
  const screenshot = testInfo.outputPath('synthetic-record.png');
  await page.screenshot({ path: screenshot, fullPage: true });
  await testInfo.attach('synthetic-record', { path: screenshot, contentType: 'image/png' });

  await page.getByRole('button', { name: '一覧に戻る', exact: true }).click();
  await expect(page.locator('#accounts-workspace')).toBeVisible();
  const downloadPromise = page.waitForEvent('download');
  await page.getByRole('button', { name: 'CSVを書き出す', exact: true }).click();
  const download = await downloadPromise;
  expect(download.suggestedFilename()).toBe('meibo-companies.csv');
  expect(await download.failure()).toBeNull();
  const stream = await download.createReadStream();
  const chunks = [];
  for await (const chunk of stream) chunks.push(chunk);
  const csv = Buffer.concat(chunks).toString('utf8');
  expect(csv).toContain('corporate_number');
  expect(csv).toContain(name);
  await fits();
  expect(errors).toEqual([]);
  expect(external).toEqual([]);
});
