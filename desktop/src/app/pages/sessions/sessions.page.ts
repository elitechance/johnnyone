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
  IonNote,
  IonRefresher,
  IonRefresherContent,
} from '@ionic/angular/standalone';
import { Router } from '@angular/router';
import { JohnnyApiService, AiSession } from '@johnnyone/ui';
import { firstValueFrom } from 'rxjs';

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
    IonNote,
    IonRefresher,
    IonRefresherContent,
  ],
  templateUrl: './sessions.page.html',
  styleUrls: ['./sessions.page.scss'],
})
export class SessionsPage implements OnInit {
  private readonly router = inject(Router);
  private readonly api = inject(JohnnyApiService);

  sessions: AiSession[] = [];
  loading = false;

  ngOnInit(): void {
    this.loadSessions();
  }

  async loadSessions(): Promise<void> {
    this.loading = true;
    try {
      this.sessions = await firstValueFrom(this.api.listSessions());
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
      const session = await firstValueFrom(this.api.createSession({}));
      this.sessions.unshift(session);
      this.openSession(session);
    } catch (err) {
      console.error('Failed to create session:', err);
    }
  }

  openSession(session: AiSession): void {
    this.router.navigate(['/chat'], { queryParams: { sessionId: session.id } });
  }

  async archiveSession(session: AiSession): Promise<void> {
    try {
      const updated = await firstValueFrom(this.api.archiveSession(session.id));
      const idx = this.sessions.findIndex((s) => s.id === session.id);
      if (idx >= 0) {
        this.sessions[idx] = updated;
      }
    } catch (err) {
      console.error('Failed to archive session:', err);
    }
  }

  async deleteSession(session: AiSession): Promise<void> {
    try {
      await firstValueFrom(this.api.deleteSession(session.id));
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
