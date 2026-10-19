import defaults from '../../resources/shortcuts.json' with { type: 'json' };

export type ShortcutAction = keyof typeof defaults;
export type ShortcutOverrides = Partial<Record<ShortcutAction, string>>;
export const shortcutActions = Object.keys(defaults) as ShortcutAction[];
export function resolveShortcuts(overrides: ShortcutOverrides = {}) { return { ...defaults, ...overrides }; }

export function shortcutFromEvent(event: Pick<KeyboardEvent, 'code' | 'ctrlKey' | 'metaKey' | 'altKey' | 'shiftKey'>): string | null {
  const punctuation: Record<string, string> = { Comma: ',', Period: '.', Semicolon: ';', Slash: '/', BracketLeft: '[', BracketRight: ']', Minus: '-', Equal: '=', Backslash: '\\' };
  const key = /^Key[A-Z]$/.test(event.code) ? event.code.slice(3) : /^Digit[0-9]$/.test(event.code) ? event.code.slice(5) : /^F([1-9]|1[0-2])$/.test(event.code) ? event.code : punctuation[event.code];
  if (!key) return null;
  if (!event.ctrlKey && !event.metaKey && !event.altKey && !/^F\d+$/.test(key)) return null;
  return [...(event.metaKey || event.ctrlKey ? ['CmdOrCtrl'] : []), ...(event.altKey ? ['Alt'] : []), ...(event.shiftKey ? ['Shift'] : []), key].join('+');
}

export function shortcutIssue(action: ShortcutAction, shortcut: string, overrides: ShortcutOverrides): 'conflict' | 'reserved' | null {
  if (['CmdOrCtrl+Q', 'CmdOrCtrl+W', 'CmdOrCtrl+C', 'CmdOrCtrl+X', 'CmdOrCtrl+V', 'CmdOrCtrl+A', 'CmdOrCtrl+Z', 'CmdOrCtrl+Shift+Z', 'CmdOrCtrl+Y', 'Alt+F4'].includes(shortcut)) return 'reserved';
  return shortcut && shortcutActions.some(other => other !== action && resolveShortcuts(overrides)[other] === shortcut) ? 'conflict' : null;
}

export function displayShortcut(value: string, mac = navigator.userAgent.includes('Mac')): string {
  return value.replace('CmdOrCtrl', mac ? '⌘' : 'Ctrl').replace('Alt', mac ? '⌥' : 'Alt');
}
