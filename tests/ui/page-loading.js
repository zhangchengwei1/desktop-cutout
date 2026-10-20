// Run with playwright-cli run-code --filename tests/ui/page-loading.js on Vite.
// Hold real module/translation requests instead of relying on a fast cached load.
async (page) => {
  const base = 'http://localhost:1420';
  const assert = (value, message) => { if (!value) throw new Error(message); };
  const errors = [];
  const prepare = async target => {
    target.on('pageerror', error => errors.push(String(error)));
    await target.addInitScript(() => Object.defineProperty(navigator, 'language', { value: 'zh-CN' }));
  };
  const hold = async (target, pattern) => {
    let release, requested;
    const gate = new Promise(resolve => { release = resolve; });
    const request = new Promise(resolve => { requested = resolve; });
    await target.route(pattern, async route => { requested(); await gate; await route.continue(); });
    return { release, request };
  };
  await prepare(page);
  const component = await hold(page, '**/components/BatchPanel.tsx*');
  const translations = await hold(page, '**/locales/zh/batch.json*');
  await page.goto(base);
  await page.getByRole('button', { name: '批量任务', exact: true }).click();
  await component.request;
  await page.locator('.batch-surface .page-state').waitFor();
  assert(await page.locator('.app-header').isVisible(), 'Lazy code must not hide navigation');
  await page.getByRole('button', { name: '工作台', exact: true }).click();
  assert(await page.locator('.empty-workspace').isVisible(), 'Work must be reachable while batch code is pending');
  component.release();
  await translations.request;
  await page.getByRole('button', { name: '批量任务', exact: true }).click();
  for (const [width, height] of [[1100, 760], [720, 560]]) {
    await page.setViewportSize({ width, height });
    assert(await page.locator('.app-header').isVisible(), 'Pending translations must not hide navigation');
    assert(await page.getByRole('status').filter({ hasText: '正在加载页面' }).isVisible(), 'Loading copy must match the UI language');
    assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), 'Loading must not overflow horizontally');
    await page.screenshot({ path: `test-output/ui/switch-loading-${width}.png` });
    await page.getByRole('button', { name: '工作台', exact: true }).click();
    await page.screenshot({ path: `test-output/ui/switch-empty-${width}.png` });
    await page.getByRole('button', { name: '批量任务', exact: true }).click();
  }
  translations.release();
  await page.getByRole('heading', { name: '批量任务', exact: true }).waitFor();
  for (let index = 0; index < 5; index++) {
    await page.getByRole('button', { name: '工作台', exact: true }).click();
    await page.getByRole('button', { name: '批量任务', exact: true }).click();
    assert(await page.locator('.app-header').isVisible(), 'Repeated switching must retain navigation');
    assert(await page.locator('.batch-surface .page-state').count() === 0, 'Loaded pages must not flash a loading state');
  }

  const failed = await page.context().newPage();
  await prepare(failed);
  await failed.route('**/components/BatchPanel.tsx*', route => route.abort());
  await failed.goto(base);
  await failed.getByRole('button', { name: '批量任务', exact: true }).click();
  await failed.getByRole('heading', { name: '页面未能加载', exact: true }).waitFor();
  assert(await failed.getByRole('button', { name: '重新加载窗口', exact: true }).isVisible(), 'Failed pages must offer recovery');
  await failed.screenshot({ path: 'test-output/ui/switch-load-failed.png' });
  await failed.getByRole('button', { name: '工作台', exact: true }).click();
  assert(await failed.locator('.empty-workspace').isVisible(), 'A failed page must not take down the workbench');
  await failed.close();

  const settings = await page.context().newPage();
  await prepare(settings);
  const shortcuts = await hold(settings, '**/locales/zh/shortcuts.json*');
  await settings.goto(`${base}/?utility=settings`);
  await settings.getByRole('button', { name: '快捷键', exact: true }).click();
  await shortcuts.request;
  await settings.locator('.settings-panel .page-state').waitFor();
  assert(await settings.getByRole('heading', { name: '设置', exact: true }).isVisible(), 'Settings heading must survive a pending tab');
  await settings.getByRole('button', { name: '通用', exact: true }).click();
  assert(await settings.getByRole('combobox').first().isVisible(), 'Settings tabs must remain usable while shortcuts load');
  shortcuts.release();
  await settings.getByRole('button', { name: '快捷键', exact: true }).click();
  await settings.locator('.shortcut-recorder').first().waitFor();
  await settings.close();
  assert(errors.length === 0, errors.join('\n'));
  return { passed: true, viewports: ['1100×760', '720×560'], verified: ['delayed code', 'delayed translations', 'navigate during loading', 'repeat switching', 'recoverable chunk failure', 'settings tab isolation'], pageErrors: errors };
}
