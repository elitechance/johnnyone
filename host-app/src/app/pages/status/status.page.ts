import { Component, inject, signal } from '@angular/core';
import {
  IonButton,
  IonCard,
  IonCardContent,
  IonCardHeader,
  IonCardSubtitle,
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
import { checkmarkCircle, closeCircle, openOutline } from 'ionicons/icons';
import { HostAuthService } from '../../services/host-auth.service';
import { HostStatusService } from '../../services/host-status.service';

@Component({
  selector: 'host-status-page',
  standalone: true,
  imports: [
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
    IonChip,
    IonIcon,
    IonText,
    IonButton,
  ],
  templateUrl: './status.page.html',
  styleUrl: './status.page.scss',
})
export class StatusPage {
  private readonly auth = inject(HostAuthService);
  protected readonly status = inject(HostStatusService);

  readonly user = this.auth.currentUser;
  readonly webUrl = signal(
    'https://johnnyone-dev.pages.dev/',
  );

  constructor() {
    addIcons({ checkmarkCircle, closeCircle, openOutline });
    this.status.refresh();
  }

  openWeb(): void {
    window.open(this.webUrl(), '_blank', 'noopener,noreferrer');
  }

  signOut(): void {
    this.auth.logout();
  }
}
