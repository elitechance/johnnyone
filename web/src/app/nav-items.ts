/**
 * Single source for the global navigation destinations (Overhaul P8 / phase 03).
 * Both the widescreen icon rail (`.gnav`) and the mobile bottom nav (`.botnav`)
 * render from this list, so the two nav surfaces can never drift (D4/D12).
 *
 * Pure — no Angular import — so the plugin-less web vitest can pin it
 * (`nav-items.spec.ts`, `nav-active.spec.ts`). `path` targets an EXISTING route
 * (`app.routes.ts`); no new routes are added. `icon` names must be registered via
 * `addIcons` in `app.component.ts`.
 */
export interface NavItem {
  id: 'work' | 'arena' | 'files' | 'shells' | 'settings';
  /** Caption shown under the icon (rail `.cap` / bottom-nav label). */
  label: string;
  /** Registered ionicon name. */
  icon: string;
  /** Router path to an existing route. Tapping the item goes here. */
  path: string;
  /**
   * EVERY route prefix this destination owns — the whole area it stands for, not just the one
   * route it links to. The lit state is computed from this (`navItemIsActive`) instead of being
   * left to `routerLinkActive` on `path`, and that is the fix for a measured defect, not a
   * refactor: `routerLinkActive` lights an item only when the URL starts with its OWN `path`, so
   * the arena's top-level routes (`/workflows`, `/workflows/:id`, `/workflows/:id/arena`) lit
   * NOTHING. Measured in Chrome at 400x860 against the built bundle: `nav.botnav a.b.on` was the
   * empty set on all three. You were inside the app and the nav said you were nowhere.
   *
   * THE ARENA IS A FIFTH DESTINATION, and the cost was measured rather than guessed: the four
   * bottom-nav items were 100px wide each at a 400px viewport, so a fifth takes every one of them
   * to 80px. It is paid because the claim that survived re-measurement is precisely that there is
   * no way into the arena FROM THE GLOBAL NAV — neither the rail nor the bottom bar — while the
   * in-console route (an initiative's Arena tab) was always there and always worked. A front door
   * inside one initiative's tab bar is not a global entry, and making Work merely light up on
   * `/workflows` would have fixed the lit state while leaving the entry problem untouched.
   *
   * The 3-step setup wizard at `/initiatives/:id/arena` stays owned by WORK, not by Arena. It is a
   * route inside one initiative, its back link returns to that initiative's console, and today it
   * already lights Work correctly — re-measured, it is not a defect and nothing here changes it.
   */
  owns: string[];
}

export const NAV_ITEMS: NavItem[] = [
  // "Work" is the initiative console, which lands on the existing `terminal` route (D6). It owns
  // the whole initiative area: the console, the new-initiative form, and the workflow arena, whose
  // routes are split across `/initiatives/:id/arena` and the top-level `/workflows` tree.
  {
    id: 'work',
    label: 'Work',
    icon: 'apps-outline',
    path: '/initiatives',
    owns: ['/initiatives', '/terminal', '/briefing'],
  },
  // The arena. `owns` is the `/workflows` tree only; see the note on `owns` for why the per-
  // initiative setup wizard stays under Work.
  { id: 'arena', label: 'Arena', icon: 'trophy-outline', path: '/workflows', owns: ['/workflows'] },
  { id: 'files', label: 'Files', icon: 'folder-outline', path: '/files', owns: ['/files'] },
  { id: 'shells', label: 'Shells', icon: 'code-working-outline', path: '/shells', owns: ['/shells'] },
  { id: 'settings', label: 'Settings', icon: 'settings-outline', path: '/settings', owns: ['/settings'] },
];

/**
 * Is `item` the destination the URL is inside? Prefix match ON A SEGMENT BOUNDARY, so `/files` does
 * not light for a hypothetical `/filesystem` and `/settings` still lights for `/settings/prompts`.
 * Query and fragment are stripped first: the console's own tab state lives in `?tab=`, and an item
 * that went dark when a tab changed would be a worse bug than the one this replaces.
 */
export function navItemIsActive(item: NavItem, url: string): boolean {
  const pathOnly = (url.split('#')[0].split('?')[0] || '/').replace(/\/+$/, '') || '/';
  return item.owns.some((p) => pathOnly === p || pathOnly.startsWith(`${p}/`));
}

/** Which destinations the URL lights. More than one would mean two `owns` lists overlap. */
export function litNavItemIds(url: string): NavItem['id'][] {
  return NAV_ITEMS.filter((item) => navItemIsActive(item, url)).map((item) => item.id);
}

/**
 * The query the GLOBAL arena entry carries, so tapping it from the console does not throw away
 * which initiative you were on.
 *
 * Measured at 1440x900 on a 3-initiative fixture, with the global entry added and this function
 * NOT yet written: on initiative p2's Arena tab, tapping the rail's Arena item and then the arena's
 * back link put a bare `/initiatives` in the address bar; reloading that landed on p1's Raw
 * terminal, because the console auto-selects `plans[0]`. In-session it looked right — the console
 * component never unmounted — which is why the check that found it reloads.
 *
 * It applies to the ARENA ITEM ONLY, and only from inside the area the arena belongs to. Files and
 * Shells are separate areas and a return path between them is not implied; Work's own item is
 * already where `from` would point. The parameter name matches the one the arena pages read
 * (`arena-nav.ts`), and those pages refuse any value that is not app-relative.
 */
export function navLinkQueryParams(item: NavItem, url: string): Record<string, string> {
  if (item.id !== 'arena') return {};
  const pathOnly = url.split('#')[0].split('?')[0];
  const inConsole = pathOnly === '/initiatives' || pathOnly.startsWith('/initiatives/');
  return inConsole && url.startsWith('/') ? { from: url } : {};
}
