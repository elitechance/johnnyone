/**
 * The arena's shared stylesheet, as a string so the components that use it stay JIT-compilable
 * (a `styleUrl` makes a component unresolvable under the plugin-less web vitest — measured). It is
 * ONE copy for all the arena surfaces, so the phone rules cannot drift between them, and
 * `arena-phone.spec.ts` runs the static phone-reach guard over this exact text.
 *
 * PHONE FIRST, deliberately and with a precedent to avoid: the Plan tab's document navigator was
 * `display: none` below 720px, so on a phone no plan document was reachable at all. Nothing here
 * hides a surface at a narrow width — every multi-column layout is a single column by default and
 * only becomes columns when there is room (`min-width`), and no box declares a px width wider than
 * a phone's.
 *
 * Colours come from the Ionic theme variables the rest of the console already uses, so the arena
 * inherits light/dark instead of defining a second palette.
 */
export const ARENA_CSS = `
:host {
  display: block;
  color: var(--ion-text-color, #e6e6e6);
  font-size: 14px;
  line-height: 1.45;
}
.aw {
  box-sizing: border-box;
  padding: 12px 16px 48px;
  max-width: 1100px;
  margin: 0 auto;
}
.a-head {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  gap: 8px;
  margin: 4px 0 10px;
}
.a-head h1 { font-size: 18px; margin: 0; font-weight: 600; }
.a-head .eyebrow {
  font-size: 11px;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  opacity: 0.65;
}
.a-back { font-size: 13px; text-decoration: none; opacity: 0.8; }
.a-card {
  border: 1px solid var(--ion-color-step-200, rgba(255,255,255,0.14));
  border-radius: 10px;
  padding: 12px;
  margin: 0 0 12px;
  background: var(--ion-card-background, rgba(255,255,255,0.03));
}
.a-card h2 { font-size: 14px; margin: 0 0 6px; font-weight: 600; }
.a-note { font-size: 12.5px; opacity: 0.8; margin: 6px 0 0; }
.a-warn {
  border-left: 3px solid var(--ion-color-warning, #e0a030);
  padding: 8px 10px;
  margin: 0 0 12px;
  font-size: 13px;
  background: rgba(224, 160, 48, 0.08);
}
.a-block {
  border-left: 3px solid var(--ion-color-danger, #d33);
  padding: 8px 10px;
  margin: 8px 0;
  font-size: 13px;
  background: rgba(221, 51, 51, 0.08);
}
.a-ok { color: var(--ion-color-success, #2dd36f); }
.a-bad { color: var(--ion-color-danger, #eb445a); }
.a-muted { opacity: 0.7; }
.mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 12.5px;
  overflow-wrap: anywhere;
}
/* Two columns ONLY when there is room. One column is the default, so a phone loses nothing. */
.a-cols { display: grid; grid-template-columns: 1fr; gap: 12px; }
@media (min-width: 760px) { .a-cols { grid-template-columns: 1fr 1fr; } }
.a-row {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
}
.a-kv { display: grid; grid-template-columns: auto 1fr; gap: 2px 10px; font-size: 13px; }
.a-kv dt { opacity: 0.65; }
.a-kv dd { margin: 0; overflow-wrap: anywhere; }
button.a-btn, a.a-btn {
  font: inherit;
  display: inline-block;
  padding: 9px 14px;
  min-height: 40px;
  border-radius: 8px;
  border: 1px solid var(--ion-color-primary, #3880ff);
  background: transparent;
  color: var(--ion-color-primary, #5b9dff);
  text-decoration: none;
  cursor: pointer;
}
button.a-btn.primary {
  background: var(--ion-color-primary, #3880ff);
  color: var(--ion-color-primary-contrast, #fff);
}
button.a-btn.danger {
  border-color: var(--ion-color-danger, #d33);
  color: var(--ion-color-danger, #eb445a);
}
button.a-btn[disabled] { opacity: 0.45; cursor: not-allowed; }
textarea.a-input, input.a-input {
  font: inherit;
  width: 100%;
  box-sizing: border-box;
  min-height: 40px;
  padding: 8px 10px;
  border-radius: 8px;
  border: 1px solid var(--ion-color-step-200, rgba(255,255,255,0.18));
  background: var(--ion-item-background, rgba(0,0,0,0.2));
  color: inherit;
}
textarea.a-input { min-height: 80px; }
.a-steps { display: flex; flex-wrap: wrap; gap: 6px; font-size: 12px; margin: 0 0 10px; }
.a-steps .st { padding: 3px 8px; border-radius: 999px; border: 1px solid var(--ion-color-step-200, rgba(255,255,255,0.14)); }
.a-steps .st.on { border-color: var(--ion-color-primary, #3880ff); }
.a-list { list-style: none; margin: 0; padding: 0; }
.a-list li { padding: 8px 0; border-top: 1px solid var(--ion-color-step-150, rgba(255,255,255,0.08)); }
.a-list li:first-child { border-top: 0; }
pre.a-out {
  margin: 8px 0 0;
  padding: 8px;
  max-height: 220px;
  overflow: auto;
  background: var(--ion-item-background, rgba(0,0,0,0.25));
  border-radius: 6px;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  font-size: 12px;
}
`;
