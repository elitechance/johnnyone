import { Component, OnDestroy, OnInit, inject, signal } from '@angular/core';
import { Location } from '@angular/common';
import { Meta } from '@angular/platform-browser';
import { ActivatedRoute, RouterLink } from '@angular/router';
import {
  IonButton,
  IonCard,
  IonCardContent,
  IonCardHeader,
  IonCardSubtitle,
  IonCardTitle,
  IonContent,
  IonHeader,
  IonSpinner,
  IonTitle,
  IonToolbar,
} from '@ionic/angular/standalone';
import { GRAPHQL_API_URL } from '@johnnyone/ui';
import {
  ACT_COPY,
  ActView,
  ActionLinkPreview,
  ActionLinkResult,
  OPEN_IN_J1_PATH,
  canConfirm,
  clockOffsetMs,
  confirmRequest,
  graphqlField,
  offersOpenInJ1,
  previewRequest,
  readToken,
  tick,
  viewFromPreview,
  viewFromResult,
} from './act-page-logic';

/**
 * /act?t=<jwt> — the page a Discord action-link button opens. Public (no login): the signed,
 * 120 s, single-use token is the only credential, and it authorizes exactly one action.
 *
 * Loading this page NEVER acts: it only previews. Confirm is the one POST that uses the link.
 * It shows nothing beyond the label, the action and the target. All decisions live in
 * act-page-logic.ts (pure, specced); this component only does fetch, timers and rendering.
 */
@Component({
  selector: 'app-act-page',
  standalone: true,
  imports: [
    RouterLink,
    IonHeader,
    IonToolbar,
    IonTitle,
    IonContent,
    IonCard,
    IonCardHeader,
    IonCardTitle,
    IonCardSubtitle,
    IonCardContent,
    IonButton,
    IonSpinner,
  ],
  template: `
    <ion-header>
      <ion-toolbar>
        <ion-title>JohnnyOne</ion-title>
      </ion-toolbar>
    </ion-header>
    <ion-content class="ion-padding">
      @let v = view();
      <ion-card class="act-card">
        @switch (v.kind) {
          @case ('loading') {
            <ion-card-content><ion-spinner name="dots"></ion-spinner> Checking the link…</ion-card-content>
          }
          @case ('ready') {
            <ion-card-header>
              <ion-card-subtitle>{{ v.action }}</ion-card-subtitle>
              <ion-card-title>{{ v.label }}</ion-card-title>
            </ion-card-header>
            <ion-card-content>
              <p class="target">{{ v.target }}</p>
              <p class="countdown">Link expires in {{ v.countdown }}</p>
              <div class="actions">
                <ion-button class="act-btn" expand="block" (click)="confirm()">Confirm</ion-button>
                <ion-button class="act-btn" expand="block" fill="outline" color="medium" (click)="cancel()">Cancel</ion-button>
              </div>
            </ion-card-content>
          }
          @case ('confirming') {
            <ion-card-header>
              <ion-card-subtitle>{{ v.action }}</ion-card-subtitle>
              <ion-card-title>{{ v.label }}</ion-card-title>
            </ion-card-header>
            <ion-card-content><ion-spinner name="dots"></ion-spinner> Doing it…</ion-card-content>
          }
          @case ('done') {
            <ion-card-header><ion-card-title>Done</ion-card-title></ion-card-header>
            <ion-card-content><p>{{ v.summary }}</p></ion-card-content>
          }
          @case ('cancelled') {
            <ion-card-content><p>{{ v.message }}</p></ion-card-content>
          }
          @case ('expired') {
            @if (v.label) {
              <ion-card-header>
                <ion-card-subtitle>Expired</ion-card-subtitle>
                <ion-card-title>{{ v.label }}</ion-card-title>
              </ion-card-header>
            }
            <ion-card-content><p>{{ v.message }}</p></ion-card-content>
          }
          @default {
            <ion-card-content><p>{{ $any(v).message }}</p></ion-card-content>
          }
        }
        @if (openInJ1()) {
          <ion-card-content>
            <ion-button expand="block" fill="clear" [routerLink]="openPath">Open in J1</ion-button>
          </ion-card-content>
        }
      </ion-card>
    </ion-content>
  `,
  styles: [
    `
      .act-card { max-width: 480px; margin: 24px auto; }
      .target { font-size: 1.05rem; margin: 0 0 8px; overflow-wrap: anywhere; }
      .countdown { opacity: 0.7; margin: 0 0 16px; font-variant-numeric: tabular-nums; }
      /* H-1-4: >= 48px touch targets in every Ionic mode (md defaults to ~36px), and a real gap
         so a thumb cannot hit Cancel when it meant Confirm. */
      .actions { display: flex; flex-direction: column; gap: 16px; }
      .act-btn { --padding-top: 14px; --padding-bottom: 14px; min-height: 48px; height: auto; margin: 0; font-size: 1.05rem; }
    `,
  ],
})
export class ActPage implements OnInit, OnDestroy {
  private readonly apiUrl = inject(GRAPHQL_API_URL);
  private readonly route = inject(ActivatedRoute);
  private readonly location = inject(Location);
  private readonly meta = inject(Meta);

