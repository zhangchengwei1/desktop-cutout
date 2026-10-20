"""Opt-in real-model checks for the installed headless entry point (no GUI)."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile

parser = argparse.ArgumentParser()
parser.add_argument('application', type=Path, help='Installed application executable')
parser.add_argument('--models', type=Path, required=True, help='Already verified model cache')
args = parser.parse_args()
root = Path(__file__).resolve().parents[2]
(root / 'test-output').mkdir(exist_ok=True)


def invoke(mode, paths, models=None):
    process = subprocess.run(
        [str(args.application.resolve()), '--context-cutout', mode, '--models',
         str(models or args.models), '--', *map(str, paths)],
        capture_output=True, text=True, timeout=120)
    reports = [json.loads(line) for line in process.stdout.splitlines() if line.startswith('{')]
    return process, reports


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


with tempfile.TemporaryDirectory(prefix='context-cli-', dir=root / 'test-output') as temporary:
    directory = Path(temporary)
    source = directory / "日本語 中文 ' & space.png"
    shutil.copyfile(root / 'tests/fixtures/images/hard-edge-opaque.png', source)
    original = digest(source)
    process, reports = invoke('single', [source], directory / 'missing-model-cache')
    assert process.returncode == 1 and not reports, process.stderr
    assert sorted(directory.iterdir()) == [source], 'Model preflight must not create outputs'

    for _ in range(2):
        process, reports = invoke('single', [source])
        assert process.returncode == 0 and reports[0]['succeeded'] == 1, process.stderr
    saved = [path for path in directory.glob('*.png') if path != source]
    assert len(saved) == 2 and digest(source) == original
    previous = {path: digest(path) for path in saved}
    for path in saved:
        header = path.read_bytes()[:29]
        assert struct.unpack('>II', header[16:24]) == (96, 64)
        assert header[25] == 6, 'Results must remain RGBA PNG'

    folder = directory / 'batch'
    (folder / 'nested').mkdir(parents=True)
    shutil.copyfile(source, folder / 'one.png')
    shutil.copyfile(source, folder / 'nested/one.png')
    (folder / 'broken.jpg').write_bytes(b'broken image')
    (folder / 'ignore.txt').write_text('not an image')
    # A user-created folder with this name is still a valid input directory.
    (folder / 'ClearFrame Cutouts').mkdir()
    shutil.copyfile(source, folder / 'ClearFrame Cutouts/owned-by-user.png')
    for expected_name in ['ClearFrame Cutouts 2', 'ClearFrame Cutouts 3']:
        process, reports = invoke('batch', [folder])
        assert process.returncode == 3 and len(reports) == 1, process.stderr
        report = reports[0]
        assert (report['succeeded'], report['failed'], report['cancelled']) == (3, 1, 0), report
        output = folder / expected_name
        assert Path(report['outputDirectory']) == output
        assert len(list(output.glob('*.png'))) == 3
        assert json.loads((output / 'ClearFrame Report.json').read_text()) == report
        assert len(report['items']) == 4, 'Previous generated folders must not be re-imported'
    assert all(digest(path) == value for path, value in previous.items())
    empty = directory / 'empty'
    empty.mkdir()
    process, _ = invoke('batch', [empty])
    assert process.returncode == 1 and not list(empty.iterdir())
    print('PASS: model preflight, literal paths, single-image no-overwrite, recursive batch, partial failure, repeat import, RGBA size')
