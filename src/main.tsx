import { lazy, StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './app/App';
import { PageBoundary } from './components/PageBoundary';
import './styles/base.css';

const SettingsPanel = lazy(() => import('./components/SettingsPanel').then(module => ({ default: module.SettingsPanel })));
const UtilityWindow = lazy(() => import('./components/UtilityWindow').then(module => ({ default: module.UtilityWindow })));
const ShellStatus = lazy(() => import('./components/ShellStatus').then(module => ({ default: module.ShellStatus })));
const parameters = new URLSearchParams(location.search);
const utility = parameters.get('utility');

const root = document.getElementById('root');
if (!root) throw new Error('缺少应用挂载节点');

createRoot(root).render(
  <StrictMode>
    <PageBoundary fullWindow>
      {parameters.has('background') ? null : parameters.has('shell-status') ? <ShellStatus /> : parameters.has('settings') || utility === 'settings' ? <SettingsPanel /> : utility === 'resources' || utility === 'help' || utility === 'about' ? <UtilityWindow view={utility} /> : <App />}
    </PageBoundary>
  </StrictMode>,
);
