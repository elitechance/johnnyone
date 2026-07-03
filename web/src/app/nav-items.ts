/**
 * Single source for the global navigation destinations (Overhaul P8 / phase 03).
 * Both the widescreen icon rail (`.gnav`) and the mobile bottom nav (`.botnav`)
 * render from this list, so the two nav surfaces can never drift (D4/D12).
 *
 * Pure — no Angular import — so the plugin-less web vitest can pin it
 * (`nav-items.spec.ts`). `path` targets an EXISTING route (`app.routes.ts`); no
 * new routes are added. `icon` names must be registered via `addIcons` in
 * `app.component.ts`.
 */
export interface NavItem {
  id: 'work' | 'files' | 'shells' | 'settings';
  /** Caption shown under the icon (rail `.cap` / bottom-nav label). */
  label: string;
  /** Registered ionicon name. */
  icon: string;
  /** Router path to an existing route. Active state is derived from this by `routerLinkActive`. */
  path: string;
}

export const NAV_ITEMS: NavItem[] = [
  // "Work" is the initiative console, which lands on the existing `terminal` route (D6).
  { id: 'work', label: 'Work', icon: 'apps-outline', path: '/terminal' },
  { id: 'files', label: 'Files', icon: 'folder-outline', path: '/files' },
  { id: 'shells', label: 'Shells', icon: 'code-working-outline', path: '/shells' },
  { id: 'settings', label: 'Settings', icon: 'settings-outline', path: '/settings' },
];
