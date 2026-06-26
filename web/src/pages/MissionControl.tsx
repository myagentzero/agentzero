import { useState, useEffect, useRef, useCallback } from 'react';
import {
  Activity,
  Pause,
  Play,
  ArrowDown,
  Filter,
  X,
} from 'lucide-react';
import type { SSEEvent } from '@/types/api';
import { SSEClient } from '@/lib/sse';

function normalizeProviderForDisplay(value: unknown): unknown {
  if (typeof value === 'string' && value.toLowerCase().startsWith('custom:')) {
    return 'custom';
  }
  return value;
}

function normalizeEventForDisplay(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map((item) => normalizeEventForDisplay(item));
  }
  if (value !== null && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value as Record<string, unknown>).map(([key, entryValue]) => [
        key,
        key === 'provider'
          ? normalizeProviderForDisplay(entryValue)
          : normalizeEventForDisplay(entryValue),
      ]),
    );
  }
  return value;
}

function formatToolCallResultSummary(event: SSEEvent): string {
  const summary: Record<string, unknown> = {};

  const tool = typeof event.tool === 'string' ? event.tool : event.payload?.tool;
  if (tool) summary.tool = tool;

  const success = event.success ?? event.payload?.success;
  if (success !== undefined) summary.success = success;

  const iteration = event.iteration ?? event.payload?.iteration;
  if (iteration !== undefined && iteration !== null) {
    summary.iteration = iteration;
  }

  const model = typeof event.model === 'string' ? event.model : event.payload?.model;
  if (model) summary.model = model;

  const durationMs = event.duration_ms ?? event.payload?.duration_ms;
  if (durationMs !== undefined && durationMs !== null) {
    summary.duration_ms = durationMs;
  }

  return Object.keys(summary).length > 0 ? JSON.stringify(summary) : '';
}

function getToolCallOutput(event: SSEEvent): string {
  return (
    (typeof event.output === 'string' && event.output) ||
    (typeof event.payload?.output === 'string' && event.payload.output) ||
    (typeof event.message === 'string' && event.message) ||
    ''
  );
}

function isToolResultEvent(type: string): boolean {
  return type === 'tool_call_result' || type === 'tool_result' || type === 'tool_call';
}

function formatToolCallStartDetail(event: SSEEvent): string {
  const summary: Record<string, unknown> = {};

  const tool = typeof event.tool === 'string' ? event.tool : event.payload?.tool;
  if (tool) summary.tool = tool;

  const iteration = event.iteration ?? event.payload?.iteration;
  if (iteration !== undefined && iteration !== null) {
    summary.iteration = iteration;
  }

  const model = typeof event.model === 'string' ? event.model : event.payload?.model;
  if (model) summary.model = model;

  const channel = typeof event.channel === 'string' ? event.channel : event.payload?.channel;
  if (channel) summary.channel = channel;

  return Object.keys(summary).length > 0 ? JSON.stringify(summary) : '';
}

function formatTimestamp(ts?: string): string {
  if (!ts) return new Date().toLocaleTimeString();
  return new Date(ts).toLocaleTimeString();
}

function eventTypeBadgeColor(type: string): string {
  switch (type.toLowerCase()) {
    case 'error':
      return 'bg-red-900/50 text-red-400 border-red-700/50';
    case 'warn':
    case 'warning':
      return 'bg-yellow-900/50 text-yellow-400 border-yellow-700/50';
    case 'tool_call':
    case 'tool_result':
    case 'tool_call_start':
    case 'tool_call_result':
      return 'bg-purple-900/50 text-purple-400 border-purple-700/50';
    case 'message':
    case 'chat':
      return 'bg-blue-900/50 text-blue-400 border-blue-700/50';
    case 'health':
    case 'status':
    case 'connected':
    case 'heartbeat_tick':
      return 'bg-green-900/50 text-green-400 border-green-700/50';
    case 'llm_response':
      return 'bg-cyan-900/50 text-cyan-400 border-cyan-700/50';
    case 'llm_request':
      return 'bg-indigo-900/50 text-indigo-400 border-indigo-700/50';
    case 'agent_start':
    case 'agent_end':
      return 'bg-amber-900/50 text-amber-400 border-amber-700/50';
    case 'turn_complete':
      return 'bg-lime-900/50 text-lime-400 border-lime-700/50';
    case 'channel_message':
      return 'bg-rose-900/50 text-rose-400 border-rose-700/50';
    case 'webhook_auth_failure':
      return 'bg-orange-900/50 text-orange-400 border-orange-700/50';
    default:
      return 'bg-gray-800 text-gray-400 border-gray-700';
  }
}