  readonly view = signal<ActView>({ kind: 'loading' });
  readonly openPath = OPEN_IN_J1_PATH;

  private token: string | null = null;
  private expiresAt: number | null = null;
  /** Server clock minus phone clock (H-1-10). */
  private offsetMs = 0;
  private timer: ReturnType<typeof setInterval> | null = null;

  openInJ1(): boolean {
    return offersOpenInJ1(this.view());
  }

  async ngOnInit(): Promise<void> {
    // Keep the token out of Referer headers and out of the address bar / history once read.
    this.meta.updateTag({ name: 'referrer', content: 'no-referrer' });
    this.token = readToken(this.route.snapshot.queryParamMap.get('t'));
    this.location.replaceState('/act');
    if (!this.token) {
      this.view.set({ kind: 'missing', message: ACT_COPY.missing });
      return;
    }
    try {
      const res = await fetch(this.apiUrl, previewRequest(this.token));
      const preview = graphqlField<ActionLinkPreview>(await res.json(), 'previewActionLink');
      if (!preview) {
        this.view.set({ kind: 'invalid', message: ACT_COPY.invalid });
        return;
      }
      this.expiresAt = preview.expiresAt;
      this.offsetMs = clockOffsetMs(preview.serverNow, Date.now());
      this.view.set(viewFromPreview(preview, this.now()));
      if (this.view().kind === 'ready') {
        this.timer = setInterval(() => {
          this.view.set(tick(this.view(), this.expiresAt, this.now()));
          if (this.view().kind !== 'ready') this.stopTimer();
        }, 1000);
      }
    } catch {
      this.view.set({ kind: 'failed', message: ACT_COPY.network });
    }
  }

  async confirm(): Promise<void> {
    const v = this.view();
    if (!this.token || !canConfirm(v) || v.kind !== 'ready') return;
    const request = confirmRequest(v, this.token);
    if (!request) return;
    this.stopTimer();
    this.view.set({ kind: 'confirming', label: v.label, action: v.action, target: v.target });
    try {
      const res = await fetch(this.apiUrl, request);
      const result = graphqlField<ActionLinkResult>(await res.json(), 'runActionLink');
      this.view.set(result ? viewFromResult(result) : { kind: 'failed', message: ACT_COPY.failed });
    } catch {
      // The POST may or may not have landed; the worker burns the link either way once it does.
      this.view.set({ kind: 'failed', message: ACT_COPY.uncertain });
    } finally {
      this.token = null;
    }
  }

  cancel(): void {
    this.stopTimer();
    this.token = null;
    this.view.set({ kind: 'cancelled', message: ACT_COPY.cancelled });
  }

  ngOnDestroy(): void {
    this.stopTimer();
    this.meta.removeTag("name='referrer'");
  }

  /** The worker's idea of now, from the phone clock plus the offset measured at preview. */
  private now(): number {
    return Date.now() + this.offsetMs;
  }

  private stopTimer(): void {
    if (this.timer) clearInterval(this.timer);
    this.timer = null;
  }
}
