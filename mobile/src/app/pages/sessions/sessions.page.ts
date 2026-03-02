import { Component, inject, signal, OnInit, OnDestroy } from '@angular/core';
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
import { SessionListComponent } from '@johnnyone/ui';
import { SessionService, Session } from '@johnnyone/core';
import { Subscription } from 'rxjs';

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
export class SessionsPage implements OnInit, OnDestroy {
  private readonly sessionService = inject(SessionService);
  private subscription: Subscription | null = null;

  readonly sessions = signal<Session[]>([]);

  constructor() {
    addIcons({ 'add-outline': addOutline });
  }

  ngOnInit(): void {
    this.subscription = this.sessionService.sessions$.subscribe((sessions) => {
      this.sessions.set(sessions);
    });
    this.sessionService.loadSessions();
  }

  ngOnDestroy(): void {
    this.subscription?.unsubscribe();
  }

  createSession(): void {
    this.sessionService.createSession();
  }

  openSession(session: Session): void {
    this.sessionService.setActiveSession(session.id);
  }

  archiveSession(session: Session): void {
    this.sessionService.archiveSession(session.id);
  }
}
