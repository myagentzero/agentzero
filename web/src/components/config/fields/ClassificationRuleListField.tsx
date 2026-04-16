import { Plus, X } from 'lucide-react';
import type { FieldProps } from '../types';

interface RuleEntry {
  hint: string;
  keywords: string[];
  patterns: string[];
  min_length?: number;
  max_length?: number;
  priority: number;
}

const DEFAULT_ENTRY: RuleEntry = {
  hint: '',
  keywords: [],
  patterns: [],
  priority: 50,
};

function toCSV(arr: string[]): string {
  return arr.join(', ');
}

function fromCSV(str: string): string[] {
  return str
    .split(',')
    .map((s) => s.trim())
    .filter(Boolean);
}

export default function ClassificationRuleListField({ value, onChange }: FieldProps) {
  const rules: RuleEntry[] = Array.isArray(value)
    ? value.map((v) => ({
        hint: v.hint ?? '',
        keywords: Array.isArray(v.keywords) ? v.keywords : [],
        patterns: Array.isArray(v.patterns) ? v.patterns : [],
        min_length: v.min_length,
        max_length: v.max_length,
        priority: v.priority ?? 50,
      }))
    : [];

  const emit = (updated: RuleEntry[]) => {
    onChange(
      updated.map((r) => {
        const entry: Record<string, unknown> = {
          hint: r.hint,
          priority: r.priority,
        };
        if (r.keywords.length > 0) entry.keywords = r.keywords;
        if (r.patterns.length > 0) entry.patterns = r.patterns;
        if (r.min_length != null && r.min_length > 0) entry.min_length = r.min_length;
        if (r.max_length != null && r.max_length > 0) entry.max_length = r.max_length;
        return entry;
      }),
    );
  };

  const update = (index: number, patch: Partial<RuleEntry>) => {
    const next = rules.map((r, i) => (i === index ? { ...r, ...patch } : r));
    emit(next);
  };

  const add = () => emit([...rules, { ...DEFAULT_ENTRY }]);

  const remove = (index: number) => emit(rules.filter((_, i) => i !== index));

  if (rules.length === 0) {
    return (
      <div>
        <p className="text-xs text-gray-500 mb-2">
          No classification rules configured. Add a rule to auto-route messages by content.
        </p>
        <button
          type="button"
          onClick={add}
          className="inline-flex items-center gap-1.5 text-xs text-blue-400 hover:text-blue-300 transition-colors"
        >
          <Plus className="h-3.5 w-3.5" />
          Add Rule
        </button>
      </div>
    );
  }

  return (
    <div className="space-y-3">
      {rules.map((entry, i) => (
        <div
          key={i}
          className="relative bg-gray-800/50 border border-gray-700 rounded-lg p-3 space-y-2"
        >
          <button
            type="button"
            onClick={() => remove(i)}
            className="absolute top-2 right-2 text-gray-500 hover:text-red-400 transition-colors"
            aria-label="Remove rule"
          >
            <X className="h-3.5 w-3.5" />
          </button>

          {/* Row 1: Hint + Priority */}
          <div className="grid grid-cols-2 gap-2 pr-6">
            <div>
              <label className="text-[11px] text-gray-500 mb-0.5 block">Hint</label>
              <input
                type="text"
                value={entry.hint}
                onChange={(e) => update(i, { hint: e.target.value })}
                placeholder="e.g. coding, reasoning, fast"
                className="w-full bg-gray-800 border border-gray-700 rounded-lg px-2.5 py-1.5 text-sm text-white placeholder-gray-500 focus:outline-none focus:ring-2 focus:ring-blue-500"
              />
            </div>
            <div>
              <label className="text-[11px] text-gray-500 mb-0.5 block">Priority</label>
              <input
                type="number"
                value={entry.priority}
                onChange={(e) => update(i, { priority: parseInt(e.target.value, 10) || 0 })}
                className="w-full bg-gray-800 border border-gray-700 rounded-lg px-2.5 py-1.5 text-sm text-white focus:outline-none focus:ring-2 focus:ring-blue-500"
              />
            </div>
          </div>

          {/* Row 2: Keywords */}
          <div>
            <label className="text-[11px] text-gray-500 mb-0.5 block">
              Keywords <span className="text-gray-600">(comma-separated, case-insensitive)</span>
            </label>
            <input
              type="text"
              value={toCSV(entry.keywords)}
              onChange={(e) => update(i, { keywords: fromCSV(e.target.value) })}
              placeholder="e.g. refactor, debug, compile"
              className="w-full bg-gray-800 border border-gray-700 rounded-lg px-2.5 py-1.5 text-sm text-white placeholder-gray-500 focus:outline-none focus:ring-2 focus:ring-blue-500"
            />
          </div>

          {/* Row 3: Patterns */}
          <div>
            <label className="text-[11px] text-gray-500 mb-0.5 block">
              Patterns <span className="text-gray-600">(comma-separated, case-sensitive)</span>
            </label>
            <input
              type="text"
              value={toCSV(entry.patterns)}
              onChange={(e) => update(i, { patterns: fromCSV(e.target.value) })}
              placeholder='e.g. ```, fn , def , class '
              className="w-full bg-gray-800 border border-gray-700 rounded-lg px-2.5 py-1.5 text-sm text-white placeholder-gray-500 focus:outline-none focus:ring-2 focus:ring-blue-500"
            />
          </div>

          {/* Row 4: Min/Max Length */}
          <div className="grid grid-cols-2 gap-2">
            <div>
              <label className="text-[11px] text-gray-500 mb-0.5 block">Min Length (chars)</label>
              <input
                type="number"
                value={entry.min_length ?? ''}
                onChange={(e) => {
                  const v = e.target.value ? parseInt(e.target.value, 10) : undefined;
                  update(i, { min_length: v });
                }}
                min={0}
                placeholder="Optional"
                className="w-full bg-gray-800 border border-gray-700 rounded-lg px-2.5 py-1.5 text-sm text-white placeholder-gray-500 focus:outline-none focus:ring-2 focus:ring-blue-500"
              />
            </div>
            <div>
              <label className="text-[11px] text-gray-500 mb-0.5 block">Max Length (chars)</label>
              <input
                type="number"
                value={entry.max_length ?? ''}
                onChange={(e) => {
                  const v = e.target.value ? parseInt(e.target.value, 10) : undefined;
                  update(i, { max_length: v });
                }}
                min={0}
                placeholder="Optional"
                className="w-full bg-gray-800 border border-gray-700 rounded-lg px-2.5 py-1.5 text-sm text-white placeholder-gray-500 focus:outline-none focus:ring-2 focus:ring-blue-500"
              />
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
        Add Rule
      </button>
    </div>
  );
}
