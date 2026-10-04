import { useLocation, Link } from 'react-router-dom';
import { LogOut, Menu, PanelLeftClose, PanelLeftOpen, OctagonAlert } from 'lucide-react';
import { t } from '@/lib/i18n';
import { useAuth } from '@/hooks/useAuth';
import { useEstopStatus } from '@/hooks/EstopProvider';
import { navItems } from './Sidebar';

interface HeaderProps {
  isSidebarCollapsed: boolean;
  onToggleSidebar: () => void;
  onToggleSidebarCollapse: () => void;
}

export default function Header({
  isSidebarCollapsed,
  onToggleSidebar,
  onToggleSidebarCollapse,
}: HeaderProps) {
  const location = useLocation();
  const { logout } = useAuth();
  const { status: estopStatus } = useEstopStatus();

  const titleKey =
    navItems.find((item) => item.to === location.pathname)?.labelKey ?? 'nav.dashboard';
  const pageTitle = t(titleKey);

  return (
    <header className="glass-header relative flex min-h-[4.5rem] flex-wrap items-center justify-between gap-2 rounded-2xl border border-[#1a3670] px-4 py-3 sm:px-5 sm:py-3.5 md:flex-nowrap md:px-8 md:py-4">
      <div className="absolute inset-0 pointer-events-none opacity-70 bg-[radial-gradient(circle_at_15%_30%,rgba(41,148,255,0.22),transparent_45%),radial-gradient(circle_at_85%_75%,rgba(0,209,255,0.14),transparent_40%)]" />

      <div className="relative flex min-w-0 items-center gap-2.5 sm:gap-3">
        <button
          type="button"
          onClick={onToggleSidebar}
          aria-label="Open navigation"
          className="rounded-lg border border-[#294a8f] bg-[#081637]/70 p-1.5 text-[#9ec2ff] transition hover:border-[#4f83ff] hover:text-white md:hidden"
        >
          <Menu className="h-5 w-5" />
        </button>

        <div className="min-w-0">
          <h1 className="truncate text-base font-semibold tracking-wide text-white sm:text-lg">
            {pageTitle}
          </h1>
          <p className="hidden text-[10px] uppercase tracking-[0.16em] text-[#7ea5eb] sm:block">
            Electric dashboard
          </p>
        </div>
      </div>

      <div className="relative flex w-full items-center justify-end gap-1.5 sm:gap-2 md:w-auto md:gap-3">
        {estopStatus?.is_engaged && (
          <Link
            to="/estop"
            className="flex items-center gap-1.5 rounded-lg border border-red-700/60 bg-red-900/40 px-2.5 py-1.5 text-xs font-semibold text-red-300 transition hover:border-red-500 hover:text-white animate-pulse sm:px-3 sm:text-sm"
            title="Emergency stop is engaged"
          >
            <OctagonAlert className="h-4 w-4" />
            <span className="hidden sm:inline">Estop Engaged</span>
          </Link>
        )}
        <button
          type="button"
          onClick={onToggleSidebarCollapse}
          className="hidden items-center gap-1 rounded-lg border border-[#2b4f97] bg-[#091937]/75 px-2.5 py-1.5 text-xs text-[#c4d8ff] transition hover:border-[#4f83ff] hover:text-white md:flex md:text-sm"
          title={isSidebarCollapsed ? 'Expand sidebar' : 'Collapse sidebar'}
        >
          {isSidebarCollapsed ? <PanelLeftOpen className="h-4 w-4" /> : <PanelLeftClose className="h-4 w-4" />}
          <span>{isSidebarCollapsed ? 'Expand' : 'Collapse'}</span>
        </button>

        <button
          type="button"
          onClick={logout}
          className="flex items-center gap-1 rounded-lg border border-[#2b4f97] bg-[#091937]/75 px-2.5 py-1.5 text-xs text-[#c4d8ff] transition hover:border-[#4f83ff] hover:text-white sm:gap-1.5 sm:px-3 sm:text-sm"
        >
          <LogOut className="h-4 w-4" />
          <span className="hidden sm:inline">{t('auth.logout')}</span>
        </button>
      </div>
    </header>
  );
}
