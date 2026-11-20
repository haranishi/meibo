import { expect, test } from '@playwright/test';

test('fill a missing business phone from CSV and replay the same file', async ({ page }, testInfo) => {
  await page.goto('/');
  const response = await page.request.get('/api/companies?has_phone=false&limit=200');
  expect(response.ok()).toBeTruthy();
  const company = (await response.json()).rows.find(row => row.corporate_number && !row.is_sole_proprietor);
  expect(company).toBeDefined();
  const phone = '03-0000-0000';
  const csv = `source,corporate_number,phone,website,industry\nsynthetic,${company.corporate_number},${phone},,F\n`;
  await page.getByRole('button', { name: 'CSVを取り込む', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByLabel('取込方法').selectOption('supplement');
  await expect(dialog.getByLabel('重複時の処理')).toBeHidden();
  await dialog.getByLabel('CSVファイル').setInputFiles({
    name: 'synthetic-contacts.csv', mimeType: 'text/csv', buffer: Buffer.from(csv),
  });
  const submit = dialog.getByRole('button', { name: '取り込む', exact: true });
  const result = dialog.getByRole('status');
  await submit.click();
  await expect(result).toContainText('連絡先を補完 1件、変更なし 0件');
  await submit.click();
  await expect(result).toContainText('連絡先を補完 0件、変更なし 1件');
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  const screenshot = testInfo.outputPath('synthetic-contact-supplement.png');
  await dialog.screenshot({ path: screenshot });
  await testInfo.attach('contact-supplement', { path: screenshot, contentType: 'image/png' });
  await dialog.getByRole('button', { name: '閉じる', exact: true }).click();
  const reread = await page.request.get(`/api/companies/${company.id}`);
  expect(reread.ok()).toBeTruthy();
  const saved = (await reread.json()).company;
  expect(saved.phone).toBe(phone);
  expect(saved.industry).toBe(company.industry ?? 6);
});
