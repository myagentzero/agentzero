const translations: Record<string, string> = {
  // Navigation
  'nav.dashboard': 'Dashboard',
  'nav.agent': 'Agent',
  'nav.tools': 'Tools',
  'nav.skills': 'Skills',
  'nav.cron': 'Scheduled Jobs',
  'nav.integrations': 'Integrations',
  'nav.memory': 'Memory',
  'nav.tasks': 'Tasks',
  'nav.devices': 'Devices',
  'nav.config': 'Configuration',
  'nav.cost': 'Cost Tracker',
  'nav.logs': 'Mission Control',
  'nav.doctor': 'Doctor',
  'nav.workspace': 'Workspace',
  'nav.estop': 'Emergency Stop',

  // Auth
  'auth.logout': 'Logout',
};

/** Translate a key. Returns the key itself if no translation is found. */
export function t(key: string): string {
  return translations[key] ?? key;
}
