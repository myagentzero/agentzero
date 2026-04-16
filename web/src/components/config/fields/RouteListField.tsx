import { Plus, X } from 'lucide-react';
import type { FieldProps } from '../types';

interface RouteEntry {
  hint: string;
  provider: string;
  model: string;
  max_tokens?: number;
  transport?: string;
}

const TRANSPORT_OPTIONS = [
  { value: '', label: 'Default' },
  { value: 'auto', label: 'Auto' },
  { value: 'websocket', label: 'WebSocket' },
  { value: 'sse', label: 'SSE' },
] as const;

const DEFAULT_ENTRY: RouteEntry = { hint: '', provider: '', model: '' };

export default function RouteListField({ value, onChange }: FieldProps) {
  const routes: RouteEntry[] = Array.isArray(value)
    ? value.map((v) => ({
        hint: v.hint ?? '',
        provider: v.provider ?? '',
        model: v.model ?? '',
        max_tokens: v.max_tokens,
        transport: v.transport,
      }))
    : [];

  const emit = (updated: RouteEntry[]) => {
    onChange(
      updated.map((r) => {
        const entry: Record<string, unknown> = {
          hint: r.hint,
          provider: r.provider,
          model: r.model,
        };
        if (r.max_tokens != null && r.max_tokens > 0) entry.max_tokens = r.max_tokens;
        if (r.transport && r.transport.trim()) entry.transport = r.transport.trim();
        return entry;
      }),
    );
  };

  const update = (index: number, patch: Partial<RouteEntry>) => {
    const next = routes.map((r, i) => (i === index ? { ...r, ...patch } : r));
    emit(next);
  };

  const add = () => emit([...routes, { ...DEFAULT_ENTRY }]);

  const remove = (index: number) => emit(routes.filter((_, i) => i !== index));

  if (routes.length === 0) {
    return (
      <div>
        <p className="text-xs text-gray-500 mb-2">
          No model routes configured. Add a route to map a task hint to a specific model.
        </p>
        <button
          type="button"
          onClick={add}
          className="inline-flex items-center gap-1.5 text-xs text-blue-400 hover:text-blue-300 transition-colors"
        >
          <Plus className="h-3.5 w-3.5" />
          Add Route
        </button>
      </div>
    );
  }

  return (
    <div className="space-y-3">
      {routes.map((entry, i) => (
        <div
          key={i}
          className="relative bg-gray-800/50 border border-gray-700 rounded-lg p-3 space-y-2"
        >
          <button
            type="button"
            onClick={() => remove(i)}
            className="absolute top-2 right-2 text-gray-500 hover:text-red-400 transition-colors"
            aria-label="Remove route"
          >
            <X className="h-3.5 w-3.5" />
          </button>

          {/* Row 1: Hint + Model */}
          <div className="grid grid-cols-2 gap-2 pr-6">
            <div>
              <label className="text-[11px] text-gray-500 mb-0.5 block">Hint</label>
              <input
                type="text"
                value={entry.hint}
                onChange={(e) => update(i, { hint: e.target.value })}
                placeholder="e.g. fast, coding, reasoning"
                className="w-full bg-gray-800 border border-gray-700 rounded-lg px-2.5 py-1.5 text-sm text-white placeholder-gray-500 focus:outline-none focus:ring-2 focus:ring-blue-500"
              />
            </div>
            <div>
              <label className="text-[11px] text-gray-500 mb-0.5 block">Model</label>
              <input
                type="text"
                value={entry.model}
                onChange={(e) => update(i, { model: e.target.value })}
                placeholder="e.g. gpt-4.1-nano, o3"
                className="w-full bg-gray-800 border border-gray-700 rounded-lg px-2.5 py-1.5 text-sm text-white placeholder-gray-500 focus:outline-none focus:ring-2 focus:ring-blue-500"
              />
            </div>
          </div>

          {/* Row 2: Provider */}
          <div>
            <label className="text-[11px] text-gray-500 mb-0.5 block">Provider</label>
            <input
              type="text"
              value={entry.provider}
              onChange={(e) => update(i, { provider: e.target.value })}
              placeholder="e.g. openrouter, custom:https://..."
              className="w-full bg-gray-800 border border-gray-700 rounded-lg px-2.5 py-1.5 text-sm text-white placeholder-gray-500 focus:outline-none focus:ring-2 focus:ring-blue-500"
            />
          </div>

          {/* Row 3: Max Tokens + Transport */}
          <div className="grid grid-cols-2 gap-2">
            <div>
              <label className="text-[11px] text-gray-500 mb-0.5 block">Max Tokens</label>
              <input
                type="number"
                value={entry.max_tokens ?? ''}
                onChange={(e) => {
                  const v = e.target.value ? parseInt(e.target.value, 10) : undefined;
                  update(i, { max_tokens: v });
                }}
                min={1}
                placeholder="Optional"
                className="w-full bg-gray-800 border border-gray-700 rounded-lg px-2.5 py-1.5 text-sm text-white placeholder-gray-500 focus:outline-none focus:ring-2 focus:ring-blue-500"
              />
            </div>
            <div>
              <label className="text-[11px] text-gray-500 mb-0.5 block">Transport</label>
              <select
                value={entry.transport ?? ''}
                onChange={(e) => update(i, { transport: e.target.value || undefined })}
                className="w-full bg-gray-800 border border-gray-700 rounded-lg px-2.5 py-1.5 text-sm text-white focus:outline-none focus:ring-2 focus:ring-blue-500"
              >
                {TRANSPORT_OPTIONS.map((opt) => (
                  <option key={opt.value} value={opt.value}>
                    {opt.label}
                  </option>
                ))}
              </select>
            </div>
          </div>
        </div>
      ))}

      <button
        type="button"
        onClick={add}
        className="inline-flex items-center gap-1.5 text-xs text-blue-400 hover:text-blue-300 transition-colors"
      >
        <Plus className="h-3.5 w-3.5" />
        Add Route
      </button>
    </div>
  );
}
