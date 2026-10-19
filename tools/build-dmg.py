"""将已签名的 Tauri 应用打包，并直接写入 Finder 布局。"""
import json
import os
from pathlib import Path
import subprocess

import dmgbuild

root = Path(__file__).resolve().parent.parent
config = json.loads((root / 'src-tauri/tauri.conf.json').read_text())
layout = json.loads((root / 'src-tauri/tauri.installer.macos.conf.json').read_text())['bundle']['macOS']['dmg']
app_name = f"{config['productName']}.app"
app = root / 'target/release/bundle/macos' / app_name
output = root / 'target/release/bundle/dmg' / f"{config['productName']}_{config['version']}_aarch64.dmg"
output.parent.mkdir(parents=True, exist_ok=True)
temporary = output.with_name(f'.{output.stem}.building.dmg')

settings = {
    'format': 'UDZO',
    # 保留签名后的应用扩展属性；隐藏扩展名会添加 FinderInfo，破坏 strict 校验。
    'files': [str(app)],
    'symlinks': {'Applications': '/Applications'},
    'icon': str(root / 'src-tauri/icons/icon.icns'),
    'background': str(root / 'src-tauri' / layout['background']),
    'icon_locations': {
        app_name: (layout['appPosition']['x'], layout['appPosition']['y']),
        'Applications': (layout['applicationFolderPosition']['x'], layout['applicationFolderPosition']['y']),
    },
    'window_rect': ((200, 200), (layout['windowSize']['width'], layout['windowSize']['height'])),
    'default_view': 'icon-view',
    'include_icon_view_settings': True,
    'show_icon_preview': True,
    'icon_size': 128,
    'text_size': 12,
    # Finder 会拒绝某些较大的网格间距；显式固定，避免系统默认排列覆盖布局。
    'grid_spacing': 50,
    'arrange_by': None,
}

try:
    subprocess.run(['codesign', '--verify', '--deep', '--strict', str(app)], check=True)
    dmgbuild.build_dmg(str(temporary), config['productName'], settings=settings)
    identity = os.environ.get('APPLE_SIGNING_IDENTITY')
    if identity and identity != '-':
        subprocess.run(['codesign', '--force', '--sign', identity, '--timestamp', str(temporary)], check=True)
    subprocess.run(['hdiutil', 'verify', str(temporary)], check=True)
    temporary.replace(output)
    print(f'Created DMG with saved drag-to-install layout: {output}')
finally:
    temporary.unlink(missing_ok=True)
