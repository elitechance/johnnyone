import { Component, OnInit, inject } from '@angular/core';
import { CommonModule } from '@angular/common';
import {
  IonHeader,
  IonToolbar,
  IonTitle,
  IonContent,
  IonButtons,
  IonBackButton,
  IonButton,
  IonList,
  IonItem,
  IonLabel,
  IonIcon,
  IonBadge,
  IonItemSliding,
  IonItemOptions,
  IonItemOption,
  IonFab,
  IonFabButton,
  IonNote,
  IonRefresher,
  IonRefresherContent,
} from '@ionic/angular/standalone';
import { Router } from '@angular/router';
import { TauriBridgeService, Session } from '../../services/tauri-bridge.service';

@Component({
  selector: 'app-sessions',
  standalone: true,
  imports: [
    CommonModule,
    IonHeader,
    IonToolbar,
    IonTitle,
    IonContent,
    IonButtons,
    IonBackButton,
    IonButton,
    IonList,
    IonItem,
    IonLabel,
    IonIcon,
    IonBadge,
    IonItemSliding,
    IonItemOptions,
    IonItemOption,
    IonFab,
    IonFabButton,
    IonNote,
    IonRefresher,
    IonRefresherContent,
  ],
  templateUrl: './sessions.page.html',
  styleUrls: ['./sessions.page.scss'],
})
export class SessionsPage implements OnInit {
  private readonly router = inject(Router);
  private readonly tauriBridge = inject(TauriBridgeService);

  sessions: Session[] = [];
  loading = false;

  ngOnInit(): void {
    this.loadSessions();
  }

  async loadSessions(): Promise<void> {
    this.loading = true;
    try {
      this.sessions = await this.tauriBridge.listSessions();
    } catch (err) {
      console.error('Failed to load sessions:', err);
    } finally {
      this.loading = false;
    }
  }

  async handleRefresh(event: any): Promise<void> {
    await this.loadSessions();
    event.target.complete();
  }

  async createSession(): Promise<void> {
    try {
      const session = await this.tauriBridge.createSession({});
      this.sessions.unshift(session);
      this.openSession(session);
    } catch (err) {
      console.error('Failed to create session:', err);
    }
  }

  openSession(session: Session): void {
    this.router.navigate(['/chat'], { queryParams: { sessionId: session.id } });
  }

  async archiveSession(session: Session): Promise<void> {
    try {
      const updated = await this.tauriBridge.archiveSession(session.id);
      const idx = this.sessions.findIndex((s) => s.id === session.id);
      if (idx >= 0) {
        this.sessions[idx] = updated;
      }
    } catch (err) {
      console.error('Failed to archive session:', err);
    }
  }

  async deleteSession(session: Session): Promise<void> {
    try {
      await this.tauriBridge.deleteSession(session.id);
      this.sessions = this.sessions.filter((s) => s.id !== session.id);
    } catch (err) {
      console.error('Failed to delete session:', err);
    }
  }

  getStatusColor(status: string): string {
    switch (status) {
      case 'active':
        return 'success';
      case 'archived':
        return 'medium';
      default:
        return 'medium';
    }
  }

  goBack(): void {
    this.router.navigate(['/chat']);
  }
}
