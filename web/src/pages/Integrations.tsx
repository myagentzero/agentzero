import { useState, useEffect } from 'react';
import { Puzzle, Check, Zap, Clock } from 'lucide-react';
import type {
  Integration,
  IntegrationCredentialsField,
  IntegrationSettingsEntry,
  StatusResponse,
} from '@/types/api';
import {
  getIntegrations,
  getIntegrationSettings,
  getStatus,
  putIntegrationCredentials,
} from '@/lib/api';

function statusBadge(status: Integration['status']) {
  switch (status) {
    case 'Active':
      return {
        icon: Check,
        label: 'Active',
        classes: 'bg-green-900/40 text-green-400 border-green-700/50',
      };
    case 'Available':
      return {
        icon: Zap,
        label: 'Available',
        classes: 'bg-blue-900/40 text-blue-400 border-blue-700/50',
      };
    case 'ComingSoon':
      return {
        icon: Clock,
        label: 'Coming Soon',
        classes: 'bg-gray-800 text-gray-400 border-gray-700',
      };
  }
}

function formatCategory(category: string): string {
  if (!category) return category;
  return category
    .replace(/([a-z])([A-Z])/g, '$1 $2')
    .replace(/Ai/g, 'AI');
}

const FALLBACK_MODEL_OPTIONS: Record<string, string[]> = {
  openrouter: ['anthropic/claude-sonnet-4-6', 'openai/gpt-5.2', 'google/gemini-3.1-pro'],
  anthropic: ['claude-sonnet-4-6', 'claude-opus-4-6'],
  openai: ['gpt-5.2', 'gpt-5.2-codex', 'gpt-4o'],
  google: ['google/gemini-3.1-pro', 'google/gemini-3-flash', 'google/gemini-2.5-pro'],
  deepseek: ['deepseek/deepseek-reasoner', 'deepseek/deepseek-chat'],
  xai: ['x-ai/grok-4', 'x-ai/grok-3'],
  mistral: ['mistral-large-latest', 'codestral-latest', 'mistral-small-latest'],
  perplexity: ['sonar-pro', 'sonar-reasoning-pro', 'sonar'],
  bedrock: ['anthropic.claude-sonnet-4-5-20250929-v1:0', 'anthropic.claude-opus-4-6-v1:0'],
  groq: ['llama-3.3-70b-versatile', 'mixtral-8x7b-32768'],
  together: [
    'meta-llama/Llama-3.3-70B-Instruct-Turbo',
    'Qwen/Qwen2.5-72B-Instruct-Turbo',
    'deepseek-ai/DeepSeek-R1-Distill-Llama-70B',
  ],
  cohere: ['command-r-plus-08-2024', 'command-r-08-2024'],
};

function modelOptionsForField(
  integrationId: string,
  field: IntegrationCredentialsField,
): string[] {
  if (field.key !== 'default_model') return field.options ?? [];
  if (field.options?.length) return field.options;
  return FALLBACK_MODEL_OPTIONS[integrationId] ?? [];
}

