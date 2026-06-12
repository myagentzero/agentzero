import { useState, useEffect } from 'react';
import {
  Search,
  ChevronDown,
  ChevronRight,
  Wrench,
  Tag,
  User,
  FolderOpen,
  BookOpen,
  Activity,
  Clock,
} from 'lucide-react';
import type { SkillSummary } from '@/types/api';
import { getSkills } from '@/lib/api';

function formatLastCalled(iso: string): string {
  const date = new Date(iso);
  const now = new Date();
  const diffMs = now.getTime() - date.getTime();
  const diffDays = Math.floor(diffMs / (1000 * 60 * 60 * 24));
  if (diffDays === 0) return 'today';
  if (diffDays === 1) return 'yesterday';
  if (diffDays < 30) return `${diffDays}d ago`;
  const diffMonths = Math.floor(diffDays / 30);
  if (diffMonths < 12) return `${diffMonths}mo ago`;
  return `${Math.floor(diffMonths / 12)}y ago`;
}

export default function Skills() {
  const [skills, setSkills] = useState<SkillSummary[]>([]);
  const [search, setSearch] = useState('');
  const [expandedSkill, setExpandedSkill] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    getSkills()
      .then(setSkills)
      .catch((err) => setError(err.message))
      .finally(() => setLoading(false));
  }, []);

  const filtered = skills.filter(
    (s) =>
      s.name.toLowerCase().includes(search.toLowerCase()) ||
      s.description.toLowerCase().includes(search.toLowerCase()) ||
      s.tags.some((tag) => tag.toLowerCase().includes(search.toLowerCase())),
  );

  if (error) {
    return (
      <div className="p-6">
        <div className="rounded-lg bg-red-900/30 border border-red-700 p-4 text-red-300">
          Failed to load skills: {error}
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
      {/* Search */}
      <div className="relative max-w-md">
        <Search className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-gray-500" />
        <input
          type="text"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder="Search skills..."
          className="w-full bg-gray-900 border border-gray-700 rounded-lg pl-10 pr-4 py-2.5 text-sm text-white placeholder-gray-500 focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
        />
      </div>

      {/* Skills Grid */}
      <div>
        <div className="flex items-center gap-2 mb-4">
          <BookOpen className="h-5 w-5 text-purple-400" />
          <h2 className="text-base font-semibold text-white">
            Installed Skills ({filtered.length})
          </h2>
        </div>

        {filtered.length === 0 ? (
          <p className="text-sm text-gray-500">
            {skills.length === 0
              ? 'No skills installed.'
              : 'No skills match your search.'}
          </p>
        ) : (
          <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-4">
            {filtered.map((skill) => {
              const isExpanded = expandedSkill === skill.name;
              return (
                <div
                  key={skill.name}
                  className="bg-gray-900 rounded-xl border border-gray-800 overflow-hidden"
                >
                  <button
                    onClick={() =>
                      setExpandedSkill(isExpanded ? null : skill.name)
                    }
                    className="w-full text-left p-4 hover:bg-gray-800/50 transition-colors"
                  >
                    <div className="flex items-start justify-between gap-2">
                      <div className="flex items-center gap-2 min-w-0">
                        <BookOpen className="h-4 w-4 text-purple-400 flex-shrink-0 mt-0.5" />
                        <h3 className="text-sm font-semibold text-white truncate">
                          {skill.name}
                        </h3>
                      </div>
                      <div className="flex items-center gap-2 flex-shrink-0">
                        <span className="text-xs text-gray-500">
                          v{skill.version}
                        </span>
                        {isExpanded ? (
                          <ChevronDown className="h-4 w-4 text-gray-400" />
                        ) : (
                          <ChevronRight className="h-4 w-4 text-gray-400" />
                        )}
                      </div>
                    </div>
                    <p className="text-sm text-gray-400 mt-2 line-clamp-2">
                      {skill.description}
                    </p>
                    {skill.usage ? (
                      <div className="flex items-center gap-3 mt-2">
                        <span className="inline-flex items-center gap-1 text-xs text-blue-400">
                          <Activity className="h-3 w-3" />
                          {skill.usage.call_count === 1
                            ? '1 call'
                            : `${skill.usage.call_count} calls`}
                        </span>
                        <span className="inline-flex items-center gap-1 text-xs text-gray-500">
                          <Clock className="h-3 w-3" />
                          {formatLastCalled(skill.usage.last_called)}
                        </span>
                      </div>
                    ) : (
                      <div className="mt-2">
                        <span className="text-xs text-gray-600">never used</span>
                      </div>
                    )}
                    {skill.tags.length > 0 && (
                      <div className="flex flex-wrap gap-1.5 mt-2">
                        {skill.tags.map((tag) => (
                          <span
                            key={tag}
                            className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-xs font-medium bg-purple-900/30 text-purple-300 border border-purple-800/50"
                          >
                            <Tag className="h-3 w-3" />
                            {tag}
                          </span>
                        ))}
                      </div>
                    )}
                  </button>

                  {isExpanded && (
                    <div className="border-t border-gray-800 p-4 space-y-3">
                      {skill.author && (
                        <div className="flex items-center gap-2 text-xs text-gray-400">
                          <User className="h-3.5 w-3.5" />
                          <span>{skill.author}</span>
                        </div>
                      )}

                      {skill.location && (
                        <div className="flex items-center gap-2 text-xs text-gray-400">
                          <FolderOpen className="h-3.5 w-3.5" />
                          <span className="font-mono truncate">
                            {skill.location}
                          </span>
                        </div>
                      )}

                      {skill.tools.length > 0 && (
                        <div>
                          <p className="text-xs text-gray-500 mb-2 font-medium uppercase tracking-wider">
                            Tools ({skill.tools.length})
                          </p>
                          <div className="space-y-2">
                            {skill.tools.map((tool) => (
                              <div
                                key={tool.name}
                                className="flex items-start gap-2 bg-gray-950 rounded-lg p-2.5"
                              >
                                <Wrench className="h-3.5 w-3.5 text-blue-400 flex-shrink-0 mt-0.5" />
                                <div className="min-w-0">
                                  <div className="flex items-center gap-2">
                                    <span className="text-xs font-medium text-white">
                                      {tool.name}
                                    </span>
                                    <span className="text-[10px] px-1.5 py-0.5 rounded bg-gray-800 text-gray-400">
                                      {tool.kind}
                                    </span>
                                  </div>
                                  <p className="text-xs text-gray-500 mt-0.5 line-clamp-1">
                                    {tool.description}
                                  </p>
                                </div>
                              </div>
                            ))}
                          </div>
                        </div>
                      )}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
