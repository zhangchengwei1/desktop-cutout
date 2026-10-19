"""Generate local Finder Quick Actions; selection paths are passed as arguments."""
import json
import plistlib
from pathlib import Path
import sys
import uuid

root = Path(__file__).resolve().parent.parent
destination = Path(sys.argv[1])
for mode in ('single', 'batch'):
    name = f'ClearFrame {mode.title()}.workflow'
    contents = destination / name / 'Contents'
    resources = contents / 'Resources'
    resources.mkdir(parents=True, exist_ok=True)
    title = json.loads((root / 'src-tauri/installer/context/locales/en.json').read_text())[mode]
    info = {
        'CFBundleIdentifier': f'dev.silta.clearframe.desktop.context.{mode}',
        'CFBundleName': title, 'CFBundleVersion': '1', 'CFBundleDevelopmentRegion': 'en',
        'NSServices': [{
            'NSMenuItem': {'default': title}, 'NSMessage': 'runWorkflowAsService',
            'NSSendTypes': ['public.file-url'],
            'NSRequiredContext': {'NSApplicationIdentifier': 'com.apple.finder'},
        }],
    }
    # At installation the helper substitutes a shell-quoted executable path.
    command = f'''binary=__CLEARFRAME_EXECUTABLE__
if [ ! -x "$binary" ]; then
  printf '%s\\n' 'ClearFrame Desktop has moved or is unavailable. Re-enable Finder actions from the app.' >&2
  exit 1
fi
"$binary" --shell-cutout {mode} -- "$@" </dev/null >/dev/null 2>&1 &
'''
    action = {
        'AMAccepts': {'Container': 'List', 'Optional': False, 'Types': ['com.apple.cocoa.path']},
        'AMProvides': {'Container': 'List', 'Types': ['com.apple.cocoa.string']},
        'AMActionVersion': '2.0.3', 'AMApplication': ['Automator'],
        'ActionBundlePath': '/System/Library/Automator/Run Shell Script.action',
        'ActionName': 'Run Shell Script', 'BundleIdentifier': 'com.apple.RunShellScript',
        'Class Name': 'RunShellScriptAction', 'CFBundleVersion': '2.0.3',
        'ActionParameters': {'COMMAND_STRING': command, 'CheckedForUserDefaultShell': True,
                             'inputMethod': 1, 'shell': '/bin/sh', 'source': command},
        'CanShowWhenRun': False, 'CanShowSelectedItemsWhenRun': False,
        'UUID': str(uuid.uuid5(uuid.NAMESPACE_URL, f'clearframe:{mode}:action')),
        'InputUUID': str(uuid.uuid5(uuid.NAMESPACE_URL, f'clearframe:{mode}:input')),
        'OutputUUID': str(uuid.uuid5(uuid.NAMESPACE_URL, f'clearframe:{mode}:output')),
        'isViewVisible': True,
    }
    workflow = {
        'AMApplicationBuild': '523', 'AMApplicationVersion': '2.10', 'AMDocumentVersion': '2',
        'actions': [{'action': action, 'isViewVisible': True}], 'connectors': [],
        'workflowMetaData': {
            'workflowTypeIdentifier': 'com.apple.Automator.servicesMenu',
            'serviceInputTypeIdentifier': 'com.apple.Automator.fileSystemObject',
            'serviceOutputTypeIdentifier': 'com.apple.Automator.nothing',
            'serviceApplicationBundleID': 'com.apple.finder', 'serviceApplicationPath': '/System/Library/CoreServices/Finder.app',
            'serviceProcessesInput': 0, 'serviceOutputType': 0, 'serviceInputType': 2,
        },
    }
    for path, value in [(contents / 'Info.plist', info), (resources / 'document.wflow', workflow)]:
        path.write_bytes(plistlib.dumps(value))
    for language, folder in [('en', 'en'), ('zh', 'zh_CN'), ('ja', 'ja')]:
        translated = json.loads((root / f'src-tauri/installer/context/locales/{language}.json').read_text())[mode]
        localized = resources / f'{folder}.lproj'
        localized.mkdir(exist_ok=True)
        (localized / 'ServicesMenu.strings').write_text(f'{json.dumps(title, ensure_ascii=False)} = {json.dumps(translated, ensure_ascii=False)};\n')
