import { open, save } from '@tauri-apps/plugin-dialog';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import formats from '../../resources/image-formats.json';

const imageFilters = [{ name: 'Images', extensions: formats.extensions }];

export async function chooseImages(): Promise<string[]> {
  const selected = await open({ multiple: true, directory: false, filters: imageFilters });
  return selected ? (Array.isArray(selected) ? selected : [selected]) : [];
}

export async function choosePngDestination(defaultPath: string): Promise<string | null> {
  return save({ defaultPath, filters: [{ name: 'PNG', extensions: ['png'] }] });
}

export async function chooseBackgroundImage(): Promise<string | null> {
  const selected = await open({ multiple: false, directory: false, filters: imageFilters });
  return typeof selected === 'string' ? selected : null;
}

export async function chooseOutputDirectory(defaultPath?: string): Promise<string | null> {
  const selected = await open({ multiple: false, directory: true, defaultPath });
  return typeof selected === 'string' ? selected : null;
}

export async function chooseImageDirectory(): Promise<string | null> {
  const selected = await open({ multiple: false, directory: true });
  return typeof selected === 'string' ? selected : null;
}

export async function chooseZipArchive(): Promise<string | null> {
  const selected = await open({ multiple: false, directory: false, filters: [{ name: 'ZIP', extensions: ['zip'] }] });
  return typeof selected === 'string' ? selected : null;
}

export async function chooseZipDestination(defaultPath: string): Promise<string | null> {
  return save({ defaultPath, filters: [{ name: 'ZIP', extensions: ['zip'] }] });
}

export async function listenImageDrops(onPaths: (paths: string[]) => void): Promise<() => void> {
  return getCurrentWebview().onDragDropEvent((event) => {
    if (event.payload.type === 'drop') onPaths(event.payload.paths);
  });
}
