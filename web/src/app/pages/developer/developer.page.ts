import { Component, OnInit, computed, inject, signal } from '@angular/core';
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
  IonSpinner,
  IonText,
  IonTitle,
  IonToolbar,
} from '@ionic/angular/standalone';
import { addIcons } from 'ionicons';
import { copyOutline, keyOutline, refreshOutline, trashOutline } from 'ionicons/icons';
import { ApiKey, JohnnyApiService } from '@johnnyone/ui';
import { firstValueFrom } from 'rxjs';
import { AuthService } from '../../services/auth.service';

/** Scopes a partner API key can carry (worker scopes.ts / runbook §M2M). */
const ALL_SCOPES = [
  'terminal:read',
  'terminal:write',
  'sessions:read',
  'sessions:write',
  'plans:read',
  'plans:write',
];

/**
 * Developer console — in-app management for the partner API credentials:
 * the account identity, the current access token, and full M2M API-key
 * lifecycle (create with scopes/expiry, reveal-once secret, revoke, delete).
 *
 * Authenticated route (/developer). Session CRUD and the live WSS terminal were
 * intentionally removed: the session list reflected DB records, not live tmux
 * state, which was misleading. Sessions + WSS remain documented at /integration
 * for API consumers.
 */
@Component({
  selector: 'app-developer-page',
  standalone: true,
  imports: [
    IonHeader,
    IonToolbar,
    IonTitle,
    IonContent,
    IonList,
    IonListHeader,
    IonItem,
    IonLabel,
    IonInput,
    IonButton,
    IonIcon,
    IonNote,
    IonText,
    IonBadge,
    IonChip,
    IonCheckbox,
    IonSpinner,
  ],
  templateUrl: './developer.page.html',
  styleUrl: './developer.page.scss',
})
export class DeveloperPage implements OnInit {
  private readonly api = inject(JohnnyApiService);
  private readonly auth = inject(AuthService);

  protected readonly allScopes = ALL_SCOPES;
  protected readonly error = signal<string>('');

  // ── Account ──────────────────────────────────────────────────────────────
  protected readonly user = this.auth.currentUser;
  protected readonly tenantId = this.auth.getTenantId();
  protected readonly accessToken = signal<string>(this.auth.getAccessToken() ?? '');
  protected readonly tokenPreview = computed(() => {
    const t = this.accessToken();
    return t ? `${t.slice(0, 12)}…${t.slice(-6)}` : '(none)';
  });

  // ── API keys ─────────────────────────────────────────────────────────────
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

  protected async deleteKey(key: ApiKey): Promise<void> {
    if (!confirm(`Delete API key "${key.name}"? This permanently removes it.`)) return;
    this.error.set('');
    try {
      await firstValueFrom(this.api.deleteApiKey(key.id));
      await this.loadApiKeys();
    } catch (err) {
      this.fail(err);
    }
  }
}
