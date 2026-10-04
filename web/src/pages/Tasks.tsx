import { useEffect, useState } from 'react';
import {
  ListChecks,
  Trash2,
  X,
  CheckCircle,
  Loader,
  Circle,
  Lock,
} from 'lucide-react';
import type { TaskItem } from '@/types/api';
import { getTasks, deleteTask } from '@/lib/api';
import { formatDate } from '@/lib/format';

const STATUS_FILTERS = ['all', 'pending', 'in_progress', 'completed'] as const;
type StatusFilter = (typeof STATUS_FILTERS)[number];

function statusIcon(status: string) {
  switch (status) {
    case 'completed':
      return <CheckCircle className="h-4 w-4 text-green-400" />;
    case 'in_progress':
      return <Loader className="h-4 w-4 text-blue-400" />;
    default:
      return <Circle className="h-4 w-4 text-gray-500" />;
  }
}

function statusBadgeClass(status: string): string {
  switch (status) {
    case 'completed':
      return 'bg-green-900/40 text-green-400 border border-green-700/50';
    case 'in_progress':
      return 'bg-blue-900/40 text-blue-400 border border-blue-700/50';
    default:
      return 'bg-gray-800 text-gray-500 border border-gray-700';
  }
}

export default function Tasks() {
  const [tasks, setTasks] = useState<TaskItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState<StatusFilter>('all');
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const [selectedTask, setSelectedTask] = useState<TaskItem | null>(null);

  const fetchTasks = () => {
    setLoading(true);
    getTasks()
      .then(setTasks)
      .catch((err) => setError(err.message))
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    fetchTasks();
  }, []);

  const handleDelete = async (id: string) => {
    try {
      await deleteTask(id);
      setTasks((prev) => prev.filter((t) => t.id !== id));
      setSelectedTask((prev) => (prev?.id === id ? null : prev));
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : 'Failed to delete task');
    } finally {
      setConfirmDelete(null);
    }
  };

  const filteredTasks =
    filter === 'all' ? tasks : tasks.filter((t) => t.status === filter);

  if (error) {
    return (
      <div className="p-6">
        <div className="rounded-lg bg-red-900/30 border border-red-700 p-4 text-red-300">
          Failed to load tasks: {error}
        </div>
      </div>
    );
  }

  if (loading) {
    return (
      <div className="flex items-center justify-center h-64">
        <div className="animate-spin rounded-full h-8 w-8 border-2 border-blue-500 border-t-transparent" />
      </div>
    );
  }

  return (
    <div className="p-6 space-y-6">
      {/* Header */}
      <div className="flex items-center justify-between flex-wrap gap-3">
        <div className="flex items-center gap-2">
          <ListChecks className="h-5 w-5 text-blue-400" />
          <h2 className="text-base font-semibold text-white">
            Tasks ({filteredTasks.length})
          </h2>
        </div>
        <div className="flex items-center gap-1 bg-gray-900 border border-gray-800 rounded-lg p-1">
          {STATUS_FILTERS.map((f) => (
            <button
              key={f}
              onClick={() => setFilter(f)}
              className={`px-3 py-1.5 rounded-md text-xs font-medium capitalize transition-colors ${
                filter === f
                  ? 'bg-blue-600 text-white'
                  : 'text-gray-400 hover:text-white'
              }`}
            >
              {f.replace('_', ' ')}
            </button>
          ))}
        </div>
      </div>

      {/* Tasks Table */}
      {filteredTasks.length === 0 ? (
        <div className="bg-gray-900 rounded-xl border border-gray-800 p-8 text-center">
          <ListChecks className="h-10 w-10 text-gray-600 mx-auto mb-3" />
          <p className="text-gray-400">No tasks found.</p>
        </div>
      ) : (
        <div className="bg-gray-900 rounded-xl border border-gray-800 overflow-x-auto">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-gray-800">
                <th className="text-left px-4 py-3 text-gray-400 font-medium">
                  ID
                </th>
                <th className="text-left px-4 py-3 text-gray-400 font-medium">
                  Subject
                </th>
                <th className="text-left px-4 py-3 text-gray-400 font-medium">
                  Status
                </th>
                <th className="text-left px-4 py-3 text-gray-400 font-medium">
                  Owner
                </th>
                <th className="text-left px-4 py-3 text-gray-400 font-medium">
                  Blocked
                </th>
                <th className="text-left px-4 py-3 text-gray-400 font-medium">
                  Updated
                </th>
                <th className="text-right px-4 py-3 text-gray-400 font-medium">
                  Actions
                </th>
              </tr>
            </thead>
            <tbody>
              {filteredTasks.map((task) => (
                <tr
                  key={task.id}
                  onClick={() => setSelectedTask(task)}
                  className="border-b border-gray-800/50 hover:bg-gray-800/30 transition-colors cursor-pointer"
                >
                  <td className="px-4 py-3 text-gray-400 font-mono text-xs">
                    {task.id}
                  </td>
                  <td className="px-4 py-3 text-white font-medium max-w-xs truncate">
                    {task.subject}
                  </td>
                  <td className="px-4 py-3">
                    <span
                      className={`inline-flex items-center gap-1.5 px-2 py-0.5 rounded-full text-xs font-medium capitalize ${statusBadgeClass(
                        task.status,
                      )}`}
                    >
                      {statusIcon(task.status)}
                      {task.status.replace('_', ' ')}
                    </span>
                  </td>
                  <td className="px-4 py-3 text-gray-300 text-xs">
                    {task.owner ?? '-'}
                  </td>
                  <td className="px-4 py-3">
                    {task.blocked ? (
                      <span className="inline-flex items-center gap-1 text-amber-400 text-xs">
                        <Lock className="h-3.5 w-3.5" />
                        Blocked
                      </span>
                    ) : (
                      <span className="text-gray-600 text-xs">-</span>
                    )}
                  </td>
                  <td className="px-4 py-3 text-gray-400 text-xs">
                    {formatDate(task.updated_at)}
                  </td>
                  <td
                    className="px-4 py-3 text-right"
                    onClick={(e) => e.stopPropagation()}
                  >
                    {confirmDelete === task.id ? (
                      <div className="flex items-center justify-end gap-2">
                        <span className="text-xs text-red-400">Delete?</span>
                        <button
                          onClick={() => handleDelete(task.id)}
                          className="text-red-400 hover:text-red-300 text-xs font-medium"
                        >
                          Yes
                        </button>
                        <button
                          onClick={() => setConfirmDelete(null)}
                          className="text-gray-400 hover:text-white text-xs font-medium"
                        >
                          No
                        </button>
                      </div>
                    ) : (
                      <button
                        onClick={() => setConfirmDelete(task.id)}
                        className="text-gray-400 hover:text-red-400 transition-colors"
                      >
                        <Trash2 className="h-4 w-4" />
                      </button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {/* Detail Modal */}
      {selectedTask && (
        <div
          className="fixed inset-0 bg-black/60 flex items-center justify-center z-50 p-4"
          onClick={() => setSelectedTask(null)}
        >
          <div
            className="bg-gray-900 border border-gray-700 rounded-xl w-full max-w-2xl max-h-[85vh] flex flex-col"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="flex items-center justify-between p-6 pb-4 border-b border-gray-800">
              <div className="flex items-center gap-2 min-w-0">
                <ListChecks className="h-5 w-5 text-blue-400 shrink-0" />
                <h3 className="text-lg font-semibold text-white truncate">
                  {selectedTask.subject}
                </h3>
              </div>
              <button
                onClick={() => setSelectedTask(null)}
                className="text-gray-400 hover:text-white transition-colors shrink-0 ml-3"
              >
                <X className="h-5 w-5" />
              </button>
            </div>

            <div className="overflow-y-auto flex-1 p-6 space-y-5 text-sm">
              <div>
                <div className="text-xs uppercase tracking-wide text-gray-500 mb-1">
                  ID
                </div>
                <div className="text-gray-300 font-mono text-xs break-all">
                  {selectedTask.id}
                </div>
              </div>

              <div>
                <div className="text-xs uppercase tracking-wide text-gray-500 mb-1">
                  Description
                </div>
                <div className="text-gray-200 whitespace-pre-wrap bg-gray-800 border border-gray-700 rounded-md px-3 py-2">
                  {selectedTask.description || '—'}
                </div>
              </div>

              {selectedTask.active_form && (
                <div>
                  <div className="text-xs uppercase tracking-wide text-gray-500 mb-1">
                    Active Form
                  </div>
                  <div className="text-gray-200">{selectedTask.active_form}</div>
                </div>
              )}

              <div className="grid grid-cols-2 gap-4">
                <div>
                  <div className="text-xs uppercase tracking-wide text-gray-500 mb-1">
                    Status
                  </div>
                  <span
                    className={`inline-flex items-center gap-1.5 px-2 py-0.5 rounded-full text-xs font-medium capitalize ${statusBadgeClass(
                      selectedTask.status,
                    )}`}
                  >
                    {statusIcon(selectedTask.status)}
                    {selectedTask.status.replace('_', ' ')}
                  </span>
                </div>
                <div>
                  <div className="text-xs uppercase tracking-wide text-gray-500 mb-1">
                    Owner
                  </div>
                  <div className="text-gray-200">
                    {selectedTask.owner ?? '—'}
                  </div>
                </div>
              </div>

              <div className="grid grid-cols-2 gap-4">
                <div>
                  <div className="text-xs uppercase tracking-wide text-gray-500 mb-1">
                    Blocked By
                  </div>
                  {selectedTask.blocked_by.length > 0 ? (
                    <div className="flex flex-wrap gap-1.5">
                      {selectedTask.blocked_by.map((id) => (
                        <span
                          key={id}
                          className="font-mono text-xs bg-gray-800 border border-gray-700 rounded px-2 py-0.5 text-gray-300"
                        >
                          {id}
                        </span>
                      ))}
                    </div>
                  ) : (
                    <div className="text-gray-500 italic">None</div>
                  )}
                </div>
                <div>
                  <div className="text-xs uppercase tracking-wide text-gray-500 mb-1">
                    Blocks
                  </div>
                  {selectedTask.blocks.length > 0 ? (
                    <div className="flex flex-wrap gap-1.5">
                      {selectedTask.blocks.map((id) => (
                        <span
                          key={id}
                          className="font-mono text-xs bg-gray-800 border border-gray-700 rounded px-2 py-0.5 text-gray-300"
                        >
                          {id}
                        </span>
                      ))}
                    </div>
                  ) : (
                    <div className="text-gray-500 italic">None</div>
                  )}
                </div>
              </div>

              {Object.keys(selectedTask.metadata).length > 0 && (
                <div>
                  <div className="text-xs uppercase tracking-wide text-gray-500 mb-1">
                    Metadata
                  </div>
                  <pre className="text-gray-200 font-mono text-xs bg-gray-800 border border-gray-700 rounded-md px-3 py-2 whitespace-pre-wrap break-words max-h-48 overflow-y-auto">
                    {JSON.stringify(selectedTask.metadata, null, 2)}
                  </pre>
                </div>
              )}

              <div className="grid grid-cols-2 gap-4 pt-2 border-t border-gray-800">
                <div>
                  <div className="text-xs uppercase tracking-wide text-gray-500 mb-1">
                    Created
                  </div>
                  <div className="text-gray-300 text-xs">
                    {formatDate(selectedTask.created_at)}
                  </div>
                </div>
                <div>
                  <div className="text-xs uppercase tracking-wide text-gray-500 mb-1">
                    Updated
                  </div>
                  <div className="text-gray-300 text-xs">
                    {formatDate(selectedTask.updated_at)}
                  </div>
                </div>
              </div>
            </div>

            <div className="flex justify-end gap-3 p-6 pt-4 border-t border-gray-800">
              <button
                onClick={() => setSelectedTask(null)}
                className="px-4 py-2 text-sm font-medium text-white bg-blue-600 hover:bg-blue-700 rounded-lg transition-colors"
              >
                Close
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
