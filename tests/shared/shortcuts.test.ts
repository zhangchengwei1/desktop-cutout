import assert from 'node:assert/strict';
import test from 'node:test';
import { shortcutFromEvent, shortcutIssue, resolveShortcuts } from '../../src/shared/shortcuts.ts';

test('keyboard capture uses physical keys and portable modifier order', () => {
  const event = { code: 'KeyR', ctrlKey: true, metaKey: false, altKey: false, shiftKey: true };
  assert.equal(shortcutFromEvent(event), 'CmdOrCtrl+Shift+R');
  assert.equal(shortcutFromEvent({ ...event, ctrlKey: false, metaKey: true }), 'CmdOrCtrl+Shift+R');
  assert.equal(shortcutFromEvent({ ...event, ctrlKey: false, shiftKey: false }), null);
  assert.equal(shortcutFromEvent({ ...event, code: 'Comma', shiftKey: false }), 'CmdOrCtrl+,');
});
test('shortcut conflicts include defaults, disabled overrides free keys, system keys stay reserved', () => {
  assert.equal(shortcutIssue('export', 'CmdOrCtrl+O', {}), 'conflict');
  assert.equal(shortcutIssue('export', 'CmdOrCtrl+O', { import: '' }), null);
  assert.equal(shortcutIssue('export', 'CmdOrCtrl+V', {}), 'reserved');
  assert.equal(resolveShortcuts({ import: '' }).import, '');
  assert.equal(resolveShortcuts({}).import, 'CmdOrCtrl+O');
});
