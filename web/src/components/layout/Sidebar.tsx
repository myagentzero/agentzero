import { useEffect, useState } from 'react';
import { NavLink } from 'react-router-dom';
import {
  ChevronsLeftRightEllipsis,
  LayoutDashboard,
  MessageSquare,
  Wrench,
  BookOpen,
  Clock,
  Puzzle,
  Brain,
  Smartphone,
  Settings,
  DollarSign,
  Activity,
  Stethoscope,
  FolderOpen,
  X,
} from 'lucide-react';
import { t } from '@/lib/i18n';

const COLLAPSE_BUTTON_DELAY_MS = 1000;

const navItems = [
  { to: '/', icon: LayoutDashboard, labelKey: 'nav.dashboard' },
  { to: '/agent', icon: MessageSquare, labelKey: 'nav.agent' },
  { to: '/mission-control', icon: Activity, labelKey: 'nav.logs' },
  { to: '/cost', icon: DollarSign, labelKey: 'nav.cost' },
  { to: '/integrations', icon: Puzzle, labelKey: 'nav.integrations' },
  { to: '/tools', icon: Wrench, labelKey: 'nav.tools' },
  { to: '/skills', icon: BookOpen, labelKey: 'nav.skills' },
  { to: '/workspace', icon: FolderOpen, labelKey: 'nav.workspace' },
  { to: '/memory', icon: Brain, labelKey: 'nav.memory' },
  { to: '/cron', icon: Clock, labelKey: 'nav.cron' },
  { to: '/devices', icon: Smartphone, labelKey: 'nav.devices' },
  { to: '/config', icon: Settings, labelKey: 'nav.config' },
  { to: '/doctor', icon: Stethoscope, labelKey: 'nav.doctor' },
];

interface SidebarProps {
  isOpen: boolean;
  isCollapsed: boolean;
  onClose: () => void;
  onToggleCollapse: () => void;
}

