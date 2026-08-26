/**
 * Prompt Library (`/settings/prompts`, managing-prompts §3.1). A read-only catalog of
 * stored `PlannerPromptSettings` slots — NAME/ROLE/SCOPE/VER/USED — with client-side
 * filters. Duplicate/Open/+ New navigate to the existing Settings editor; this page
 * does not edit bodies. Every list decision lives in `prompt-library-logic.ts`.
 */
import { CommonModule } from '@angular/common';
import { Component, OnInit, computed, inject, signal } from '@angular/core';
import { Router, RouterLink } from '@angular/router';
import {
  IonBadge,
  IonButton,
  IonButtons,
  IonChip,
  IonContent,
  IonHeader,
  IonItem,
  IonLabel,
  IonList,
  IonNote,
  IonSearchbar,
  IonSelect,
  IonSelectOption,
  IonSpinner,
  IonText,
  IonTitle,
  IonToolbar,
} from '@ionic/angular/standalone';
import { JohnnyApiService, type PromptLibraryEntry } from '@johnnyone/ui';
import { firstValueFrom } from 'rxjs';
import type { LoadState } from '../settings/settings-prompts-logic';
import {
  editorHref,
  editorQueryParams,
  filterRows,
  formatUsed,
  loadView,
  newHref,
  rowAction,
  scopeLabel,
} from './prompt-library-logic';

@Component({
  selector: 'app-prompt-library-page',
  standalone: true,
  imports: [
    CommonModule,
    RouterLink,
    IonHeader,
    IonToolbar,
    IonTitle,
    IonButtons,
    IonButton,
    IonNote,
    IonContent,
    IonSelect,
    IonSelectOption,
    IonSearchbar,
    IonList,
    IonItem,
    IonLabel,
    IonChip,
    IonBadge,
    IonSpinner,
    IonText,
  ],
  templateUrl: './prompt-library.page.html',
  styleUrls: ['./prompt-library.page.scss'],
})
export class PromptLibraryPage implements OnInit {
  private readonly api = inject(JohnnyApiService);
  private readonly router = inject(Router);

  protected readonly rows = signal<PromptLibraryEntry[]>([]);
  protected readonly loadState = signal<LoadState>('idle');
  protected readonly roleFilter = signal('');
  protected readonly scopeFilter = signal('');
  protected readonly query = signal('');

  protected readonly editorHref = editorHref;
  protected readonly editorQueryParams = editorQueryParams;
  protected readonly newHref = newHref;
  protected readonly formatUsed = formatUsed;
  protected readonly scopeLabel = scopeLabel;
  protected readonly rowAction = rowAction;

  protected readonly filtered = computed(() =>
    filterRows(this.rows(), {
      role: this.roleFilter(),
      scope: this.scopeFilter(),
      query: this.query(),
    }),
  );

  protected readonly view = computed(() => loadView(this.loadState(), this.filtered().length));

  private inflight: Promise<void> | null = null;

  ngOnInit(): void {
    void this.load();
  }

  ionViewWillEnter(): void {
    void this.load();
  }

  async load(): Promise<void> {
    if (this.inflight) return this.inflight;
    this.inflight = this.loadInner().finally(() => {
      this.inflight = null;
    });
    return this.inflight;
  }

  private async loadInner(): Promise<void> {
    this.loadState.set('loading');
    try {
      const entries = await firstValueFrom(this.api.listPromptLibrary());
      this.rows.set(entries ?? []);
      this.loadState.set('ready');
    } catch {
      this.loadState.set('load-error');
    }
  }

  onRoleChange(value: string | null | undefined): void {
    this.roleFilter.set(value ?? '');
  }

  onScopeChange(value: string | null | undefined): void {
    this.scopeFilter.set(value ?? '');
  }

  onSearch(value: string | null | undefined): void {
    this.query.set(value ?? '');
  }

  onRowClick(row: PromptLibraryEntry): void {
    if (rowAction(row).kind !== 'open') return;
    void this.router.navigateByUrl(editorHref(row.key));
  }
}
