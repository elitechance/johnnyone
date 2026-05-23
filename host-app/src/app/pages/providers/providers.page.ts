import { Component, inject, signal } from '@angular/core';
import {
  IonCard,
  IonCardContent,
  IonCardHeader,
  IonCardTitle,
  IonChip,
  IonContent,
  IonHeader,
  IonIcon,
  IonItem,
  IonLabel,
  IonList,
  IonText,
  IonTitle,
  IonToolbar,
} from '@ionic/angular/standalone';
import { addIcons } from 'ionicons';
import { checkmarkCircle, closeCircle, terminal } from 'ionicons/icons';
import { HostStatusService } from '../../services/host-status.service';

interface DetectedTool {
  provider: string;
  command: string;
  found: boolean;
  path?: string | null;
}

@Component({
  selector: 'host-providers-page',
  standalone: true,
  imports: [
    IonHeader,
    IonToolbar,
    IonTitle,
    IonContent,
    IonCard,
    IonCardHeader,
    IonCardTitle,
    IonCardContent,
    IonList,
    IonItem,
    IonLabel,
    IonChip,
    IonIcon,
    IonText,
  ],
  templateUrl: './providers.page.html',
  styleUrl: './providers.page.scss',
})
export class ProvidersPage {
  protected readonly status = inject(HostStatusService);
  readonly tools = signal<DetectedTool[]>([]);
  readonly loading = signal(true);

  constructor() {
    addIcons({ checkmarkCircle, closeCircle, terminal });
    this.load();
  }

  async load(): Promise<void> {
    this.loading.set(true);
    try {
      this.tools.set(await this.status.detectCliTools());
    } catch {
      this.tools.set([]);
    } finally {
      this.loading.set(false);
    }
  }
}
