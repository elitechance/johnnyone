import { Component, computed, input, output } from '@angular/core';
import {
  IonButton,
  IonButtons,
  IonContent,
  IonHeader,
  IonItem,
  IonLabel,
  IonList,
  IonNote,
  IonSpinner,
  IonTitle,
  IonToolbar,
} from '@ionic/angular/standalone';
import type { DirListing } from '@johnnyone/ui';
import {
  absolutePath,
  breadcrumbs,
  canChoose,
  dirEntries,
  displayPath,
  parentPath,
} from './host-dir-picker-logic';

/**
 * The workspace-folder picker (presentational), shared by `/briefing/new` and the New-plan modal.
 *
 * The HOST owns the `ion-modal` and the `listDir` calls (container/presentational split): it passes
 * the current `listing` / `loading` / `error` in and reacts to `navigate(relPath)` (load that folder),
 * `choose(absPath)` (use it) and `cancel`. This component never injects a service.
 */
@Component({
  selector: 'app-host-dir-picker',
  standalone: true,
  host: { class: 'ion-page' },
  imports: [
    IonHeader,
    IonToolbar,
    IonTitle,
    IonButtons,
    IonButton,
    IonContent,
    IonList,
    IonItem,
    IonLabel,
    IonNote,
    IonSpinner,
  ],
  templateUrl: './host-dir-picker.component.html',
  styleUrl: './host-dir-picker.component.scss',
})
export class HostDirPickerComponent {
  readonly listing = input<DirListing | null>(null);
  readonly loading = input(false);
  readonly error = input<string | null>(null);
  /** Header title (the host decides the wording). */
  readonly heading = input('Pick a workspace folder');

  readonly navigate = output<string>();
  readonly choose = output<string>();
  readonly cancel = output<void>();

  protected readonly dirs = computed(() => dirEntries(this.listing()));
  protected readonly crumbs = computed(() => breadcrumbs(this.listing()));
  protected readonly shownPath = computed(() => displayPath(this.listing()));
  protected readonly parent = computed(() => parentPath(this.listing()?.path));
  protected readonly chooseEnabled = computed(() =>
    canChoose(this.listing(), this.loading(), this.error()),
  );

  protected up(): void {
    const p = this.parent();
    if (p !== null) this.navigate.emit(p);
  }

  protected useCurrent(): void {
    if (this.chooseEnabled()) this.choose.emit(absolutePath(this.listing()));
  }
}
