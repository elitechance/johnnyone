import { Component } from '@angular/core';
import {
  IonContent,
  IonHeader,
  IonTitle,
  IonToolbar,
} from '@ionic/angular/standalone';
import { getWorkerBaseUrl } from '../../../worker-url';

/**
 * Public partner / third-party API integration guide.
 *
 * Served at the unauthenticated `/integration` route (no authGuard) — this is
 * the in-app replacement for the old standalone johnnyone-partner-api.pages.dev
 * docs site. The source-of-truth design copy still lives at
 * `docs/api-partner/index.html`; keep the two in sync when the API surface
 * changes (see docs/api-partner/runbooks/api-versioning.md).
 *
 * Base URLs are resolved at runtime via getWorkerBaseUrl() so the examples on
 * the prod-served page show the prod worker, dev shows dev, etc.
 */
@Component({
  selector: 'app-integration-page',
  standalone: true,
  imports: [IonHeader, IonToolbar, IonTitle, IonContent],
  templateUrl: './integration.page.html',
  styleUrl: './integration.page.scss',
})
export class IntegrationPage {
  /** e.g. https://johnnyone.ethan-353.workers.dev (prod) — matches the env serving this page. */
  protected readonly base = getWorkerBaseUrl().replace(/\/$/, '');
  /** wss:// variant of the base, for the live-terminal examples. */
  protected readonly wsBase = this.base.replace(/^https/, 'wss').replace(/^http/, 'ws');

  /** Anchor sections rendered in the on-this-page table of contents. */
  protected readonly sections = [
    { id: 'overview', label: 'Overview' },
    { id: 'access', label: 'Access' },
    { id: 'auth', label: '— Authentication (JWT)' },
    { id: 'apikeys', label: '— API keys (M2M)' },
    { id: 'graphql', label: 'API (GraphQL)' },
    { id: 'sessions', label: 'Terminal sessions' },
    { id: 'providers', label: 'Providers & models' },
    { id: 'plans', label: 'Planner / development' },
    { id: 'wss', label: 'Live terminal (WSS)' },
    { id: 'quickstart', label: 'Quickstart (Node.js)' },
    { id: 'errors', label: 'Errors & limits' },
    { id: 'versioning', label: 'Versioning' },
    { id: 'agents', label: 'For AI agents' },
  ];

  /** ion-content scrolls inside its own container, so use scrollIntoView for TOC jumps. */
  protected scrollTo(id: string): void {
    document.getElementById(id)?.scrollIntoView({ behavior: 'smooth', block: 'start' });
  }
}
