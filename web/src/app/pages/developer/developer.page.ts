import { Component, OnDestroy, OnInit, computed, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { Subscription, firstValueFrom } from 'rxjs';
import {
  IonBadge,
  IonButton,
  IonCheckbox,
  IonChip,
  IonContent,
  IonHeader,
  IonIcon,
  IonInput,
  IonItem,
  IonLabel,
  IonList,
  IonListHeader,
  IonNote,
  IonSegment,
  IonSegmentButton,
  IonSpinner,
  IonText,
  IonTitle,
  IonToolbar,
} from '@ionic/angular/standalone';
import { addIcons } from 'ionicons';
import {
  copyOutline,
  keyOutline,
  refreshOutline,
  trashOutline,
} from 'ionicons/icons';
import {
  AiSession,
  ApiKey,
  JohnnyApiService,
  TerminalScreen,
  TerminalScreenComponent,
} from '@johnnyone/ui';
import { AuthService } from '../../services/auth.service';
import { RelayTerminalService } from '../../services/relay-terminal.service';

type DevSegment = 'auth' | 'agents' | 'wss';

/** The scopes a partner API key can carry (worker scopes.ts / runbook §M2M). */
const ALL_SCOPES = [
  'terminal:read',
  'terminal:write',
  'sessions:read',
  'sessions:write',
  'plans:read',
  'plans:write',
];

/**
 * Developer console — the in-app UI for driving the partner API surface:
 * Auth/OAuth (account + access token + M2M API keys), Agents (AI/terminal
 * session CRUD), and a Live Terminal (WSS) playground. Authenticated route
 * (/developer); reuses the shared JohnnyApiService + RelayTerminalService so it
 * exercises the exact same GraphQL/WSS contract documented at /integration.
 */
@Component({
  selector: 'app-developer-page',
  standalone: true,
  imports: [
    FormsModule,
    IonHeader,
    IonToolbar,
    IonTitle,
    IonContent,
    IonSegment,
    IonSegmentButton,
    IonLabel,
    IonList,
    IonListHeader,
    IonItem,
    IonInput,
    IonButton,
    IonIcon,
    IonNote,
    IonText,
    IonBadge,
    IonChip,
    IonCheckbox,
    IonSpinner,
    TerminalScreenComponent,
  ],
  templateUrl: './developer.page.html',
  styleUrl: './developer.page.scss',
})
export class DeveloperPage implements OnInit, OnDestroy {
  private readonly api = inject(JohnnyApiService);
  private readonly auth = inject(AuthService);
  private readonly relay = inject(RelayTerminalService);

  protected readonly segment = signal<DevSegment>('auth');
  protected readonly allScopes = ALL_SCOPES;

  // ── Auth / OAuth ────────────────────────────────────────────────────────
  protected readonly user = this.auth.currentUser;
  protected readonly tenantId = this.auth.getTenantId();
  protected readonly accessToken = signal<string>(this.auth.getAccessToken() ?? '');
  protected readonly tokenPreview = computed(() => {
    const t = this.accessToken();
    return t ? `${t.slice(0, 12)}…${t.slice(-6)}` : '(none)';
  });

  protected readonly apiKeys = signal<ApiKey[]>([]);
  protected readonly keysLoading = signal(false);
  protected readonly keyName = signal('');
  protected readonly keyExpiresAt = signal('');
  protected readonly selectedScopes = signal<Set<string>>(
    new Set(['sessions:read', 'sessions:write', 'terminal:read', 'terminal:write']),
  );
  protected readonly creatingKey = signal(false);
  /** Full jk_… secret, held transiently and shown once after create (reveal-once). */
  protected readonly revealedSecret = signal<string | null>(null);

  // ── Agents (AI / terminal sessions) ──────────────────────────────────────
  protected readonly sessions = signal<AiSession[]>([]);
  protected readonly sessionsLoading = signal(false);
  protected readonly newProvider = signal('claude_code');
  protected readonly newWorkingDir = signal('');
  protected readonly newTitle = signal('');
  protected readonly creatingSession = signal(false);

  // ── Live terminal (WSS) ──────────────────────────────────────────────────
  protected readonly wssSessionId = signal<string>('');
  protected readonly wssConnected = signal(false);
  protected readonly wssScreen = signal<TerminalScreen | null>(null);
  protected readonly wssInput = signal('');
  protected readonly wssLog = signal<string[]>([]);
  private screenSub?: Subscription;

  protected readonly error = signal<string>('');

  constructor() {
    addIcons({
      'key-outline': keyOutline,
      'copy-outline': copyOutline,
      'trash-outline': trashOutline,
      'refresh-outline': refreshOutline,
    });
  }

  ngOnInit(): void {
    void this.loadApiKeys();
    void this.loadSessions();
  }

  ngOnDestroy(): void {
    this.screenSub?.unsubscribe();
    this.relay.disconnect();
  }

  protected setSegment(value: string | number | undefined): void {
    if (value === 'auth' || value === 'agents' || value === 'wss') {
      this.segment.set(value);
    }
  }

  private fail(err: unknown): void {
    this.error.set(err instanceof Error ? err.message : String(err));
  }

  protected async copy(text: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      // clipboard may be unavailable (insecure context); ignore silently
    }
  }

  // ── Auth / API keys ──────────────────────────────────────────────────────

  protected scopeChecked(scope: string): boolean {
    return this.selectedScopes().has(scope);
  }

  protected toggleScope(scope: string, checked: boolean): void {
    this.selectedScopes.update((set) => {
      const next = new Set(set);
      if (checked) next.add(scope);
      else next.delete(scope);
      return next;
    });
  }

  protected async loadApiKeys(): Promise<void> {
    this.keysLoading.set(true);
    this.error.set('');
    try {
      this.apiKeys.set(await firstValueFrom(this.api.listApiKeys()));
    } catch (err) {
      this.fail(err);
    } finally {
      this.keysLoading.set(false);
    }
  }

  protected async createKey(): Promise<void> {
    const name = this.keyName().trim();
    const scopes = [...this.selectedScopes()];
    if (!name || scopes.length === 0 || this.creatingKey()) return;
    this.creatingKey.set(true);
    this.error.set('');
    try {
      const expiresAt = this.keyExpiresAt().trim();
      const result = await firstValueFrom(
        this.api.createApiKey({ name, scopes, ...(expiresAt ? { expiresAt } : {}) }),
      );
      this.revealedSecret.set(result.secret);
      this.keyName.set('');
      this.keyExpiresAt.set('');
      await this.loadApiKeys();
    } catch (err) {
      this.fail(err);
    } finally {
      this.creatingKey.set(false);
    }
  }

  protected dismissSecret(): void {
    this.revealedSecret.set(null);
  }

  protected async revokeKey(key: ApiKey): Promise<void> {
    if (key.revokedAt) return;
    this.error.set('');
    try {
      await firstValueFrom(this.api.revokeApiKey(key.id));
      await this.loadApiKeys();
    } catch (err) {
      this.fail(err);
    }
  }

  // ── Agents (sessions) ────────────────────────────────────────────────────

  protected async loadSessions(): Promise<void> {
    this.sessionsLoading.set(true);
    this.error.set('');
    try {
      this.sessions.set(await firstValueFrom(this.api.listSessions()));
    } catch (err) {
      this.fail(err);
    } finally {
      this.sessionsLoading.set(false);
    }
  }

  protected async createSession(): Promise<void> {
    if (this.creatingSession()) return;
    const provider = this.newProvider().trim() || 'claude_code';
    this.creatingSession.set(true);
    this.error.set('');
    try {
      await firstValueFrom(
        this.api.createSession({
          provider,
          title: this.newTitle().trim() || undefined,
          workingDirectory: this.newWorkingDir().trim() || undefined,
        }),
      );
      this.newTitle.set('');
      this.newWorkingDir.set('');
      await this.loadSessions();
    } catch (err) {
      this.fail(err);
    } finally {
      this.creatingSession.set(false);
    }
  }

  protected async deleteSession(session: AiSession): Promise<void> {
    this.error.set('');
    try {
      await firstValueFrom(this.api.deleteSession(session.id));
      if (this.wssSessionId() === session.id) this.disconnectWss();
      await this.loadSessions();
    } catch (err) {
      this.fail(err);
    }
  }

  protected async archiveSession(session: AiSession): Promise<void> {
    this.error.set('');
    try {
      await firstValueFrom(this.api.archiveSession(session.id));
      await this.loadSessions();
    } catch (err) {
      this.fail(err);
    }
  }

  protected useInTerminal(session: AiSession): void {
    this.wssSessionId.set(session.id);
    this.segment.set('wss');
  }

  // ── Live terminal (WSS) ──────────────────────────────────────────────────

  private appendLog(line: string): void {
    this.wssLog.update((lines) => [...lines.slice(-40), line]);
  }

  protected async connectWss(): Promise<void> {
    const sessionId = this.wssSessionId().trim();
    if (!sessionId) return;
    this.error.set('');
    try {
      this.appendLog(`connecting → ${sessionId}`);
      this.screenSub?.unsubscribe();
      this.screenSub = this.relay.screens().subscribe((screen) => {
        if (screen.sessionId === this.wssSessionId()) {
          this.wssScreen.set(screen);
          this.appendLog(`terminal_screen (${screen.rows}×${screen.cols}, ${screen.status})`);
        }
      });
      await this.relay.connect();
      await this.relay.subscribeVisual(sessionId);
      await this.relay.refreshVisual(sessionId);
      this.wssConnected.set(true);
      this.appendLog('connected + visual_subscribe + visual_refresh');
    } catch (err) {
      this.fail(err);
      this.appendLog(`error: ${err instanceof Error ? err.message : String(err)}`);
    }
  }

  protected async refreshScreen(): Promise<void> {
    const sessionId = this.wssSessionId().trim();
    if (!sessionId || !this.wssConnected()) return;
    try {
      await this.relay.refreshVisual(sessionId);
      this.appendLog('visual_refresh');
    } catch (err) {
      this.fail(err);
    }
  }

  protected async sendWssInput(): Promise<void> {
    const sessionId = this.wssSessionId().trim();
    const data = this.wssInput();
    if (!sessionId || !this.wssConnected() || !data) return;
    try {
      await this.relay.sendInput(sessionId, `${data}\r`);
      this.appendLog(`sent: ${JSON.stringify(data)}\\r`);
      this.wssInput.set('');
    } catch (err) {
      this.fail(err);
    }
  }

  protected disconnectWss(): void {
    this.screenSub?.unsubscribe();
    this.screenSub = undefined;
    this.relay.disconnect();
    this.wssConnected.set(false);
    this.wssScreen.set(null);
    this.appendLog('disconnected');
  }
}
