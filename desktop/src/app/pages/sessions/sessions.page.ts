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

export interface AiSession {
  id: string;
  name: string;
  createdAt: Date;
  lastMessageAt: Date;
  messageCount: number;
  status: 'active' | 'archived' | 'expired';
  provider: string;
  model: string;
}

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

  sessions: AiSession[] = [];
  loading = false;

  ngOnInit(): void {
    this.loadSessions();
  }

  async loadSessions(): Promise<void> {
    this.loading = true;
    try {
      // In production, this would call JohnnyApiService to fetch sessions
      // For now, populate with placeholder data structure
      this.sessions = [];
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
      const newSession: AiSession = {
        id: crypto.randomUUID(),
        name: `Session ${new Date().toLocaleDateString()}`,
        createdAt: new Date(),
        lastMessageAt: new Date(),
        messageCount: 0,
        status: 'active',
        provider: 'ollama',
        model: 'llama3',
      };
      this.sessions.unshift(newSession);
      this.openSession(newSession);
    } catch (err) {
      console.error('Failed to create session:', err);
    }
  }

  openSession(session: AiSession): void {
    this.router.navigate(['/chat'], { queryParams: { sessionId: session.id } });
  }

  async archiveSession(session: AiSession): Promise<void> {
    try {
      session.status = 'archived';
      // In production, persist via JohnnyApiService
    } catch (err) {
      console.error('Failed to archive session:', err);
    }
  }

  async deleteSession(session: AiSession): Promise<void> {
    try {
      this.sessions = this.sessions.filter((s) => s.id !== session.id);
      // In production, delete via JohnnyApiService
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
      case 'expired':
        return 'danger';
      default:
        return 'medium';
    }
  }

  goBack(): void {
    this.router.navigate(['/chat']);
  }
}
