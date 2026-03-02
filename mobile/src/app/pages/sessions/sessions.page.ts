import { Component, inject, signal, OnInit } from '@angular/core';
import { CommonModule } from '@angular/common';
import {
  IonHeader,
  IonToolbar,
  IonTitle,
  IonContent,
  IonButtons,
  IonButton,
  IonIcon,
} from '@ionic/angular/standalone';
import { addIcons } from 'ionicons';
import { addOutline } from 'ionicons/icons';
import {
  SessionListComponent,
  JohnnyApiService,
  AiSession,
} from '@johnnyone/ui';
import { Router } from '@angular/router';

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
    IonButton,
    IonIcon,
    SessionListComponent,
  ],
  templateUrl: './sessions.page.html',
  styleUrls: ['./sessions.page.scss'],
})
export class SessionsPage implements OnInit {
  private readonly api = inject(JohnnyApiService);
  private readonly router = inject(Router);

  readonly sessions = signal<AiSession[]>([]);

  constructor() {
    addIcons({ 'add-outline': addOutline });
  }

  ngOnInit(): void {
    this.api.listSessions().subscribe((sessions) => {
      this.sessions.set(sessions);
    });
  }

  createSession(): void {
    this.api.createSession({}).subscribe((session) => {
      this.router.navigate(['/chat'], { queryParams: { sessionId: session.id } });
    });
  }

  openSession(sessionId: string): void {
    this.router.navigate(['/chat'], { queryParams: { sessionId } });
  }

  archiveSession(sessionId: string): void {
    this.api.archiveSession(sessionId).subscribe(() => {
      this.sessions.update((s) => s.filter((x) => x.id !== sessionId));
    });
  }
}
