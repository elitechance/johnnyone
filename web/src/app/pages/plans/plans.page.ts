import { Component, OnDestroy, OnInit, computed, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { Router } from '@angular/router';
import {
  AlertController,
  IonBadge,
  IonButton,
  IonButtons,
  IonCard,
  IonCardContent,
  IonCardHeader,
  IonCardSubtitle,
  IonCardTitle,
  IonContent,
  IonHeader,
  IonIcon,
  IonInput,
  IonItem,
  IonList,
  IonModal,
  IonNote,
  IonRefresher,
  IonRefresherContent,
  IonSkeletonText,
  IonTitle,
  IonToolbar,
  ToastController,
} from '@ionic/angular/standalone';
import { addIcons } from 'ionicons';
import {
  addOutline,
  alertCircleOutline,
  copyOutline,
  createOutline,
  documentTextOutline,
  folderOpenOutline,
  refreshOutline,
  trashOutline,
} from 'ionicons/icons';
import { JohnnyApiService, type DirListing, type DraftPlan } from '@johnnyone/ui';
import { Subscription, firstValueFrom } from 'rxjs';
import { HostDirPickerComponent } from '../../components/host-dir-picker/host-dir-picker.component';
import { RelayTerminalService } from '../../services/relay-terminal.service';
import { draftCountLabel, listState, newPlanValid, planCards, type PlanCard } from './plans-list-logic';

/**
 * `/plans` — the draft plans (briefing-stage initiatives) of the Plans tab (initiative 8ad3ae71,
 * phase 03 task 05; screens S2-S4). A container: it loads `listDraftPlans`, renders the pure
 * `planCards` view model, and runs New / Rename / Duplicate / Delete. Every decision lives in
 * `plans-list-logic.ts`. Live: any `agent_plan_run_updated` push for a listed (or new) plan reloads.
 */
@Component({
  selector: 'app-plans',
  standalone: true,
  imports: [
    FormsModule,
    IonHeader,
    IonToolbar,
    IonTitle,
    IonButtons,
    IonButton,
    IonIcon,
    IonContent,
    IonRefresher,
    IonRefresherContent,
    IonCard,
    IonCardHeader,
    IonCardTitle,
    IonCardSubtitle,
    IonCardContent,
    IonBadge,
    IonSkeletonText,
    IonModal,
    IonList,
    IonItem,
    IonInput,
    IonNote,
    HostDirPickerComponent,
  ],
  templateUrl: './plans.page.html',
  styleUrl: './plans.page.scss',
})
export class PlansPage implements OnInit, OnDestroy {
  private readonly api = inject(JohnnyApiService);
  private readonly router = inject(Router);
  private readonly relay = inject(RelayTerminalService);
  private readonly alertCtrl = inject(AlertController);
  private readonly toastCtrl = inject(ToastController);
  private pushSub?: Subscription;

  protected readonly rows = signal<DraftPlan[]>([]);
  protected readonly loading = signal(true);
  protected readonly error = signal<string | null>(null);
  /** Id of the card with an action in flight (its buttons are disabled). */
  protected readonly busyId = signal<string | null>(null);
  private readonly now = signal(new Date().toISOString());

  protected readonly cards = computed<PlanCard[]>(() => planCards(this.rows(), this.now()));
  protected readonly state = computed(() => listState(this.loading(), this.error(), this.rows()));
  protected readonly countLabel = computed(() => draftCountLabel(this.rows().length));

  // ── New plan modal ──
  protected readonly newOpen = signal(false);
  protected readonly pickerOpen = signal(false);
  protected newTitle = '';
  protected readonly newWorkspace = signal('');
  protected readonly creating = signal(false);
  protected readonly pickerListing = signal<DirListing | null>(null);
  protected readonly pickerLoading = signal(false);
  protected readonly pickerError = signal<string | null>(null);
  protected readonly titleValue = signal('');
  protected readonly canCreate = computed(() => newPlanValid(this.titleValue(), this.newWorkspace()) && !this.creating());

  constructor() {
    addIcons({
      'add-outline': addOutline,
      'alert-circle-outline': alertCircleOutline,
      'copy-outline': copyOutline,
      'create-outline': createOutline,
      'document-text-outline': documentTextOutline,
      'folder-open-outline': folderOpenOutline,
      'refresh-outline': refreshOutline,
      'trash-outline': trashOutline,
    });
  }

  ngOnInit(): void {
    void this.load();
    // Live list: a brief write, rename, create or delete pushes `agent_plan_run_updated`. A missing
    // socket is not fatal — Retry / pull-to-refresh still work.
    this.pushSub = this.relay.agentPlanRuns().subscribe((u) => {
      const known = this.rows().some((r) => r.id === u.planId);
      const briefing = u.run?.plan?.initiativeStatus === 'briefing';
      if (known || briefing) void this.load(true);
    });
    void this.relay.connect().catch(() => undefined);
  }

  ngOnDestroy(): void {
    this.pushSub?.unsubscribe();
  }

  /** Load the list. Bound directly to Retry / Refresh; `quiet` = a background refresh (push, pull). */
  protected async load(quiet = false): Promise<void> {
    if (!quiet) {
      this.loading.set(true);
      this.error.set(null);
    }
    try {
      const rows = await firstValueFrom(this.api.listDraftPlans());
      this.rows.set(rows ?? []);
      this.error.set(null);
    } catch (err) {
      if (!quiet) this.error.set(`Couldn't load plans: ${message(err)}`);
    } finally {
      this.now.set(new Date().toISOString());
      this.loading.set(false);
    }
  }

  protected async handleRefresh(ev: CustomEvent): Promise<void> {
    try {
      await this.load(true);
    } finally {
      (ev.target as HTMLIonRefresherElement | null)?.complete();
    }
  }

  protected open(card: PlanCard): void {
    void this.router.navigate(['/plans', card.id]);
  }

  // ── Rename ──
  protected async rename(card: PlanCard, ev?: Event): Promise<void> {
    ev?.stopPropagation();
    const alert = await this.alertCtrl.create({
      header: 'Rename plan',
      inputs: [{ name: 'title', type: 'text', value: card.title, attributes: { maxlength: 200 } }],
      buttons: [
        { text: 'Cancel', role: 'cancel' },
        { text: 'Rename', role: 'confirm' },
      ],
    });
    await alert.present();
    const { role, data } = await alert.onDidDismiss();
    const title = String(data?.values?.title ?? '').trim();
    if (role !== 'confirm' || !title || title === card.title) return;
    await this.run(card.id, async () => {
      await firstValueFrom(this.api.updateAgentPlanTitle(card.id, title));
      await this.toast('Plan renamed');
      await this.load(true);
    });
  }

  // ── Duplicate ──
  protected async duplicate(card: PlanCard, ev?: Event): Promise<void> {
    ev?.stopPropagation();
    await this.run(card.id, async () => {
      const copy = await firstValueFrom(this.api.createDraftPlanDuplicate(card.id));
      await this.toast('Plan duplicated');
      void this.router.navigate(['/plans', copy.plan.id]);
    });
  }

  // ── Delete ──
  protected async remove(card: PlanCard, ev?: Event): Promise<void> {
    ev?.stopPropagation();
    const alert = await this.alertCtrl.create({
      header: 'Delete draft plan?',
      message: 'Its chat is archived.',
      buttons: [
        { text: 'Cancel', role: 'cancel' },
        { text: 'Delete', role: 'destructive' },
      ],
    });
    await alert.present();
    const { role } = await alert.onDidDismiss();
    if (role !== 'destructive') return;
    await this.run(card.id, async () => {
      await firstValueFrom(this.api.deleteAgentPlan(card.id));
      this.rows.update((rs) => rs.filter((r) => r.id !== card.id));
      await this.toast('Plan deleted');
    });
  }

  /** Run one card action with its buttons disabled; errors become a toast. */
  private async run(id: string, fn: () => Promise<void>): Promise<void> {
    this.busyId.set(id);
    try {
      await fn();
    } catch (err) {
      await this.toast(message(err), 'danger');
    } finally {
      this.busyId.set(null);
    }
  }

  // ── New plan ──
  protected openNew(): void {
    this.newTitle = '';
    this.titleValue.set('');
    this.newWorkspace.set('');
    this.newOpen.set(true);
  }

  protected closeNew(): void {
    this.newOpen.set(false);
    this.pickerOpen.set(false);
  }

  protected onTitle(value: string | null | undefined): void {
    this.newTitle = value ?? '';
    this.titleValue.set(this.newTitle);
  }

  protected async openPicker(): Promise<void> {
    this.pickerOpen.set(true);
    if (!this.pickerListing()) await this.browseTo('');
  }

  protected closePicker(): void {
    this.pickerOpen.set(false);
  }

  protected async browseTo(relPath: string): Promise<void> {
    this.pickerLoading.set(true);
    this.pickerError.set(null);
    try {
      this.pickerListing.set(await firstValueFrom(this.api.listDir(relPath)));
    } catch (err) {
      this.pickerError.set(message(err));
    } finally {
      this.pickerLoading.set(false);
    }
  }

  protected chooseDir(absPath: string): void {
    this.newWorkspace.set(absPath);
    this.pickerOpen.set(false);
  }

  protected async create(): Promise<void> {
    if (!this.canCreate()) return;
    this.creating.set(true);
    try {
      const run = await firstValueFrom(
        this.api.createDraftPlan({ title: this.newTitle.trim(), workspacePath: this.newWorkspace() }),
      );
      this.newOpen.set(false);
      void this.router.navigate(['/plans', run.plan.id]);
    } catch (err) {
      await this.toast(message(err), 'danger');
    } finally {
      this.creating.set(false);
    }
  }

  private async toast(text: string, color: 'success' | 'danger' = 'success'): Promise<void> {
    const t = await this.toastCtrl.create({ message: text, duration: 2500, color, position: 'bottom' });
    await t.present();
  }
}

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}
