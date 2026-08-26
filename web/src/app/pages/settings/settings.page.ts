import { Component, DestroyRef, OnInit, inject, signal } from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { FormsModule } from '@angular/forms';
import { ActivatedRoute, RouterLink } from '@angular/router';
import {
  IonAccordion,
  IonAccordionGroup,
  IonBadge,
  IonButton,
  IonCard,
  IonCardContent,
  IonCardHeader,
  IonCardSubtitle,
  IonCardTitle,
  IonContent,
  IonHeader,
  IonInput,
  IonItem,
  IonLabel,
  IonList,
  IonNote,
  IonSpinner,
  IonText,
  IonTextarea,
  IonTitle,
  IonToolbar,
} from '@ionic/angular/standalone';
import { firstValueFrom } from 'rxjs';
import { GRAPHQL_API_URL, JohnnyApiService } from '@johnnyone/ui';
import { AuthService } from '../../services/auth.service';
import {
  COMMERCIAL_ROWS,
  PROMPT_KEYS,
  SMALL_MODE_ROWS,
  accordionValueFromQuery,
  applyMutationResult,
  applyResetFailure,
  applySaveFailure,
  differsFromDefault,
  emptyDraft,
  fromSettings,
  isDirty,
  resetKey,
  toUpdateInput,
  type LoadState,
  type PromptDraft,
  type PromptKey,
} from './settings-prompts-logic';

const DISCORD_WEBHOOK_SETTING = 'discord_webhook_url';

@Component({
  selector: 'app-settings-page',
  standalone: true,
  imports: [
    FormsModule,
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
    IonList,
    IonItem,
    IonLabel,
    IonInput,
    IonButton,
    IonText,
    IonNote,
    IonTextarea,
    IonAccordion,
    IonAccordionGroup,
    IonBadge,
    IonSpinner,
  ],
  templateUrl: './settings.page.html',
  styleUrl: './settings.page.scss',
})
export class SettingsPage implements OnInit {
  readonly apiUrl = inject(GRAPHQL_API_URL);
  private readonly auth = inject(AuthService);
  private readonly api = inject(JohnnyApiService);
  private readonly route = inject(ActivatedRoute);
  private readonly destroyRef = inject(DestroyRef);

  readonly commercialRows = COMMERCIAL_ROWS;
  readonly smallModeRows = SMALL_MODE_ROWS;
  readonly differsFromDefault = differsFromDefault;
  readonly isDirty = isDirty;

  tenantId = (() => {
    const id = this.auth.getTenantId();
    return () => id;
  })();

  discordWebhook = '';
  savingDiscord = signal(false);
  discordSaved = signal(false);

  promptLoadState = signal<LoadState>('idle');
  draft = signal<PromptDraft>(emptyDraft());
  lastSaved = signal<PromptDraft>(emptyDraft());
  smallModeSupported = signal(true);
  unsupportedKeys = signal<PromptKey[]>([]);
  savingPrompts = signal(false);
  promptsSaved = signal(false);
  saveError = signal('');
  openPromptAccordion = signal<PromptKey | undefined>(undefined);

  ngOnInit(): void {
    this.route.queryParamMap.pipe(takeUntilDestroyed(this.destroyRef)).subscribe((params) => {
      this.openPromptAccordion.set(
        accordionValueFromQuery(params.get('prompt'), PROMPT_KEYS),
      );
    });
    firstValueFrom(this.api.getSetting(DISCORD_WEBHOOK_SETTING))
      .then((value) => {
        this.discordWebhook = value ?? '';
      })
      .catch(() => {
        /* setting not set yet / host unreachable — leave blank */
      });
    void this.loadPrompts();
  }

  isUnsupported(key: PromptKey): boolean {
    return this.unsupportedKeys().includes(key);
  }

  setPrompt(key: PromptKey, value: string): void {
    this.draft.update((draft) => ({ ...draft, [key]: value }));
    this.promptsSaved.set(false);
    this.saveError.set('');
  }

  async loadPrompts(): Promise<void> {
    this.promptLoadState.set('loading');
    this.saveError.set('');
    try {
      const settings = await firstValueFrom(this.api.getPlannerPromptSettings());
      const { draft, smallModeSupported, unsupportedKeys } = fromSettings(settings);
      this.draft.set(draft);
      this.lastSaved.set({ ...draft });
      this.smallModeSupported.set(smallModeSupported);
      this.unsupportedKeys.set(unsupportedKeys);
      this.promptLoadState.set('ready');
    } catch {
      this.promptLoadState.set('load-error');
    }
  }

  async savePrompts(): Promise<void> {
    if (this.savingPrompts() || this.promptLoadState() !== 'ready') return;
    this.savingPrompts.set(true);
    this.promptsSaved.set(false);
    this.saveError.set('');
    try {
      const returned = await firstValueFrom(
        this.api.updatePlannerPromptSettings(
          toUpdateInput(this.draft(), this.smallModeSupported()),
        ),
      );
      const next = applyMutationResult(returned);
      this.lastSaved.set(next);
      this.draft.set(next);
      this.promptsSaved.set(true);
    } catch {
      const failed = applySaveFailure(this.draft(), this.lastSaved());
      this.draft.set(failed.draft);
      this.lastSaved.set(failed.lastSaved);
      this.saveError.set('Could not save planner prompts.');
    } finally {
      this.savingPrompts.set(false);
    }
  }

  async resetPrompt(key: PromptKey): Promise<void> {
    const result = resetKey(this.draft(), this.lastSaved(), key);
    if (!result) return;
    this.draft.set(result.draft);
    this.savingPrompts.set(true);
    this.promptsSaved.set(false);
    this.saveError.set('');
    try {
      const returned = await firstValueFrom(
        this.api.updatePlannerPromptSettings(
          toUpdateInput(result.persist, this.smallModeSupported()),
        ),
      );
      const saved = applyMutationResult(returned);
      this.lastSaved.set(saved);
      this.draft.update((draft) => ({ ...draft, [key]: saved[key] }));
      this.promptsSaved.set(true);
    } catch {
      this.draft.set(applyResetFailure(this.draft(), this.lastSaved(), key));
      this.saveError.set('Could not save planner prompts.');
    } finally {
      this.savingPrompts.set(false);
    }
  }

  async saveDiscordWebhook(): Promise<void> {
    this.savingDiscord.set(true);
    this.discordSaved.set(false);
    try {
      await firstValueFrom(
        this.api.setSetting(DISCORD_WEBHOOK_SETTING, this.discordWebhook.trim()),
      );
      this.discordSaved.set(true);
    } finally {
      this.savingDiscord.set(false);
    }
  }

  logout(): void {
    this.auth.logout();
  }
}
