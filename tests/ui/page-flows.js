// Run with playwright-cli run-code --filename tests/ui/page-flows.js on Vite.
// The second half deliberately simulates IPC in an isolated browser page.
// It verifies frontend states only; it is not evidence of native inference.
async (page) => {
  const base = 'http://localhost:1420';
  const assert = (value, message) => { if (!value) throw new Error(message); };
  const screenshots = 'test-output/ui';
  const errors = [];
  page.on('pageerror', (error) => errors.push(String(error)));
  const capture = async (name) => {
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth > innerWidth);
    assert(!overflow, `${name}: horizontal overflow`);
    await page.screenshot({ path: `${screenshots}/${name}.png` });
  };
  await page.goto(base);
  for (const [width, height] of [[1100, 760], [720, 560]]) {
    await page.setViewportSize({ width, height });
    await page.getByRole('button', { name: '工作台', exact: true }).click();
    assert(await page.getByRole('button', { name: '导入图片', exact: true }).isDisabled(), 'Browser import must be unavailable');
    await capture(`browser-work-${width}`);
    await page.getByRole('button', { name: '批量任务', exact: true }).click();
    await page.getByRole('heading', { name: '批量任务', exact: true }).waitFor();
    await capture(`browser-batch-${width}`);
    assert(await page.locator('.app-header button:visible').count() === 2, 'Only core navigation belongs in the header');
    assert(await page.getByRole('button', { name: '设置', exact: true }).count() === 0, 'Settings must be absent from the core surface');
    assert(await page.getByRole('button', { name: '查看操作指南', exact: true }).count() === 0, 'Empty workspace must not duplicate the help entry');
    for (const [name, view] of [['设置…', 'settings'], ['模型与资源', 'resources'], ['帮助', 'help'], ['关于 ClearFrame', 'about']]) {
      await page.locator('.preview-application-menu summary').click();
      const opened = page.waitForEvent('popup');
      await page.getByRole('button', { name, exact: true }).click();
      const utility = await opened;
      await utility.setViewportSize({ width, height });
      utility.on('pageerror', error => errors.push(String(error)));
      await utility.waitForURL(`**/?utility=${view}`);
      await utility.locator('main, .settings-panel').waitFor();
      if (view === 'settings') assert(await utility.getByRole('combobox').first().isDisabled(), 'Preview settings must be read-only');
      if (view === 'about') await utility.getByRole('heading', { name: '关于应用', exact: true }).waitFor();
      await utility.screenshot({ path: `${screenshots}/browser-${view}-${width}.png` });
      await utility.close();
    }
  }

  const context = page.context();
  const simulated = await context.newPage();
  simulated.on('pageerror', (error) => errors.push(String(error)));
  await simulated.addInitScript(() => {
    const callbacks = new Map();
    const listeners = new Map();
    let sequence = 1;
    const calls = [];
    const name = 'a-very-long-original-filename-用于检查截断与布局-'.repeat(3) + '.png';
    const asset = { assetId: 1, workflowId: 2, displayName: name, width: 96, height: 64, generation: 1, maskRevision: 1, resultRevision: 1, backgroundRevision: 1, background: { kind: 'transparent' }, previewRevision: 1, previewAvailable: true, resultAvailable: true, workflow: 'automatic', selectionPoints: [], canUndo: true, canRedo: false, state: 'ready', error: null };
    const workspace = { revision: 1, selectedAssetId: 1, assets: [asset], tasks: [], batches: [{ batchId: 3, outputDirectory: '/local/test-output', paused: true, total: 65, queued: 60, running: 0, succeeded: 2, failed: 2, cancelled: 1 }] };
    const items = Array.from({ length: 65 }, (_, index) => ({ itemId: index + 100, batchId: 3, displayName: `${index}-${name}`, outputName: `${index}-cutout.png`, state: index === 58 || index === 64 ? 'failed' : index === 60 ? 'cancelled' : index < 2 ? 'succeeded' : 'queued', stage: 'queued', error: index === 64 ? 'Test-only save error' : null }));
    const models = { revision: 1, models: ['isnet-auto-high', 'mobilesam-image-encoder', 'mobilesam-mask-decoder', 'vitmatte-high'].map((id) => ({ spec: { id, bytes: 100, license: 'Test-only license', revision: 'test-only-revision', sha256: 'test-only-checksum', verifiedBackend: 'Simulated browser IPC', licenseUrl: 'https://example.test' }, state: 'file_verified', receivedBytes: 100, error: null })), features: Object.fromEntries(['automatic', 'selection', 'matting'].map((key) => [key, { filesVerified: true, sessionReady: false, missingModelIds: [] }])) };
    if (location.search.includes('ui-model-setup')) { models.models[0].state = 'downloading'; models.models[0].receivedBytes = 10; models.features.automatic.filesVerified = false; }
    const preferences = { revision: 1, value: { schemaVersion: 1, language: 'zh', exportNaming: 'cutout', restoreSession: false, batchOutputDirectory: null }, loadError: null };
    const emit = (event) => { for (const [, listener] of listeners) if (listener.event === event) callbacks.get(listener.handler)?.({ event, payload: workspace.revision }); };
    window.isTauri = true;
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: (_, id) => listeners.delete(id) };
    window.__TAURI_INTERNALS__ = {
      metadata: { currentWebview: { label: 'main' }, currentWindow: { label: 'main' } },
      transformCallback: (handler) => { const id = sequence++; callbacks.set(id, handler); return id; },
      invoke: async (command, args = {}) => {
        calls.push({ command, args });
        if (command === 'plugin:event|listen') { const id = sequence++; listeners.set(id, args); return id; }
        if (command === 'plugin:event|unlisten') return;
        if (command === 'runtime_info') return { coreVersion: 'test-only', inferenceReady: false };
        if (command === 'workspace_snapshot') return structuredClone(workspace);
        if (command === 'model_snapshot') return structuredClone(models);
        if (command === 'preferences_snapshot') return structuredClone(preferences);
        if (command === 'recovery_status') return { enabled: false, storageBytes: 0, savedRevision: null, error: null, maintenanceWarning: null, blocked: false };
        if (command === 'preview_url') return `${location.origin}/tests/fixtures/images/hard-edge-opaque.png`;
        if (command === 'batch_result_preview_url') return `${location.origin}/tests/fixtures/images/wide-alpha.png`;
        if (command === 'plugin:dialog|open') return '/local/saved-results';
        if (command === 'open_settings') return false;
        if (command === 'batch_items') {
          const filtered = items.filter((item) => args.filter === 'all' || args.filter === 'issues' && ['failed', 'cancelled', 'stale'].includes(item.state) || args.filter === 'completed' && item.state === 'succeeded' || args.filter === 'active' && ['queued', 'running', 'cancel_requested'].includes(item.state));
          return { total: filtered.length, items: filtered.slice(args.offset, args.offset + args.limit) };
        }
        if (command === 'review_batch_item') return { assetId: 1 };
        if (command === 'create_batch') {
          const batchId = Math.max(...workspace.batches.map(batch => batch.batchId)) + 1;
          workspace.batches.push({ ...workspace.batches[0], batchId, outputDirectory: args.outputDir });
          workspace.revision++; emit('workspace-changed');
          return batchId;
        }
        if (command === 'remove_asset') { workspace.assets = workspace.assets.filter((entry) => entry.assetId !== args.assetId); workspace.selectedAssetId = null; }
        if (command === 'select_asset') workspace.selectedAssetId = args.assetId;
        if (command === 'switch_workflow') asset.workflow = args.workflow;
        if (command === 'select_points') { asset.selectionPoints = args.points; asset.maskRevision++; }
        if (command === 'update_preferences') { preferences.value = args.value; preferences.revision++; emit('preferences-changed'); return structuredClone(preferences); }
        workspace.revision++; emit('workspace-changed');
        return true;
      },
    };
    window.__clearframeUiTest = { calls, workspace, models, emit };
  });
  await simulated.goto(base);
  await simulated.getByRole('button', { name: '导出 PNG', exact: true }).waitFor();
  for (const [width, height] of [[1100, 760], [720, 560]]) {
    await simulated.setViewportSize({ width, height });
    await simulated.getByRole('button', { name: '工作台', exact: true }).click();
    await simulated.waitForFunction(() => {
      const bounds = document.querySelector('.preview-stage')?.getBoundingClientRect();
      return bounds && bounds.width > 200 && bounds.height > 150;
    });
    const bounds = await simulated.locator('.preview-stage').boundingBox();
    assert(bounds && bounds.width > 200 && bounds.height > 150, `Canvas must remain usable at ${width}`);
    await simulated.getByRole('button', { name: '精修边缘', exact: true }).click();
    assert(await simulated.getByRole('button', { name: '应用边缘精修', exact: true }).isVisible(), 'Refinement controls must be reachable');
    assert(!await simulated.getByRole('slider', { name: /画笔大小/ }).count(), 'Only the selected tool should be expanded');
    await simulated.getByRole('button', { name: '背景', exact: true }).click();
    await simulated.getByRole('button', { name: '应用纯色', exact: true }).click();
    await simulated.screenshot({ path: `${screenshots}/simulated-editor-${width}.png` });
    await simulated.getByRole('button', { name: '批量任务', exact: true }).click();
    if (width === 1100) {
      await simulated.getByRole('button', { name: '选择文件夹', exact: true }).click();
      await simulated.locator('.batch-output-path').getByText('/local/saved-results', { exact: true }).waitFor();
      assert(await simulated.evaluate(() => window.__clearframeUiTest.calls.filter((call) => call.command === 'update_preferences').at(-1).args.value.batchOutputDirectory) === '/local/saved-results', 'Output folder must be stored in native preferences');
      assert(await simulated.locator('.batch-summary').getByText('/local/test-output', { exact: true }).isVisible(), 'Changing the default must not move an existing batch');
    }
    await simulated.getByRole('button', { name: '已保存', exact: true }).click();
    await simulated.getByRole('button', { name: '查看处理结果', exact: true }).first().click();
    await simulated.locator('.batch-result-image img').waitFor({ state: 'visible' });
    assert(await simulated.locator('.batch-result-image img').evaluate((image) => image.complete && image.naturalWidth > 0), 'Saved result preview must load');
    assert(await simulated.evaluate(() => window.__clearframeUiTest.calls.some((call) => call.command === 'batch_result_preview_url' && call.args.itemId === 100)), 'Preview must request the saved batch result by item ID');
    await simulated.screenshot({ path: `${screenshots}/simulated-batch-result-${width}.png` });
    await simulated.getByRole('button', { name: '收起结果', exact: true }).click();
    assert(await simulated.locator('#batch-view-100').evaluate((button) => document.activeElement === button), 'Closing a result must restore keyboard focus');
    if (width === 1100) {
      await simulated.route('**/tests/fixtures/images/wide-alpha.png*', (route) => route.abort());
      await simulated.getByRole('button', { name: '查看处理结果', exact: true }).first().click();
      await simulated.locator('.batch-result-error').waitFor();
      await simulated.unroute('**/tests/fixtures/images/wide-alpha.png*');
      await simulated.getByRole('button', { name: '重新读取', exact: true }).click();
      await simulated.locator('.batch-result-image img').waitFor({ state: 'visible' });
      await simulated.getByRole('button', { name: '收起结果', exact: true }).click();
      await simulated.getByRole('button', { name: '新建批次', exact: true }).click();
      const commands = await simulated.evaluate(() => window.__clearframeUiTest.calls);
      const start = commands.findIndex((call) => call.command === 'create_batch');
      assert(start >= 0 && commands[start].args.outputDir === '/local/saved-results', 'New batches must use the saved output folder');
      assert(!commands.slice(start - 1, start).some((call) => call.command === 'plugin:dialog|open' && call.args.options.directory), 'Saved folders must avoid a second folder prompt');
    }
    await simulated.getByRole('button', { name: '失败与取消', exact: true }).click();
    await simulated.locator('.batch-list li').first().waitFor();
    assert(await simulated.locator('.batch-list li').count() === 3, 'Rust-filtered pages must expose issues beyond the first 50 jobs');
    await simulated.screenshot({ path: `${screenshots}/simulated-batch-${width}.png` });
    await simulated.getByRole('button', { name: '打开原图复核', exact: true }).first().click();
    await simulated.getByRole('button', { name: '返回批量任务', exact: true }).click();
    assert(await simulated.getByRole('button', { name: '失败与取消', exact: true }).getAttribute('aria-pressed') === 'true', 'Review must preserve filters');
    assert(await simulated.locator('.app-header button').count() === 2, 'Native core window must not duplicate OS menus');
  }
  await simulated.getByRole('button', { name: '工作台', exact: true }).click();
  await simulated.getByRole('button', { name: '点选抠图', exact: true }).click();
  const canvas = simulated.getByRole('group', { name: '点选抠图', exact: true });
  await canvas.focus();
  await canvas.press('ArrowRight');
  await canvas.press('Enter');
  const points = await simulated.evaluate(() => window.__clearframeUiTest.calls.filter((call) => call.command === 'select_points').at(-1).args.points);
  assert(points.length === 1 && points[0].x === 0.51 && points[0].label === 1, 'Keyboard point selection must use normalized image coordinates');
  await simulated.getByRole('button', { name: '画笔修补', exact: true }).click();
  const brushLayer = simulated.locator('svg.stroke-layer');
  await brushLayer.waitFor();
  const brushBounds = await brushLayer.boundingBox();
  assert(brushBounds, 'Brush overlay must have dimensions');
  await simulated.mouse.move(brushBounds.x + brushBounds.width * 0.25, brushBounds.y + brushBounds.height * 0.5);
  await simulated.mouse.down();
  await simulated.mouse.move(brushBounds.x + brushBounds.width * 0.75, brushBounds.y + brushBounds.height * 0.5, { steps: 3 });
  await simulated.mouse.up();
  const painted = await simulated.evaluate(() => window.__clearframeUiTest.calls.filter(call => call.command === 'paint_mask').at(-1)?.args.stroke);
  assert(painted && painted.radius === 16 && painted.value === 255 && painted.points.length > 1, 'Brush must submit source radius and normalized stroke');
  assert(Math.abs(painted.points[0].x - 0.25) < 0.01 && Math.abs(painted.points.at(-1).x - 0.75) < 0.01, 'Stroke must use source coordinates');
  await simulated.goto(`${base}/?utility=settings`);
  await simulated.getByRole('button', { name: '快捷键', exact: true }).click();
  const importShortcut = simulated.getByRole('button', { name: '导入图片…', exact: true });
  await importShortcut.click();
  await importShortcut.getByText('请按组合键…', { exact: true }).waitFor();
  await importShortcut.press('Control+i');
  await importShortcut.getByText(/Ctrl\+I|⌘\+I/).waitFor();
  assert(await simulated.evaluate(() => window.__clearframeUiTest.calls.filter(call => call.command === 'update_preferences').at(-1).args.value.shortcuts.import) === 'CmdOrCtrl+I', 'Custom shortcut must persist in native preferences');
  const exportShortcut = simulated.getByRole('button', { name: '导出 PNG…', exact: true });
  await exportShortcut.click();
  await exportShortcut.getByText('请按组合键…', { exact: true }).waitFor();
  await exportShortcut.press('Control+i');
  await simulated.getByRole('alert').filter({ hasText: '此组合键已分配给其他操作' }).waitFor();
  await exportShortcut.press('Escape');
  await simulated.getByRole('button', { name: '禁用: 导入图片…', exact: true }).click();
  await importShortcut.getByText('未分配', { exact: true }).waitFor();
  await simulated.getByRole('button', { name: '恢复默认快捷键', exact: true }).click();
  await importShortcut.getByText(/Ctrl\+O|⌘\+O/).waitFor();
  assert(await simulated.evaluate(() => window.__clearframeUiTest.calls.filter(call => call.command === 'set_shortcut_recording').some(call => call.args.recording === false)), 'Recording must restore native accelerators');
  await simulated.screenshot({ path: `${screenshots}/simulated-shortcuts-720.png` });
  await simulated.getByRole('button', { name: '通用', exact: true }).click();
  let releaseLanguage, languageRequested;
  const languageGate = new Promise(resolve => { releaseLanguage = resolve; });
  const languageRequest = new Promise(resolve => { languageRequested = resolve; });
  await simulated.route('**/locales/ja/settings.json*', async route => { languageRequested(); await languageGate; await route.continue(); });
  await simulated.getByRole('combobox').first().selectOption('ja');
  await languageRequest;
  assert(await simulated.getByRole('heading', { name: '设置', exact: true }).isVisible(), 'Language changes must retain the current page until translations are ready');
  assert(await simulated.locator('.settings-tabs').isVisible(), 'Language loading must not hide settings navigation');
  releaseLanguage();
  await simulated.getByRole('heading', { name: '設定', exact: true }).waitFor();
  await simulated.unroute('**/locales/ja/settings.json*');
  await simulated.getByRole('combobox').first().selectOption('en');
  await simulated.getByRole('heading', { name: 'Settings', exact: true }).waitFor();
  assert(await simulated.evaluate(() => window.__clearframeUiTest.calls.some(call => call.command === 'set_menu_language' && call.args.language === 'en')), 'Settings must synchronize native menu language');
  await simulated.goto(`${base}/?utility=about`);
  await simulated.getByRole('heading', { name: '关于应用', exact: true }).waitFor();
  assert(await simulated.locator('.primary-nav').count() === 0, 'About must be a standalone utility surface');
  await simulated.goto(`${base}/?ui-model-setup`);
  await simulated.getByRole('dialog').waitFor();
  for (const key of ['Tab', 'Tab', 'Shift+Tab']) {
    await simulated.keyboard.press(key);
    assert(await simulated.getByRole('dialog').evaluate(dialog => dialog.contains(document.activeElement)), 'Modal must constrain keyboard focus');
  }
  assert(await simulated.locator('.desktop-shell').evaluate(shell => !!shell.closest('[aria-hidden="true"]')), 'Modal must hide the background from assistive technology');
  await simulated.getByRole('dialog').press('Escape');
  assert(await simulated.evaluate(() => window.__clearframeUiTest.calls.some(call => call.command === 'open_utility_window' && call.args.view === 'resources')), 'Setup must open models in a separate window');
  assert(await simulated.getByRole('dialog').count() === 0, 'Model downloads must allow leaving setup');
  await simulated.getByRole('button', { name: '工作台', exact: true }).click();
  assert(await simulated.getByRole('dialog').count() === 0, 'Dismissed setup must not reopen while downloading');
  await simulated.evaluate(() => {
    const test = window.__clearframeUiTest;
    test.workspace.assets.push({ ...test.workspace.assets[0], assetId: 99, displayName: 'remaining-test-original.png' });
    test.workspace.revision++; test.emit('workspace-changed');
  });
  await simulated.getByRole('button', { name: '移除', exact: true }).click();
  await simulated.locator('.canvas-toolbar-title').getByText('remaining-test-original.png', { exact: true }).waitFor();
  assert(await simulated.getByRole('button', { name: '导出 PNG', exact: true }).isVisible(), 'Removing the selected image must keep remaining assets accessible');
  await simulated.close();
  assert(errors.length === 0, errors.join('\n'));
  return { passed: true, viewports: ['1100×760', '720×560'], runtime: 'browser + deliberately simulated IPC', pageErrors: errors, verified: ['preview read-only', 'native core navigation + standalone utility windows', 'saved batch result preview', 'remember output folder', 'existing batch folder unchanged', 'tool switching', 'filtered pagination', 'review preserves filters', 'keyboard point coordinates', 'normalized brush strokes', 'modal keyboard focus', 'language synchronization', 'dismiss model setup during download', 'remaining assets accessible after removal'] };
}
