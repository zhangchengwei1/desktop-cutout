import { useEffect, useRef, useState } from 'react';
import type { PointerEvent } from 'react';
import { useMessages } from '../../i18n';
import type { Language } from '../../platform/preferences';
import type { AssetSnapshot, Stroke } from '../../platform/workspace';

export type WorkspaceTool = 'auto' | 'point' | 'brush' | 'refine' | 'background';
type Position = Stroke['points'][number];
type Props = {
  asset: AssetSnapshot;
  language: Language;
  imageUrl: string | null;
  previewError: string | null;
  result: boolean;
  tool: WorkspaceTool;
  disabled: boolean;
  selectionReady: boolean;
  newSelection: boolean;
  brushRadius: number;
  brushValue: 0 | 255;
  onPoint: (point: Position) => void;
  onStroke: (stroke: Stroke) => void;
  onImageError: () => void;
  onRecover: () => void;
};

function clamp(value: number): number { return Math.max(0, Math.min(1, value)); }

function pointerPoint(event: PointerEvent<SVGSVGElement>): Position {
  const rect = event.currentTarget.getBoundingClientRect();
  return { x: clamp((event.clientX - rect.left) / rect.width), y: clamp((event.clientY - rect.top) / rect.height) };
}

// Owns display geometry and transient gestures. Persisted masks and selections
// remain in the native workspace; callbacks carry normalized source coordinates.
export function PreviewCanvas({ asset, language, imageUrl, previewError, result, tool, disabled,
  selectionReady, newSelection, brushRadius, brushValue, onPoint, onStroke, onImageError, onRecover }: Props) {
  const { labels: t, feedback: f } = useMessages('app', language);
  const areaRef = useRef<HTMLDivElement>(null);
  const strokeRef = useRef<Stroke['points']>([]);
  const [stroke, setStroke] = useState<Stroke['points']>([]);
  const [size, setSize] = useState({ width: 0, height: 0 });
  const [pointCursor, setPointCursor] = useState<Position>({ x: 0.5, y: 0.5 });
  const [pointFocused, setPointFocused] = useState(false);

  useEffect(() => {
    const area = areaRef.current;
    if (!area) return;
    const observer = new ResizeObserver(() => {
      const style = getComputedStyle(area);
      setSize({
        width: Math.max(1, area.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight)),
        height: Math.max(1, area.clientHeight - parseFloat(style.paddingTop) - parseFloat(style.paddingBottom)),
      });
    });
    observer.observe(area);
    return () => observer.disconnect();
  }, []);
  useEffect(() => { setPointCursor({ x: 0.5, y: 0.5 }); }, [asset.assetId]);
  useEffect(() => { strokeRef.current = []; setStroke([]); }, [asset.assetId, asset.maskRevision, tool]);

  const aspect = asset.width && asset.height ? asset.width / asset.height : 1;
  const height = Math.max(1, Math.min(size.height, size.width / aspect));
  const width = height * aspect;
  const radius = brushRadius * width / Math.max(1, asset.width ?? 1);
  const strokeColor = brushValue ? '#27835d' : '#e94b86';
  function finishStroke(event: PointerEvent<SVGSVGElement>) {
    if (!strokeRef.current.length) return;
    event.currentTarget.releasePointerCapture(event.pointerId);
    const points = strokeRef.current;
    strokeRef.current = [];
    setStroke([]);
    onStroke({ points, radius: brushRadius, value: brushValue });
  }

  return <div className="canvas-area" ref={areaRef}>
    {imageUrl ? <div className={result && asset.background.kind === 'solid' ? 'preview-stage solid-background' : 'preview-stage'}
      style={{ width, height, backgroundColor: result && asset.background.kind === 'solid' ? `rgb(${asset.background.rgb.join(', ')})` : undefined }}>
      <img src={imageUrl} alt={result ? t.result : t.original} onError={onImageError} />
      {tool === 'brush' && result && !disabled && <svg className="stroke-layer" viewBox={`0 0 ${width} ${height}`}
        onPointerDown={event => {
          if (event.button !== 0) return;
          event.currentTarget.setPointerCapture(event.pointerId);
          strokeRef.current = [pointerPoint(event)]; setStroke(strokeRef.current);
        }}
        onPointerMove={event => {
          if (!strokeRef.current.length || strokeRef.current.length >= 4096) return;
          const point = pointerPoint(event);
          const last = strokeRef.current[strokeRef.current.length - 1];
          if (Math.hypot((point.x - last.x) * width, (point.y - last.y) * height) < 2) return;
          strokeRef.current = [...strokeRef.current, point]; setStroke(strokeRef.current);
        }}
        onPointerUp={finishStroke} onPointerCancel={() => { strokeRef.current = []; setStroke([]); }}>
        <polyline points={stroke.map(point => `${point.x * width},${point.y * height}`).join(' ')}
          fill="none" stroke={strokeColor} strokeWidth={radius * 2} strokeLinecap="round" strokeLinejoin="round" />
        {stroke.length === 1 && <circle cx={stroke[0].x * width} cy={stroke[0].y * height} r={radius} fill={strokeColor} />}
      </svg>}
      {tool === 'point' && selectionReady && !disabled && asset.workflow === 'selection' && <svg className="stroke-layer"
        viewBox={`0 0 ${width} ${height}`} role="group" tabIndex={0} aria-label={t.point} aria-describedby="point-keyboard-help"
        onPointerDown={event => { if (event.button === 0) onPoint(pointerPoint(event)); }}
        onFocus={() => setPointFocused(true)} onBlur={() => setPointFocused(false)}
        onKeyDown={event => {
          if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); onPoint(pointCursor); return; }
          const step = event.shiftKey ? 0.1 : 0.01;
          const deltas: Record<string, [number, number]> = { ArrowLeft: [-step, 0], ArrowRight: [step, 0], ArrowUp: [0, -step], ArrowDown: [0, step] };
          const delta = deltas[event.key];
          if (delta) { event.preventDefault(); setPointCursor(point => ({ x: clamp(point.x + delta[0]), y: clamp(point.y + delta[1]) })); }
        }}>
        {pointFocused && <path d={`M${pointCursor.x * width - 8},${pointCursor.y * height}h16M${pointCursor.x * width},${pointCursor.y * height - 8}v16`} stroke="var(--ink)" strokeWidth="2" />}
        {!newSelection && asset.selectionPoints.map((point, index) => <circle key={index} cx={point.x * width} cy={point.y * height}
          r="6" fill={point.label ? '#27835d' : '#e94b86'} stroke="white" strokeWidth="2" />)}
      </svg>}
    </div> : <div className="preview-message">
      {previewError || asset.error ? <><p role="alert">{previewError ?? asset.error}</p>
        <button type="button" disabled={disabled} onClick={onRecover}>{f.retry}</button></> : <p role="status">{t.preview}</p>}
    </div>}
  </div>;
}