export default function Integrations() {
  const [integrations, setIntegrations] = useState<Integration[]>([]);
  const [settingsByName, setSettingsByName] = useState<
    Record<string, IntegrationSettingsEntry>
  >({});
  const [settingsRevision, setSettingsRevision] = useState('');
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [activeCategory, setActiveCategory] = useState<string>('all');
  const [saveSuccess, setSaveSuccess] = useState<string | null>(null);
  const [runtimeStatus, setRuntimeStatus] = useState<Pick<StatusResponse, 'model' | 'provider'> | null>(null);
  const [activeAiIntegrationId, setActiveAiIntegrationId] = useState<string | null>(null);
  const [quickModelDrafts, setQuickModelDrafts] = useState<Record<string, string>>({});
  const [quickModelSavingId, setQuickModelSavingId] = useState<string | null>(null);
  const [quickModelError, setQuickModelError] = useState<string | null>(null);

  const modelFieldFor = (integration: IntegrationSettingsEntry) =>
    integration.fields.find((field) => field.key === 'default_model');

  const fallbackModelFor = (integration: IntegrationSettingsEntry): string | null => {
    const modelField = modelFieldFor(integration);
    if (!modelField) return null;
    return modelOptionsForField(integration.id, modelField)[0] ?? null;
  };

  const modelValueFor = (
    integration: IntegrationSettingsEntry,
    isActiveDefaultProvider: boolean,
  ): string | null => {
    if (isActiveDefaultProvider && runtimeStatus?.model?.trim()) {
      return runtimeStatus.model.trim();
    }

    const fieldModel = modelFieldFor(integration)?.current_value?.trim();
    if (fieldModel) {
      return fieldModel;
    }

    return null;
  };

  const activeAiIntegration = Object.values(settingsByName).find(
    (integration) => integration.id === activeAiIntegrationId,
  );

  const loadData = async (
    showLoadingState = true,
  ): Promise<Record<string, IntegrationSettingsEntry> | null> => {
    if (showLoadingState) {
      setLoading(true);
    }
    setError(null);
    try {
      const [integrationList, settings, status] = await Promise.all([
        getIntegrations(),
        getIntegrationSettings(),
        getStatus().catch(() => null),
      ]);

      const nextSettingsByName = settings.integrations.reduce<
        Record<string, IntegrationSettingsEntry>
      >((acc, item) => {
        acc[item.name] = item;
        return acc;
      }, {});

      setIntegrations(integrationList);
      setSettingsRevision(settings.revision);
      setSettingsByName(nextSettingsByName);
      setActiveAiIntegrationId(settings.active_default_provider_integration_id ?? null);
      setRuntimeStatus(status ? { model: status.model, provider: status.provider } : null);
      return nextSettingsByName;
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : 'Failed to load integrations');
      setActiveAiIntegrationId(null);
      setRuntimeStatus(null);
      return null;
    } finally {
      if (showLoadingState) {
        setLoading(false);
      }
    }
  };

  useEffect(() => {
    void loadData();
  }, []);

  useEffect(() => {
    if (!saveSuccess) return;
    const timer = setTimeout(() => setSaveSuccess(null), 4000);
    return () => clearTimeout(timer);
  }, [saveSuccess]);

  const saveQuickModel = async (
    integration: IntegrationSettingsEntry,
    targetModel: string,
    currentModel: string,
    isActiveDefaultProvider: boolean,
  ) => {
    const trimmedTarget = targetModel.trim();
    if (!trimmedTarget || trimmedTarget === currentModel) {
      return;
    }

    if (
      activeAiIntegrationId &&
      !isActiveDefaultProvider &&
      integration.id !== activeAiIntegrationId
    ) {
      const currentProvider = activeAiIntegration?.name ?? 'current provider';
      const confirmed = window.confirm(
        `Switch default AI provider from ${currentProvider} to ${integration.name} and set model to ${trimmedTarget}?`,
      );
      if (!confirmed) {
        return;
      }
    }

    setQuickModelSavingId(integration.id);
    setQuickModelError(null);
    try {
      await putIntegrationCredentials(integration.id, {
        revision: settingsRevision,
        fields: {
          default_model: trimmedTarget,
        },
      });

      await loadData(false);
      setSaveSuccess(`Model updated to ${trimmedTarget} for ${integration.name}.`);
      setQuickModelDrafts((prev) => {
        const next = { ...prev };
        delete next[integration.id];
        return next;
      });
    } catch (err: unknown) {
      const message = err instanceof Error ? err.message : 'Failed to update model';
      if (message.includes('API 409')) {
        await loadData(false);
        setQuickModelError(
          'Configuration changed elsewhere. Refreshed latest settings; choose the model again.',
        );
      } else {
        setQuickModelError(message);
      }
    } finally {
      setQuickModelSavingId(null);
    }
  };

  const categories = [
    'all',
    ...Array.from(new Set(integrations.map((i) => i.category))).sort(),
  ];

  const filtered =
    activeCategory === 'all'
      ? integrations
      : integrations.filter((i) => i.category === activeCategory);

  // Group by category for display
  const grouped = filtered.reduce<Record<string, Integration[]>>((acc, item) => {
    const key = item.category;
    if (!acc[key]) acc[key] = [];
    acc[key].push(item);
    return acc;
  }, {});

  if (error) {
    return (
      <div className="p-6">
        <div className="rounded-lg bg-red-900/30 border border-red-700 p-4 text-red-300">
          Failed to load integrations: {error}
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
      <div className="flex items-center gap-2">
        <Puzzle className="h-5 w-5 text-blue-400" />
        <h2 className="text-base font-semibold text-white">
          Integrations ({integrations.length})
        </h2>
      </div>

      {saveSuccess && (
        <div className="rounded-lg bg-green-900/30 border border-green-700 p-3 text-sm text-green-300">
          {saveSuccess}
        </div>
      )}

      {quickModelError && (
        <div className="rounded-lg bg-red-900/30 border border-red-700 p-3 text-sm text-red-300">
          {quickModelError}
        </div>
      )}

      {/* Category Filter Tabs */}
      <div className="flex flex-wrap gap-2">
        {categories.map((cat) => (
          <button
            key={cat}
            onClick={() => setActiveCategory(cat)}
            className={`px-3 py-1.5 rounded-lg text-sm font-medium transition-colors capitalize ${
              activeCategory === cat
                ? 'bg-blue-600 text-white'
                : 'bg-gray-900 text-gray-400 border border-gray-700 hover:bg-gray-800 hover:text-white'
            }`}
          >
            {cat === 'all' ? 'All' : formatCategory(cat)}
          </button>
        ))}
      </div>

      {/* Grouped Integration Cards */}
      {Object.keys(grouped).length === 0 ? (
        <div className="bg-gray-900 rounded-xl border border-gray-800 p-8 text-center">
          <Puzzle className="h-10 w-10 text-gray-600 mx-auto mb-3" />
          <p className="text-gray-400">No integrations found.</p>
        </div>
      ) : (
        Object.entries(grouped)
          .sort(([a], [b]) => a.localeCompare(b))
          .map(([category, items]) => (
            <div key={category}>
              <h3 className="text-sm font-semibold text-gray-400 uppercase tracking-wider mb-3 capitalize">
                {formatCategory(category)}
              </h3>
              <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-4">
                {items.map((integration) => {
                  const badge = statusBadge(integration.status);
                  const BadgeIcon = badge.icon;
                  const editable = settingsByName[integration.name];
                  const isAiIntegration = !!editable?.activates_default_provider;
                  const isActiveDefaultProvider =
                    !!editable &&
                    isAiIntegration &&
                    editable.id === activeAiIntegrationId;
                  const modelField = editable ? modelFieldFor(editable) : undefined;
                  const modelOptions =
                    editable && modelField ? modelOptionsForField(editable.id, modelField) : [];
                  const currentModel =
                    editable && isAiIntegration
                      ? modelValueFor(editable, isActiveDefaultProvider)
                      : null;
                  const fallbackModel =
                    editable && isAiIntegration ? fallbackModelFor(editable) : null;
                  const modelSummary = currentModel
                    ? currentModel
                    : fallbackModel
                      ? `default: ${fallbackModel}`
                      : 'default';
                  const modelBaseline = currentModel ?? fallbackModel ?? '';
                  const quickDraft = editable
                    ? quickModelDrafts[editable.id] ?? modelBaseline
                    : '';
                  const quickOptions = [
                    ...(currentModel && !modelOptions.includes(currentModel)
                      ? [currentModel]
                      : []),
                    ...modelOptions,
                  ];
                  const showQuickModelControls =
                    !!editable &&
                    editable.configured &&
                    isAiIntegration &&
                    quickOptions.length > 0;

                  return (
                    <div
                      key={integration.name}
                      className={`bg-gray-900 rounded-xl border p-5 transition-colors ${
                        isActiveDefaultProvider
                          ? 'border-green-700/70 bg-gradient-to-b from-green-950/20 to-gray-900'
                          : 'border-gray-800 hover:border-gray-700'
                      }`}
                    >
                      <div className="flex items-start justify-between gap-3">
                        <div className="min-w-0">
                          <h4 className="text-sm font-semibold text-white truncate">
                            {integration.name}
                          </h4>
                          <p className="text-sm text-gray-400 mt-1 line-clamp-2">
                            {integration.description}
                          </p>
                        </div>
                        <div className="flex items-center gap-1.5 flex-wrap justify-end">
                          {isAiIntegration && editable?.configured && (
                            <span
                              className={`flex-shrink-0 inline-flex items-center gap-1 px-2 py-1 rounded-full text-xs font-medium border ${
                                isActiveDefaultProvider
                                  ? 'bg-emerald-900/40 text-emerald-300 border-emerald-700/60'
                                  : 'bg-gray-800 text-gray-300 border-gray-700'
                              }`}
                            >
                              {isActiveDefaultProvider ? 'Default' : 'Configured'}
                            </span>
                          )}
                          <span
                            className={`flex-shrink-0 inline-flex items-center gap-1 px-2 py-1 rounded-full text-xs font-medium border ${badge.classes}`}
                          >
                            <BadgeIcon className="h-3 w-3" />
                            {badge.label}
                          </span>
                        </div>
                      </div>

                      {editable && (isActiveDefaultProvider || runtimeStatus?.provider === integration.name) && (
                        <div className="mt-3 rounded-lg border border-gray-800 bg-gray-950/50 p-3 space-y-2">
                          <div className="flex items-center justify-between gap-2">
                            <span className="text-[11px] uppercase tracking-wider text-gray-500">
                              Current model
                            </span>
                            <span className="text-xs text-gray-200 truncate" title={modelSummary}>
                              {runtimeStatus?.provider === integration.name ? runtimeStatus.model : modelSummary}
                            </span>
                          </div>

                          {showQuickModelControls && editable && (
                            <div className="space-y-1">
                              <div className="flex items-center gap-2">
                                <select
                                  value={quickDraft}
                                  onChange={(e) =>
                                    setQuickModelDrafts((prev) => ({
                                      ...prev,
                                      [editable.id]: e.target.value,
                                    }))
                                  }
                                  disabled={quickModelSavingId === editable.id}
                                  className="min-w-0 flex-1 px-2.5 py-1.5 rounded-lg bg-gray-950 border border-gray-700 text-xs text-gray-200 focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent disabled:opacity-50"
                                >
                                  {quickOptions.map((option) => (
                                    <option key={option} value={option}>
                                      {option}
                                    </option>
                                  ))}
                                </select>
                                <button
                                  onClick={() =>
                                    editable &&
                                    void saveQuickModel(
                                      editable,
                                      quickDraft,
                                      modelBaseline,
                                      isActiveDefaultProvider,
                                    )
                                  }
                                  disabled={
                                    quickModelSavingId === editable.id ||
                                    !quickDraft ||
                                    quickDraft === modelBaseline
                                  }
                                  className="px-2.5 py-1.5 rounded-lg text-xs font-medium bg-blue-600 hover:bg-blue-700 text-white transition-colors disabled:opacity-50"
                                >
                                  {quickModelSavingId === editable.id ? 'Saving...' : 'Apply'}
                                </button>
                              </div>
                              <p className="text-[11px] text-gray-500">
                                For custom model IDs, use Edit Keys.
                              </p>
                            </div>
                          )}
                        </div>
                      )}

                      {editable && editable.configured && (
                        <div className="mt-4 pt-4 border-t border-gray-800">
                          <div className="text-xs text-gray-400">
                            {editable.activates_default_provider
                              ? isActiveDefaultProvider
                                ? 'Default provider configured'
                                : 'Provider configured'
                              : 'Credentials configured'}
                          </div>
                        </div>
                      )}
                    </div>
                  );
                })}
              </div>
            </div>
          ))
      )}

    </div>
  );
}