export default function Sidebar({
  isOpen,
  isCollapsed,
  onClose,
  onToggleCollapse,
}: SidebarProps) {
  const [showCollapseButton, setShowCollapseButton] = useState(false);

  useEffect(() => {
    const id = setTimeout(() => setShowCollapseButton(true), COLLAPSE_BUTTON_DELAY_MS);
    return () => clearTimeout(id);
  }, []);

  return (
    <>
      <button
        type="button"
        aria-label="Close navigation"
        onClick={onClose}
        className={[
          'fixed inset-0 z-30 bg-black/50 transition-opacity md:hidden',
          isOpen ? 'opacity-100' : 'pointer-events-none opacity-0',
        ].join(' ')}
      />
      <aside
        className={[
          'fixed left-0 top-0 z-40 flex h-screen w-[86vw] max-w-[17.5rem] flex-col border-r border-[#1e2f5d] bg-[#050b1a]/95 backdrop-blur-xl',
          'shadow-[0_0_50px_-25px_rgba(8,121,255,0.7)]',
          'transform transition-[width,transform] duration-300 ease-out',
          isOpen ? 'translate-x-0' : '-translate-x-full',
          isCollapsed ? 'md:w-[6.25rem]' : 'md:w-[17.5rem]',
          'md:translate-x-0',
        ].join(' ')}
      >
        <div className="relative flex items-center justify-between border-b border-[#1a2d5e] px-4 py-4">
          <div className="flex items-center gap-3 overflow-hidden">
            {!isCollapsed && (
              <>
                <div
                  className="electric-brand-mark h-9 w-9 shrink-0 rounded-xl"
                  role="img"
                  aria-label="AgentZero"
                >
                  <svg
                    xmlns="http://www.w3.org/2000/svg"
                    viewBox="0 0 512 512"
                    className="electric-brand-glyph"
                    aria-hidden="true"
                    focusable="false"
                  >
                    <defs>
                      <radialGradient id="zeroclaw-brand-bg" cx="50%" cy="50%" r="50%">
                        <stop offset="0%" stopColor="#1a1a2e" />
                        <stop offset="100%" stopColor="#0d0d1a" />
                      </radialGradient>
                      <filter id="zeroclaw-brand-glow">
                        <feGaussianBlur stdDeviation="4" result="blur" />
                        <feMerge>
                          <feMergeNode in="blur" />
                          <feMergeNode in="SourceGraphic" />
                        </feMerge>
                      </filter>
                      <filter id="zeroclaw-brand-eye-glow">
                        <feGaussianBlur stdDeviation="8" result="blur" />
                        <feMerge>
                          <feMergeNode in="blur" />
                          <feMergeNode in="blur" />
                          <feMergeNode in="SourceGraphic" />
                        </feMerge>
                      </filter>
                      <filter id="zeroclaw-brand-circuit-glow">
                        <feGaussianBlur stdDeviation="2" result="blur" />
                        <feMerge>
                          <feMergeNode in="blur" />
                          <feMergeNode in="SourceGraphic" />
                        </feMerge>
                      </filter>
                    </defs>
                    <circle cx="256" cy="256" r="256" fill="url(#zeroclaw-brand-bg)" />
                    <g opacity="0.08" stroke="#ff6b35" strokeWidth="0.5" fill="none">
                      <path d="M128,80 L158,60 L188,80 L188,120 L158,140 L128,120Z" />
                      <path d="M188,80 L218,60 L248,80 L248,120 L218,140 L188,120Z" />
                      <path d="M248,80 L278,60 L308,80 L308,120 L278,140 L248,120Z" />
                      <path d="M308,80 L338,60 L368,80 L368,120 L338,140 L308,120Z" />
                      <path d="M158,140 L188,120 L218,140 L218,180 L188,200 L158,180Z" />
                      <path d="M218,140 L248,120 L278,140 L278,180 L248,200 L218,180Z" />
                      <path d="M278,140 L308,120 L338,140 L338,180 L308,200 L278,180Z" />
                      <path d="M128,340 L158,320 L188,340 L188,380 L158,400 L128,380Z" />
                      <path d="M308,340 L338,320 L368,340 L368,380 L338,400 L308,380Z" />
                      <path d="M188,400 L218,380 L248,400 L248,440 L218,460 L188,440Z" />
                      <path d="M248,400 L278,380 L308,400 L308,440 L278,460 L248,440Z" />
                    </g>
                    <g filter="url(#zeroclaw-brand-circuit-glow)" stroke="#ff6b35" strokeWidth="1.5" fill="none" opacity="0.25">
                      <path d="M80,256 L130,256 L145,240" />
                      <path d="M432,256 L382,256 L367,240" />
                      <path d="M256,440 L256,410 L240,395" />
                      <path d="M256,72 L256,102 L240,117" />
                      <circle cx="80" cy="256" r="3" fill="#ff6b35" />
                      <circle cx="432" cy="256" r="3" fill="#ff6b35" />
                      <circle cx="256" cy="440" r="3" fill="#ff6b35" />
                      <circle cx="256" cy="72" r="3" fill="#ff6b35" />
                    </g>
                    <ellipse cx="256" cy="270" rx="95" ry="75" fill="#c0392b" opacity="0.9" />
                    <ellipse cx="256" cy="260" rx="88" ry="68" fill="#e74c3c" />
                    <ellipse cx="245" cy="240" rx="55" ry="35" fill="#ff6b6b" opacity="0.3" />
                    <g stroke="#c0392b" strokeWidth="2" fill="none" opacity="0.5">
                      <path d="M200,235 Q256,210 312,235" />
                      <path d="M190,260 Q256,235 322,260" />
                      <path d="M195,285 Q256,260 317,285" />
                    </g>
                    <g filter="url(#zeroclaw-brand-glow)">
                      <path d="M168,260 L120,220 L95,195" stroke="#e74c3c" strokeWidth="14" fill="none" strokeLinecap="round" strokeLinejoin="round" />
                      <path d="M95,195 L55,160 L70,175" stroke="#ff6b35" strokeWidth="12" fill="none" strokeLinecap="round" strokeLinejoin="round" />
                      <path d="M95,195 L60,200 L75,190" stroke="#ff6b35" strokeWidth="12" fill="none" strokeLinecap="round" strokeLinejoin="round" />
                      <circle cx="55" cy="160" r="5" fill="#ff9f43" />
                      <circle cx="60" cy="200" r="5" fill="#ff9f43" />
                    </g>
                    <g filter="url(#zeroclaw-brand-glow)">
                      <path d="M344,260 L392,220 L417,195" stroke="#e74c3c" strokeWidth="14" fill="none" strokeLinecap="round" strokeLinejoin="round" />
                      <path d="M417,195 L457,160 L442,175" stroke="#ff6b35" strokeWidth="12" fill="none" strokeLinecap="round" strokeLinejoin="round" />
                      <path d="M417,195 L452,200 L437,190" stroke="#ff6b35" strokeWidth="12" fill="none" strokeLinecap="round" strokeLinejoin="round" />
                      <circle cx="457" cy="160" r="5" fill="#ff9f43" />
                      <circle cx="452" cy="200" r="5" fill="#ff9f43" />
                    </g>
                    <g stroke="#c0392b" strokeWidth="6" fill="none" strokeLinecap="round">
                      <path d="M180,290 L140,320 L120,350" />
                      <path d="M175,305 L135,340 L118,368" />
                      <path d="M178,315 L145,355 L130,385" />
                    </g>
                    <g fill="#ff6b35">
                      <circle cx="120" cy="350" r="4" />
                      <circle cx="118" cy="368" r="4" />
                      <circle cx="130" cy="385" r="4" />
                    </g>
                    <g stroke="#c0392b" strokeWidth="6" fill="none" strokeLinecap="round">
                      <path d="M332,290 L372,320 L392,350" />
                      <path d="M337,305 L377,340 L394,368" />
                      <path d="M334,315 L367,355 L382,385" />
                    </g>
                    <g fill="#ff6b35">
                      <circle cx="392" cy="350" r="4" />
                      <circle cx="394" cy="368" r="4" />
                      <circle cx="382" cy="385" r="4" />
                    </g>
                    <path d="M220,215 L210,175 L208,155" stroke="#e74c3c" strokeWidth="8" fill="none" strokeLinecap="round" />
                    <path d="M292,215 L302,175 L304,155" stroke="#e74c3c" strokeWidth="8" fill="none" strokeLinecap="round" />
                    <g filter="url(#zeroclaw-brand-eye-glow)">
                      <circle cx="208" cy="150" r="18" fill="#1a1a2e" stroke="#ff6b35" strokeWidth="2" />
                      <circle cx="208" cy="150" r="12" fill="#ff6b35" />
                      <circle cx="208" cy="150" r="6" fill="#fff" opacity="0.9" />
                      <circle cx="204" cy="146" r="3" fill="#fff" />
                      <circle cx="304" cy="150" r="18" fill="#1a1a2e" stroke="#ff6b35" strokeWidth="2" />
                      <circle cx="304" cy="150" r="12" fill="#ff6b35" />
                      <circle cx="304" cy="150" r="6" fill="#fff" opacity="0.9" />
                      <circle cx="300" cy="146" r="3" fill="#fff" />
                    </g>
                    <g transform="translate(256,265)" opacity="0.6">
                      <circle r="18" fill="none" stroke="#ff6b35" strokeWidth="2" />
                      <circle r="10" fill="none" stroke="#ff6b35" strokeWidth="1.5" />
                      <g stroke="#ff6b35" strokeWidth="3" strokeLinecap="round">
                        <line x1="0" y1="-18" x2="0" y2="-24" />
                        <line x1="15.6" y1="-9" x2="20.8" y2="-12" />
                        <line x1="15.6" y1="9" x2="20.8" y2="12" />
                        <line x1="0" y1="18" x2="0" y2="24" />
                        <line x1="-15.6" y1="9" x2="-20.8" y2="12" />
                        <line x1="-15.6" y1="-9" x2="-20.8" y2="-12" />
                      </g>
                      <circle r="4" fill="#ff6b35" />
                    </g>
                    <path id="zeroclaw-brand-text-arc" d="M130,400 Q256,460 382,400" fill="none" />
                    <text fontFamily="'Courier New', monospace" fontSize="22" fontWeight="bold" fill="#ff6b35" textAnchor="middle" filter="url(#zeroclaw-brand-circuit-glow)" opacity="0.85">
                      <textPath href="#zeroclaw-brand-text-arc" startOffset="50%">AgentZero</textPath>
                    </text>
                    <text fontFamily="'Courier New', monospace" fontSize="9" fill="#ff6b35" opacity="0.15" textAnchor="middle">
                      <tspan x="256" y="480">01011010 01000011</tspan>
                    </text>
                    <circle cx="256" cy="256" r="248" fill="none" stroke="#ff6b35" strokeWidth="1" opacity="0.2" />
                    <circle cx="256" cy="256" r="252" fill="none" stroke="#ff6b35" strokeWidth="0.5" opacity="0.1" strokeDasharray="8,4" />
                  </svg>
                  <span className="sr-only">AgentZero</span>
                </div>
                <span className="text-lg font-semibold tracking-[0.1em] text-white">
                  AgentZero
                </span>
              </>
            )}
          </div>

          <div className="flex items-center gap-2">
            {showCollapseButton && (
              <button
                type="button"
                onClick={onToggleCollapse}
                aria-label={isCollapsed ? 'Expand navigation' : 'Collapse navigation'}
                className="hidden rounded-lg border border-[#2c4e97] bg-[#0a1b3f]/60 p-1.5 text-[#8bb9ff] transition hover:border-[#4f83ff] hover:text-white md:block"
              >
                <ChevronsLeftRightEllipsis className="h-4 w-4" />
              </button>
            )}
            <button
              type="button"
              onClick={onClose}
              aria-label="Close navigation"
              className="rounded-lg p-1.5 text-gray-300 transition-colors hover:bg-gray-800 hover:text-white md:hidden"
            >
              <X className="h-4 w-4" />
            </button>
          </div>
        </div>

        <nav className="flex-1 space-y-1 overflow-y-auto px-3 py-4">
          {navItems.map(({ to, icon: Icon, labelKey }) => (
            <NavLink
              key={to}
              to={to}
              end={to === '/'}
              onClick={onClose}
              title={isCollapsed ? t(labelKey) : undefined}
              className={({ isActive }) =>
                [
                  'group flex items-center gap-3 overflow-hidden rounded-xl px-3 py-2.5 text-sm font-medium transition-all duration-300',
                  isActive
                    ? 'border border-[#3a6de0] bg-[#0b2f80]/55 text-white shadow-[0_0_30px_-16px_rgba(72,140,255,0.95)]'
                    : 'border border-transparent text-[#9bb7eb] hover:border-[#294a8d] hover:bg-[#07132f] hover:text-white',
                ].join(' ')
              }
            >
              <Icon className="h-5 w-5 shrink-0 transition-transform duration-300 group-hover:scale-110" />
              <span
                className={[
                  'whitespace-nowrap transition-[opacity,transform,width] duration-300',
                  isCollapsed ? 'w-0 -translate-x-3 opacity-0 md:invisible' : 'w-auto opacity-100',
                ].join(' ')}
              >
                {t(labelKey)}
              </span>
            </NavLink>
          ))}
        </nav>

        <div
          className={[
            'mx-3 mb-4 rounded-xl border border-[#1b3670] bg-[#071328]/80 px-3 py-3 text-xs text-[#89a9df] transition-all duration-300',
            isCollapsed ? 'md:px-1.5 md:text-center' : '',
          ].join(' ')}
        >
          <p className={isCollapsed ? 'hidden md:block' : ''}>Gateway + Dashboard</p>
          <p className={isCollapsed ? 'text-[10px] uppercase tracking-widest' : 'mt-1 text-[#5f84cc]'}>
            {isCollapsed ? 'UI' : 'Runtime Mode'}
          </p>
        </div>
      </aside>
    </>
  );
}
