import { Plus, X } from 'lucide-react';
import type { FieldProps } from '../types';

interface BoardEntry {
  board: string;
  transport: string;
  path?: string;
  baud: number;
}

// Source of truth: src/peripherals/mod.rs (create_peripheral_tools match arms)
const BOARD_OPTIONS = [
  { value: 'nucleo-f401re', label: 'STM32 Nucleo F401RE', defaultTransport: 'serial' },
  { value: 'arduino-uno', label: 'Arduino Uno', defaultTransport: 'serial' },
  { value: 'arduino-uno-q', label: 'Arduino Uno Q', defaultTransport: 'bridge' },
  { value: 'esp32', label: 'ESP32', defaultTransport: 'serial' },
  { value: 'rpi-gpio', label: 'Raspberry Pi GPIO', defaultTransport: 'native' },
] as const;

const TRANSPORT_OPTIONS = [
  { value: 'serial', label: 'Serial (USB)' },
  { value: 'native', label: 'Native GPIO' },
  { value: 'websocket', label: 'WebSocket' },
  { value: 'bridge', label: 'Bridge (Uno Q)' },
] as const;

const DEFAULT_ENTRY: BoardEntry = { board: '', transport: 'serial', baud: 115200 };

function needsPath(transport: string) {
  return transport !== 'native' && transport !== 'bridge';
}

function needsBaud(transport: string) {
  return transport === 'serial';
}

export default function BoardListField({ value, onChange }: FieldProps) {
  const boards: BoardEntry[] = Array.isArray(value)
    ? value.map((v) => ({
        board: v.board ?? '',
        transport: v.transport ?? 'serial',
        path: v.path,
        baud: v.baud ?? 115200,
      }))
    : [];

  const emit = (updated: BoardEntry[]) => {
    onChange(
      updated.map((b) => {
        const entry: Record<string, unknown> = {
          board: b.board,
          transport: b.transport,
          baud: b.baud,
        };
        if (b.path && b.path.trim()) entry.path = b.path.trim();
        return entry;
      }),
    );
  };

  const update = (index: number, patch: Partial<BoardEntry>) => {
    const next = boards.map((b, i) => (i === index ? { ...b, ...patch } : b));
    emit(next);
  };

  const add = () => emit([...boards, { ...DEFAULT_ENTRY }]);

  const remove = (index: number) => emit(boards.filter((_, i) => i !== index));

  const handleBoardChange = (index: number, boardValue: string) => {
    const opt = BOARD_OPTIONS.find((o) => o.value === boardValue);
    update(index, {
      board: boardValue,
      ...(opt ? { transport: opt.defaultTransport } : {}),
    });
  };

  if (boards.length === 0) {
    return (
      <div>
        <p className="text-xs text-gray-500 mb-2">
          No boards configured. Add a board to expose it as an agent tool.
        </p>
        <button
          type="button"
          onClick={add}
          className="inline-flex items-center gap-1.5 text-xs text-blue-400 hover:text-blue-300 transition-colors"
        >
          <Plus className="h-3.5 w-3.5" />
          Add Board
        </button>
      </div>
    );
  }

  return (
    <div className="space-y-3">
      {boards.map((entry, i) => (
        <div
          key={i}
          className="relative bg-gray-800/50 border border-gray-700 rounded-lg p-3 space-y-2"
        >
          <button
            type="button"
            onClick={() => remove(i)}
            className="absolute top-2 right-2 text-gray-500 hover:text-red-400 transition-colors"
            aria-label="Remove board"
          >
            <X className="h-3.5 w-3.5" />
          </button>

          {/* Row 1: Board + Transport */}
          <div className="grid grid-cols-2 gap-2 pr-6">
            <div>
              <label className="text-[11px] text-gray-500 mb-0.5 block">Board</label>
              <select
                value={entry.board}
                onChange={(e) => handleBoardChange(i, e.target.value)}
                className="w-full bg-gray-800 border border-gray-700 rounded-lg px-2.5 py-1.5 text-sm text-white focus:outline-none focus:ring-2 focus:ring-blue-500"
              >
                <option value="">Select board...</option>
                {BOARD_OPTIONS.map((opt) => (
                  <option key={opt.value} value={opt.value}>
                    {opt.label}
                  </option>
                ))}
              </select>
            </div>
            <div>
              <label className="text-[11px] text-gray-500 mb-0.5 block">Transport</label>
              <select
                value={entry.transport}
                onChange={(e) => update(i, { transport: e.target.value })}
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

          {/* Row 2: Path + Baud (conditional) */}
          {(needsPath(entry.transport) || needsBaud(entry.transport)) && (
            <div className="grid grid-cols-2 gap-2">
              {needsPath(entry.transport) && (
                <div>
                  <label className="text-[11px] text-gray-500 mb-0.5 block">Path</label>
                  <input
                    type="text"
                    value={entry.path ?? ''}
                    onChange={(e) => update(i, { path: e.target.value })}
                    placeholder="/dev/ttyACM0"
                    className="w-full bg-gray-800 border border-gray-700 rounded-lg px-2.5 py-1.5 text-sm text-white placeholder-gray-500 focus:outline-none focus:ring-2 focus:ring-blue-500"
                  />
                </div>
              )}
              {needsBaud(entry.transport) && (
                <div>
                  <label className="text-[11px] text-gray-500 mb-0.5 block">Baud Rate</label>
                  <input
                    type="number"
                    value={entry.baud}
                    onChange={(e) => update(i, { baud: parseInt(e.target.value, 10) || 115200 })}
                    min={1}
                    className="w-full bg-gray-800 border border-gray-700 rounded-lg px-2.5 py-1.5 text-sm text-white focus:outline-none focus:ring-2 focus:ring-blue-500"
                  />
                </div>
              )}
            </div>
          )}
        </div>
      ))}

      <button
        type="button"
        onClick={add}
        className="inline-flex items-center gap-1.5 text-xs text-blue-400 hover:text-blue-300 transition-colors"
      >
        <Plus className="h-3.5 w-3.5" />
        Add Board
      </button>
    </div>
  );
}
