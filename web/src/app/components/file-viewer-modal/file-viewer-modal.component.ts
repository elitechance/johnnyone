import { ChangeDetectionStrategy, Component, computed, inject } from '@angular/core';
import { CommonModule } from '@angular/common';
import {
  IonButton,
  IonButtons,
  IonContent,
  IonHeader,
  IonIcon,
  IonModal,
  IonTitle,
  IonToolbar,
} from '@ionic/angular/standalone';
import { DomSanitizer, SafeResourceUrl } from '@angular/platform-browser';
import { addIcons } from 'ionicons';
import { closeOutline } from 'ionicons/icons';
import { FileViewerService } from '../../services/file-viewer.service';

addIcons({ 'close-outline': closeOutline });

/**
 * Shows a file a report pointed at, over the page rather than in a new tab.
 *
 * On a phone a new tab means leaving the conversation and finding your way back, so this keeps
 * the file in the app with one obvious way out. Mounted once at the app root and driven by
 * `FileViewerService`, mirroring how the mermaid zoom modal works.
 *
 * HTML renders inside a SANDBOXED iframe: the content comes from the workspace, which an agent
 * writes, so it is not trusted to run with the app's origin or cookies.
 */
@Component({
  selector: 'app-file-viewer-modal',
  standalone: true,
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [
    CommonModule,
    IonModal,
    IonHeader,
    IonToolbar,
    IonTitle,
    IonButtons,
    IonButton,
    IonIcon,
    IonContent,
  ],
  templateUrl: './file-viewer-modal.component.html',
  styleUrl: './file-viewer-modal.component.scss',
})
export class FileViewerModalComponent {
  protected readonly viewer = inject(FileViewerService);
  private readonly sanitizer = inject(DomSanitizer);

  /** Just the filename — a full path overflows the toolbar on a phone. */
  protected readonly title = computed(() => {
    const path = this.viewer.view()?.path ?? '';
    return path.split('/').filter(Boolean).slice(-1)[0] || 'File';
  });

  /**
   * The iframe/image source.
   *
   * Angular blocks a blob: or data: URL on `[src]` unless it is explicitly trusted. This is our
   * own object URL built from bytes the host just handed us, and the iframe is sandboxed, so
   * trusting it here is bounded.
   */
  protected readonly safeSource = computed<SafeResourceUrl | null>(() => {
    const view = this.viewer.view();
    if (!view || view.kind === 'text') return null;
    return this.sanitizer.bypassSecurityTrustResourceUrl(view.source);
  });

  protected close(): void {
    this.viewer.close();
  }
}
