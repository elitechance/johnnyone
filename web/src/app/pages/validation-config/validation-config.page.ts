import { CommonModule } from '@angular/common';
import { Component, OnInit, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { ActivatedRoute } from '@angular/router';
import {
  IonHeader,
  IonToolbar,
  IonTitle,
  IonButtons,
  IonBackButton,
  IonContent,
  IonItem,
  IonLabel,
  IonInput,
  IonSelect,
  IonSelectOption,
  IonToggle,
  IonReorderGroup,
  IonReorder,
  IonButton,
  IonIcon,
  IonNote,
  ToastController,
  type ItemReorderCustomEvent,
} from '@ionic/angular/standalone';
import { addIcons } from 'ionicons';
import { addOutline, trashOutline, reorderThreeOutline } from 'ionicons/icons';
import { JohnnyApiService } from '@johnnyone/ui';
import {
  type LensDraft,
  addLens,
  removeLens,
  moveLens,
  providerOptions,
  toConfigJson,
  fromConfigJson,
  defaultLenses,
} from './validation-config-logic';

/**
 * "Validation · configure" (mock §07, overhaul P7). An ordered, drag-reorderable list of review
 * lenses on the Initiative — per-lens name, provider (real `CliProvider` registry, D9) + free-text
 * model, a `vision` flag (design-authority §3, D14), and a BLOCK/WARN toggle. Add/remove/reorder
 * freely (min 1, no max). Every list transform delegates to the pure `validation-config-logic.ts`;
 * the page is thin. Saves to the Initiative's `validationConfig` via `updateAgentPlanValidationConfig`
 * (Phase 01). Reached by one lazy `authGuard` route (D12); live activation deferred to the desktop
 * rebuild (D8).
 */
@Component({
  selector: 'app-validation-config',
  standalone: true,
  imports: [
    CommonModule,
    FormsModule,
    IonHeader,
    IonToolbar,
    IonTitle,
    IonButtons,
    IonBackButton,
    IonContent,
    IonItem,
    IonLabel,
    IonInput,
    IonSelect,
    IonSelectOption,
    IonToggle,
    IonReorderGroup,
    IonReorder,
    IonButton,
    IonIcon,
    IonNote,
  ],
  templateUrl: './validation-config.page.html',
  styleUrls: ['./validation-config.page.scss'],
})
export class ValidationConfigPage implements OnInit {
  private readonly api = inject(JohnnyApiService);
  private readonly route = inject(ActivatedRoute);
  private readonly toast = inject(ToastController);

  protected readonly lenses = signal<LensDraft[]>([]);
  protected readonly loading = signal(true);
  protected readonly saving = signal(false);
  protected readonly loadError = signal<string | null>(null);
  /** Provider dropdown options — the real registry minus `shell` (D9). */
  protected readonly providers = providerOptions();

  private planId = '';

  constructor() {
    addIcons({ addOutline, trashOutline, reorderThreeOutline });
  }

  ngOnInit(): void {
    const id = this.route.snapshot.paramMap.get('id') ?? '';
    this.planId = id;
    if (!id) {
      this.loadError.set('No initiative id in the route.');
      this.lenses.set(defaultLenses());
      this.loading.set(false);
      return;
    }
    this.api.getAgentPlan(id).subscribe({
      next: (run) => {
        this.lenses.set(fromConfigJson(run.plan.validationConfig));
        this.loading.set(false);
      },
      error: () => {
        // Non-fatal: show the default template so the operator can still configure + save.
        this.loadError.set('Could not load the initiative; showing the default template.');
        this.lenses.set(defaultLenses());
        this.loading.set(false);
      },
    });
  }

  /** Remove is disabled while a single lens remains (min 1). */
  protected canRemove(): boolean {
    return this.lenses().length > 1;
  }

  /** `ion-reorder-group` drop → reorder via the pure `moveLens`, then release the gesture. */
  protected onReorder(ev: ItemReorderCustomEvent): void {
    this.lenses.set(moveLens(this.lenses(), ev.detail.from, ev.detail.to));
    ev.detail.complete();
  }

  protected onAddLens(): void {
    this.lenses.set(addLens(this.lenses()));
  }

  protected removeLensAt(index: number): void {
    this.lenses.set(removeLens(this.lenses(), index));
  }

  /** Persist the drafts as the Initiative's `validationConfig` JSON (Phase 01 mutation). */
  protected save(): void {
    if (!this.planId || this.saving()) return;
    this.saving.set(true);
    this.api
      .updateAgentPlanValidationConfig(this.planId, toConfigJson(this.lenses()))
      .subscribe({
        next: () => {
          this.saving.set(false);
          void this.notify('Validation config saved.');
        },
        error: (err: unknown) => {
          this.saving.set(false);
          void this.notify(this.errorText(err), true);
        },
      });
  }

  private errorText(err: unknown): string {
    if (err && typeof err === 'object' && 'message' in err) {
      return String((err as { message: unknown }).message);
    }
    return 'Could not save the validation config.';
  }

  private async notify(message: string, danger = false): Promise<void> {
    const toast = await this.toast.create({
      message,
      duration: 2200,
      position: 'bottom',
      color: danger ? 'danger' : 'success',
    });
    await toast.present();
  }
}
