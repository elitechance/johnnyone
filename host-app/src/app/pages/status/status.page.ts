import { Component, inject, signal } from '@angular/core';
import { Router } from '@angular/router';
import {
  IonButton,
  IonButtons,
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
import { checkmarkCircle, closeCircle, openOutline, settingsOutline } from 'ionicons/icons';
import { HostAuthService } from '../../services/host-auth.service';
import { HostStatusService } from '../../services/host-status.service';

@Component({
  selector: 'host-status-page',
  standalone: true,
  imports: [
    IonHeader,
    IonToolbar,
    IonButtons,
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
  private readonly router = inject(Router);
  protected readonly status = inject(HostStatusService);

  readonly user = this.auth.currentUser;
  readonly webUrl = this.status.webClientUrl;

  constructor() {
    addIcons({ checkmarkCircle, closeCircle, openOutline, settingsOutline });
    void this.status.refresh();
  }

  openWeb(): void {
    window.open(this.webUrl(), '_blank', 'noopener,noreferrer');
  }

  openSettings(): void {
    void this.router.navigateByUrl('/settings');
  }

  signOut(): void {
    this.auth.logout();
    void this.router.navigateByUrl('/login');
  }
}