interface LogEntry {
  id: string;
  event: SSEEvent;
}

export default function MissionControl() {
  const [entries, setEntries] = useState<LogEntry[]>([]);
  const [paused, setPaused] = useState(false);
  const [connected, setConnected] = useState(false);
  const [autoScroll, setAutoScroll] = useState(true);
  const [typeFilters, setTypeFilters] = useState<Set<string>>(new Set());
  const [selectedEntry, setSelectedEntry] = useState<LogEntry | null>(null);

  const containerRef = useRef<HTMLDivElement>(null);
  const sseRef = useRef<SSEClient | null>(null);
  const pausedRef = useRef(false);
  const entryIdRef = useRef(0);

  // Keep pausedRef in sync
  useEffect(() => {
    pausedRef.current = paused;
  }, [paused]);

  useEffect(() => {
    const client = new SSEClient();

    client.onConnect = () => {
      setConnected(true);
    };

    client.onError = () => {
      setConnected(false);
    };

    client.onEvent = (event: SSEEvent) => {
      if (pausedRef.current) return;
      entryIdRef.current += 1;
      const entry: LogEntry = {
        id: `log-${entryIdRef.current}`,
        event,
      };
      setEntries((prev) => {
        // Cap at 500 entries for performance
        const next = [...prev, entry];
        return next.length > 500 ? next.slice(-500) : next;
      });
    };

    client.connect();
    sseRef.current = client;

    return () => {
      client.disconnect();
    };
  }, []);

  // Auto-scroll to bottom
  useEffect(() => {
    if (autoScroll && containerRef.current) {
      containerRef.current.scrollTop = containerRef.current.scrollHeight;
    }
  }, [entries, autoScroll]);

  // Detect user scroll to toggle auto-scroll
  const handleScroll = useCallback(() => {
    if (!containerRef.current) return;
    const { scrollTop, scrollHeight, clientHeight } = containerRef.current;
    const isAtBottom = scrollHeight - scrollTop - clientHeight < 50;
    setAutoScroll(isAtBottom);
  }, []);

  const jumpToBottom = () => {
    if (containerRef.current) {
      containerRef.current.scrollTop = containerRef.current.scrollHeight;
    }
    setAutoScroll(true);
  };

  // Collect all event types for filter checkboxes
  const allTypes = Array.from(new Set(entries.map((e) => e.event.type))).sort();

  const toggleTypeFilter = (type: string) => {
    setTypeFilters((prev) => {
      const next = new Set(prev);
      if (next.has(type)) {
        next.delete(type);
      } else {
        next.add(type);
      }
      return next;
    });
  };

  const filteredEntries =
    typeFilters.size === 0
      ? entries
      : entries.filter((e) => typeFilters.has(e.event.type));

  return (
    <div className="flex min-h-[28rem] flex-col h-[calc(100dvh-8.5rem)]">
      {/* Toolbar */}
      <div className="flex items-center justify-between px-6 py-3 border-b border-gray-800 bg-gray-900">
        <div className="flex items-center gap-3">
          <Activity className="h-5 w-5 text-blue-400" />
          <h2 className="text-base font-semibold text-white">Mission Control</h2>
          <div className="flex items-center gap-2 ml-2">
            <span
              className={`inline-block h-2 w-2 rounded-full ${
                connected ? 'bg-green-500' : 'bg-red-500'
              }`}
            />
            <span className="text-xs text-gray-500">
              {connected ? 'Connected' : 'Disconnected'}
            </span>
          </div>
          <span className="text-xs text-gray-500 ml-2">
            {filteredEntries.length} events
          </span>
        </div>

        <div className="flex items-center gap-2">
          {/* Pause/Resume */}
          <button
            onClick={() => setPaused(!paused)}
            className={`flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-sm font-medium transition-colors ${
              paused
                ? 'bg-green-600 hover:bg-green-700 text-white'
                : 'bg-yellow-600 hover:bg-yellow-700 text-white'
            }`}
          >
            {paused ? (
              <>
                <Play className="h-3.5 w-3.5" /> Resume
              </>
            ) : (
              <>
                <Pause className="h-3.5 w-3.5" /> Pause
              </>
            )}
          </button>

          {/* Jump to Bottom */}
          {!autoScroll && (
            <button
              onClick={jumpToBottom}
              className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-sm font-medium bg-blue-600 hover:bg-blue-700 text-white transition-colors"
            >
              <ArrowDown className="h-3.5 w-3.5" />
              Jump to bottom
            </button>
          )}
        </div>
      </div>

      {/* Event type filters */}
      {allTypes.length > 0 && (
        <div className="flex items-center gap-2 px-6 py-2 border-b border-gray-800 bg-gray-900/80 overflow-x-auto">
          <Filter className="h-4 w-4 text-gray-500 flex-shrink-0" />
          <span className="text-xs text-gray-500 flex-shrink-0">Filter:</span>
          {allTypes.map((type) => (
            <label
              key={type}
              className="flex items-center gap-1.5 cursor-pointer flex-shrink-0"
            >
              <input
                type="checkbox"
                checked={typeFilters.has(type)}
                onChange={() => toggleTypeFilter(type)}
                className="rounded bg-gray-800 border-gray-600 text-blue-500 focus:ring-blue-500 focus:ring-offset-0 h-3.5 w-3.5"
              />
              <span className="text-xs text-gray-400 capitalize">{type}</span>
            </label>
          ))}
          {typeFilters.size > 0 && (
            <button
              onClick={() => setTypeFilters(new Set())}
              className="text-xs text-blue-400 hover:text-blue-300 flex-shrink-0 ml-1"
            >
              Clear
            </button>
          )}
        </div>
      )}

      {/* Log entries */}
      <div
        ref={containerRef}
        onScroll={handleScroll}
        className="flex-1 overflow-y-auto p-4 space-y-2"
      >
        {filteredEntries.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-full text-gray-500">
            <Activity className="h-10 w-10 text-gray-600 mb-3" />
            <p className="text-sm">
              {paused
                ? 'Log streaming is paused.'
                : 'Waiting for events...'}
            </p>
          </div>
        ) : (
          filteredEntries.map((entry) => {
            const { event } = entry;
            let detail: string;

            // Fields to exclude from the list display (verbose/redundant data)
            const verboseFields = new Set([
              'type',
              'timestamp',
              'raw_response',
              'arguments',
              'output',
              'error_traceback',
              'full_response',
            ]);

            // Helper to recursively filter out verbose fields
            const cleanData = (obj: any): any => {
              if (typeof obj !== 'object' || obj === null) return obj;
              if (Array.isArray(obj)) return obj.map(cleanData);
              return Object.fromEntries(
                Object.entries(obj)
                  .filter(([k]) => !verboseFields.has(k))
                  .map(([k, v]) => [
                    k,
                    k === 'provider'
                      ? normalizeProviderForDisplay(cleanData(v))
                      : cleanData(v),
                  ])
              );
            };

            if (event.type === 'turn_complete') {
              detail = 'Agent completed turn';
            } else if (event.type === 'channel_message') {
              detail = `${event.direction === 'inbound' ? 'Received' : 'Sent'} on ${event.channel}`;
            } else if (event.type === 'webhook_auth_failure') {
              detail = `Auth failure on ${event.channel} (signature: ${event.signature}, bearer: ${event.bearer})`;
            } else if (event.type === 'heartbeat_tick') {
              detail = 'Runtime heartbeat';
            } else if (event.type === 'tool_call_start') {
              detail = formatToolCallStartDetail(event) || JSON.stringify(cleanData(event));
            } else if (event.type === 'tool_call_result' || event.type === 'tool_result') {
              detail = formatToolCallResultSummary(event);
            } else if (event.type === 'tool_call') {
              detail = formatToolCallResultSummary(event);
            } else {
              detail =
                event.message ??
                event.content ??
                event.data ??
                (JSON.stringify(cleanData(event)) || '');
            }

            return (
              <div
                key={entry.id}
                onClick={() => setSelectedEntry(entry)}
                className="bg-gray-900 border border-gray-800 rounded-lg p-3 hover:border-gray-600 hover:bg-gray-800/50 transition-colors cursor-pointer"
              >
                <div className="flex items-start gap-3">
                  <span className="text-xs text-gray-500 font-mono whitespace-nowrap mt-0.5">
                    {formatTimestamp(event.timestamp)}
                  </span>
                  <span
                    className={`inline-flex items-center px-2 py-0.5 rounded text-xs font-medium border capitalize flex-shrink-0 ${eventTypeBadgeColor(
                      event.type,
                    )}`}
                  >
                    {event.type}
                  </span>
                  <p className="text-sm text-gray-300 break-all min-w-0">
                    {typeof detail === 'string' ? detail : JSON.stringify(detail)}
                  </p>
                </div>
              </div>
            );
          })
        )}
      </div>

      {/* Detail Modal */}
      {selectedEntry && (() => {
        const { event } = selectedEntry;
        const metadata: Record<string, unknown> = {};
        const payload: Record<string, unknown> = {};
        const toolOutput = isToolResultEvent(event.type) ? getToolCallOutput(event) : '';

        // Separate metadata from payload
        for (const [key, value] of Object.entries(event)) {
          if (key === 'type' || key === 'timestamp') {
            metadata[key] = value;
          } else if (key === 'payload' && typeof value === 'object' && value !== null) {
            // If there's a payload object, merge its contents
            Object.assign(payload, value as Record<string, unknown>);
          } else {
            payload[key] = value;
          }
        }

        if (toolOutput) {
          delete payload.output;
        }

        const payloadEntries = Object.entries(
          normalizeEventForDisplay(payload) as Record<string, unknown>,
        ).filter(([key]) => key !== 'output');

        return (
          <div
            className="fixed inset-0 bg-black/50 flex items-center justify-center p-4 z-50"
            onClick={() => setSelectedEntry(null)}
          >
            <div
              className="bg-gray-900 border border-gray-700 rounded-lg p-6 max-w-4xl w-full max-h-[90vh] overflow-auto"
              onClick={(e) => e.stopPropagation()}
            >
              <div className="flex items-center justify-between mb-6">
                <h3 className="text-lg font-semibold text-white">Event Details</h3>
                <button
                  onClick={() => setSelectedEntry(null)}
                  className="text-gray-400 hover:text-gray-300 transition-colors"
                >
                  <X className="h-5 w-5" />
                </button>
              </div>

              <div className="space-y-6">
                {/* Type and Timestamp */}
                <div className="grid grid-cols-2 gap-4">
                  <div>
                    <p className="text-xs text-gray-500 uppercase tracking-wide mb-1 font-semibold">Type</p>
                    <p
                      className={`inline-flex items-center px-3 py-1.5 rounded text-sm font-medium border capitalize ${eventTypeBadgeColor(
                        event.type,
                      )}`}
                    >
                      {event.type}
                    </p>
                  </div>
                  <div>
                    <p className="text-xs text-gray-500 uppercase tracking-wide mb-1 font-semibold">Timestamp</p>
                    <p className="text-sm font-mono text-gray-300">
                      {formatTimestamp(event.timestamp)}
                    </p>
                  </div>
                </div>

                {toolOutput && (
                  <div>
                    <p className="text-xs text-gray-500 uppercase tracking-wide mb-2 font-semibold">Output</p>
                    <pre className="bg-gray-800 border border-gray-700 rounded p-4 text-xs text-gray-300 overflow-auto max-h-64 w-full whitespace-pre-wrap break-all">
                      {toolOutput}
                    </pre>
                  </div>
                )}

                {/* Payload Fields (if any) */}
                {payloadEntries.length > 0 && (
                  <div>
                    <p className="text-xs text-gray-500 uppercase tracking-wide mb-3 font-semibold">Payload Data</p>
                    <div className="bg-gray-800 border border-gray-700 rounded p-4 space-y-4">
                      {payloadEntries.map(([key, value]) => (
                        <div key={key}>
                          <p className="text-xs text-gray-600 mb-2 capitalize font-medium">{key}</p>
                          {typeof value === 'object' && value !== null ? (
                            <pre className="bg-gray-900 border border-gray-600 rounded p-3 text-xs text-gray-300 overflow-auto max-h-64">
                              {JSON.stringify(value, null, 2)}
                            </pre>
                          ) : (
                            <p className="text-sm text-gray-300 font-mono bg-gray-900 border border-gray-600 rounded px-3 py-2 inline-block">
                              {typeof value === 'boolean'
                                ? (value ? 'true' : 'false')
                                : typeof value === 'number'
                                  ? value
                                  : `"${String(key === 'provider' ? normalizeProviderForDisplay(value) : value)}"`}
                            </p>
                          )}
                        </div>
                      ))}
                    </div>
                  </div>
                )}

                {/* Full Event JSON */}
                <div>
                  <p className="text-xs text-gray-500 uppercase tracking-wide mb-2 font-semibold">Complete Event (JSON)</p>
                  <pre className="bg-gray-800 border border-gray-700 rounded p-4 text-xs text-gray-300 overflow-auto max-h-64 w-full">
                    {JSON.stringify(normalizeEventForDisplay(event), null, 2)}
                  </pre>
                </div>
              </div>
            </div>
          </div>
        );
      })()}
    </div>
  );
}
